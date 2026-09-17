//! 错误抛出与有界强转：强转失败统一出口、调用错误恢复、错误构造与异常
//! 展开、有界数值强转三件套与抛出错误 kind 查询。

use oxide_runtime_api as coercion;

use oxide_types::error::JsError;
use oxide_types::object::JsObject;
use oxide_types::private_key::make_well_known_symbol_key;
use oxide_types::value::{JsValue, PTR_MASK};

use super::{format_error_message, js_error_kind, js_error_kind_name, Vm};
use crate::vm_debug;

/// `Symbol.toPrimitive` 的 well-known symbol 下标。
const TO_PRIMITIVE_SYMBOL_ID: u32 = 5;

impl Vm {
    /// ToPrimitive 有界版：按规范序把对象转为原始值，失败统一抛 `TypeError`。
    ///
    /// 对象上（沿原型链读取）的 `Symbol.toPrimitive` 优先；不存在时按 `prefer_string` 定序尝试
    /// `toString`/`valueOf`，等价于 OrdinaryToPrimitive。
    ///
    /// # 步骤
    /// 1. 非对象或空指针原样返回。
    /// 2. 读 `@@toPrimitive`：存在则必须可调用（否则抛 `TypeError`），以
    ///    `"string"`/`"number"` hint 调用；返回对象再抛 `TypeError`。
    /// 3. 回退方法族：按 hint 顺序试 `toString`/`valueOf`，不可调用者跳过，
    ///    首个返回非对象的结果即采用。
    /// 4. 两条路径都不产出原始值时抛 `TypeError`。
    ///
    /// # 边界与前提
    /// - `prefer_string` 为 `true` 时先试 `toString`，否则先试 `valueOf`。
    /// - 方法调用经 `call_function_sync`；调用错误转 `raise_call_error` 恢复
    ///   原始异常。
    ///
    /// # 副作用
    /// - 执行用户代码（getter / 方法调用），可能抛异常并触发 GC 安全点。
    ///
    /// # 注意事项
    /// - 主 dispatch 下就地展开异常；原生 builtin 内部只返回格式化 `Err`，
    ///   由其调用边界恢复（见 `conversion_error`）。
    pub(crate) fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String> {
        if !value.is_object() {
            return Ok(value);
        }

        let obj_ptr = value.as_js_object_ptr();
        if obj_ptr.is_null() {
            return Ok(value);
        }

        // ECMA-262 §7.1.1 step 1：exotic 对象上（沿原型链读取）的 Symbol.toPrimitive 优先于
        // OrdinaryToPrimitive；well-known symbol 键恒定，直接取保留槽。
        let sym_si = make_well_known_symbol_key(TO_PRIMITIVE_SYMBOL_ID);
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

    /// ToNumber 有界版：先按 number hint 做 ToPrimitive，再转 `f64`。
    pub(crate) fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String> {
        let primitive = self.coerce_primitive_bounded(value, false)?;
        Ok(coercion::to_number(primitive))
    }

    /// ToInt32 有界版：整数直通；其余先 ToNumber，再取模 2^32 后按符号回卷。
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

    /// ToUint32 有界版：整数直通；其余先 ToNumber，再取模 2^32。
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

    /// 校验 `JsValue` 为合法非空对象指针，成功时返回对象裸指针。
    ///
    /// # 边界与前提
    /// - 非对象、空指针、地址低于 `0x10000` 或未按 `JsObject` 对齐：抛
    ///   `TypeError`（消息由调用方给出）并返回 `Ok(None)`，调用方据此短路
    ///   （错误已进异常通道，后续由 dispatch 继续展开）。
    /// - 本函数不做语义检查（可调用/可构造等能力由调用方判定）。
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

    /// 按 kind 名构造 `JsError` 并转 `raise_js_error` 抛出。
    pub(crate) fn raise_error_kind(&mut self, kind: &'static str, msg: &str) -> Result<(), String> {
        self.raise_js_error(JsError::new(js_error_kind(kind), msg))
    }

    /// 构造错误对象、写入异常通道并展开到最近的 try 处理器。
    ///
    /// - `exception_value` 存错误对象、`pending_error_kind` 存 kind 名，供外围
    ///   try/catch 与错误通道查询。
    /// - 展开失败（无 handler）时 `unwind` 返回 `Err`，异常逃出 run 边界经
    ///   `last_uncaught_value` 上交调用方。
    pub(crate) fn raise_js_error(&mut self, err: JsError) -> Result<(), String> {
        let kind = js_error_kind_name(err.kind);
        vm_debug!("raise_js_error: {} \"{}\"", kind, err.message);
        let error = oxide_builtins::error::create_kind_error(self, kind, &err.message);
        self.exception_value = Some(error);
        self.pending_error_kind = Some(kind);
        self.unwind()
    }

    /// 抛 `TypeError` 的便捷入口，等价于以 kind 名 `"TypeError"` 调
    /// `raise_error_kind`。
    pub(crate) fn raise_type_error(&mut self, msg: &str) -> Result<(), String> {
        self.raise_error_kind("TypeError", msg)
    }

    /// 格式化错误文本，供原生 builtin 调用边界恢复异常时使用。
    pub(crate) fn error_message_text(&self, kind: &str, msg: &str) -> String {
        format_error_message(kind, msg)
    }

    /// 从异常对象读 `name` 属性并映射为错误 kind 名。
    ///
    /// 非对象、无 `name`、name 非字符串或未知名称一律回退 `"Error"`。
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
