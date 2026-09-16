//! 错误抛出与有界强转：强转失败统一出口、调用错误恢复、错误构造与异常
//! 展开、有界数值强转三件套与抛出错误 kind 查询。

use oxide_runtime_api as coercion;

use oxide_types::error::JsError;
use oxide_types::object::JsObject;
use oxide_types::value::{JsValue, PTR_MASK};

use super::{format_error_message, js_error_kind, js_error_kind_name, Vm};
use crate::vm_debug;

impl Vm {
    pub(crate) fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String> {
        if !value.is_object() {
            return Ok(value);
        }

        let obj_ptr = value.as_js_object_ptr();
        if obj_ptr.is_null() {
            return Ok(value);
        }

        // ECMA-262 §7.1.1 step 1: an exotic obj[Symbol.toPrimitive] takes precedence
        // over OrdinaryToPrimitive. well-known symbol 键经 property_key_si 映射为
        // 固定 Symbol 键，读键路径与写键路径一致。
        let sym_key = {
            let sym_ptr = self.session.builtin_world().sym_to_primitive.as_ptr() as *mut JsObject;
            JsValue::from_js_object(sym_ptr)
        };
        let sym_si = self.property_key_si(sym_key)?;
        let exotic = {
            let obj = unsafe { &*obj_ptr };
            self.ordinary_get(obj, sym_si, value)?
        };
        if !exotic.is_undefined() && !exotic.is_null() {
            let exotic_ptr = exotic.as_js_object_ptr();
            if !exotic.is_object() || exotic_ptr.is_null() || !unsafe { &*exotic_ptr }.is_function() {
                // 抛可捕获的 JS 异常（dispatch 层 try/catch 可捕获），见 conversion_error。
                self.conversion_error("Symbol.toPrimitive is not a function")?;
                return Ok(JsValue::undefined());
            }
            let hint_val = self.new_string(if prefer_string { "string" } else { "number" });
            let result = match self.call_function_sync(exotic, value, &[hint_val]) {
                Ok(r) => r,
                Err(err) => return self.raise_call_error(&err),
            };
            if result.is_object() {
                self.conversion_error("Cannot convert object to primitive value")?;
                return Ok(JsValue::undefined());
            }
            return Ok(result);
        }

        let method_names = if prefer_string { ["toString", "valueOf"] } else { ["valueOf", "toString"] };

        for method_name in method_names {
            let method_si = self.kernel_core.perm_interner().intern(method_name).0;
            let method = {
                let obj = unsafe { &*obj_ptr };
                self.ordinary_get(obj, method_si, value)?
            };
            if method.is_undefined() || method.is_null() {
                continue;
            }
            // OrdinaryToPrimitive：valueOf/toString 不可调用时跳过（IsCallable == false
            // 则 continue），不抛错；仅 @@toPrimitive 不可调用时抛 TypeError。
            if !method.is_object() {
                continue;
            }
            let method_ptr = method.as_js_object_ptr();
            if method_ptr.is_null() || !unsafe { &*method_ptr }.is_function() {
                continue;
            }

            let result = match self.call_function_sync(method, value, &[]) {
                Ok(r) => r,
                Err(err) => return self.raise_call_error(&err),
            };
            if !result.is_object() {
                return Ok(result);
            }
        }

        // 主 dispatch 抛可捕获异常（外围 JS try/catch 可捕获）；原生 builtin 内部
        // 只传播格式化 Err，由其调用边界恢复为异常对象（见 conversion_error）。
        self.conversion_error("Cannot convert object to primitive value")?;
        Ok(JsValue::undefined())
    }

    /// 对象转原始值失败时的统一出口：主 dispatch（native_call_depth == 0）下抛可捕获
    /// 的 JS 异常并就地展开到外围 try/catch；原生 builtin 内部（depth > 0）不得就地
    /// 展开（展开会消费 try 处理器，builtin 却继续执行并可能产生二次错误），改为返回
    /// 格式化 Err，由原生调用边界（call_function_sync → dispatch_native_call）恢复为
    /// 原始异常对象。
    #[inline(always)]
    fn conversion_error(&mut self, msg: &str) -> Result<(), String> {
        if self.native_call_depth == 0 {
            self.raise_error_kind("TypeError", msg)
        } else {
            Err(self.error_message_text("TypeError", msg))
        }
    }

    /// 把 `call_function_sync` 返回的调用错误恢复为原始异常值并走异常展开，
    /// 使外围 try/catch 可捕获（native 函数抛错时原值存于 `last_uncaught_value`）。
    ///
    /// # 注意事项
    /// 仅在主 dispatch（`native_call_depth == 0`）下展开——此时 try_stack 只含当前
    /// 字节码的处理器，展开后 pc/regs[0] 不会被中途的原生调用栈覆盖。原生 builtin
    /// 内部（depth > 0）必须传播错误，由其调用边界（dispatch_native_call）转换。
    pub(crate) fn raise_call_error(&mut self, err: &str) -> Result<JsValue, String> {
        if self.native_call_depth == 0 {
            let exc = self
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, err));
            let kind = self.thrown_error_kind(exc);
            self.exception_value = Some(exc);
            self.pending_error_kind = Some(kind);
            self.unwind()?;
            return Ok(JsValue::undefined());
        }
        Err(err.to_string())
    }

    pub(crate) fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String> {
        let primitive = self.coerce_primitive_bounded(value, false)?;
        Ok(coercion::to_number(primitive))
    }

    pub(crate) fn coerce_int32_bounded(&mut self, value: JsValue) -> Result<i32, String> {
        if value.is_int() {
            return Ok(value.as_int());
        }
        let n = self.coerce_number_bounded(value)?;
        if n == 0.0 || !n.is_finite() {
            return Ok(0);
        }
        let int = n.trunc().rem_euclid(4_294_967_296.0) as u32;
        if int > i32::MAX as u32 {
            Ok((int as i64 - 4_294_967_296i64) as i32)
        } else {
            Ok(int as i32)
        }
    }

    pub(crate) fn coerce_uint32_bounded(&mut self, value: JsValue) -> Result<u32, String> {
        if value.is_int() {
            return Ok(value.as_int() as u32);
        }
        let n = self.coerce_number_bounded(value)?;
        if n == 0.0 || !n.is_finite() {
            return Ok(0);
        }
        Ok(n.trunc().rem_euclid(4_294_967_296.0) as u32)
    }

    pub(crate) fn checked_object_ptr(
        &mut self, val: JsValue, error_msg: &str,
    ) -> Result<Option<*mut JsObject>, String> {
        if !val.is_object() {
            self.raise_type_error(error_msg)?;
            return Ok(None);
        }
        let ptr = (val.to_bits() & PTR_MASK) as *mut JsObject;
        let addr = ptr as usize;
        if ptr.is_null() || addr < 0x10000 || addr % std::mem::align_of::<JsObject>() != 0 {
            self.raise_type_error(error_msg)?;
            return Ok(None);
        }
        Ok(Some(ptr))
    }

    pub(crate) fn raise_error_kind(&mut self, kind: &'static str, msg: &str) -> Result<(), String> {
        self.raise_js_error(JsError::new(js_error_kind(kind), msg))
    }

    pub(crate) fn raise_js_error(&mut self, err: JsError) -> Result<(), String> {
        let kind = js_error_kind_name(err.kind);
        vm_debug!("raise_js_error: {} \"{}\"", kind, err.message);
        let error = oxide_builtins::error::create_kind_error(self, kind, &err.message);
        self.exception_value = Some(error);
        self.pending_error_kind = Some(kind);
        self.unwind()
    }

    pub(crate) fn raise_type_error(&mut self, msg: &str) -> Result<(), String> {
        self.raise_error_kind("TypeError", msg)
    }

    pub(crate) fn error_message_text(&self, kind: &str, msg: &str) -> String {
        format_error_message(kind, msg)
    }

    pub(crate) fn thrown_error_kind(&self, val: JsValue) -> &'static str {
        if !val.is_object() {
            return "Error";
        }
        let name_si = self.kernel_core.perm_interner().intern("name").0;
        let obj = unsafe { &*val.as_js_object_ptr() };
        let Some(name_val) = self.resolve_property(obj, name_si) else {
            return "Error";
        };
        let Some(name) = self.lookup_str(name_val) else {
            return "Error";
        };
        match name.as_str() {
            "TypeError" => "TypeError",
            "ReferenceError" => "ReferenceError",
            "RangeError" => "RangeError",
            "SyntaxError" => "SyntaxError",
            "URIError" => "URIError",
            "EvalError" => "EvalError",
            "Error" => "Error",
            _ => "Error",
        }
    }
}
