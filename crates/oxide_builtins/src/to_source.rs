//! toSource（SpiderMonkey 遗留扩展）：Object / Function / Array / String / Boolean /
//! Number / Date / RegExp / Error 九原型各一个 own 方法。
//!
//! 关键约定：
//! - 门禁统一走 CheckObjectCoercible 口径——this 为 null / undefined 抛 TypeError，
//!   其余原始值与原对象放行；
//! - 返回值取最小合理源码串（不追求 SpiderMonkey 完整序列化）：Function 空串、
//!   Object / Array 恒形、String 带引号、Boolean / Number 字面量、Date / Error 构造形、
//!   RegExp 斜杠形；
//! - 本模块不提供 test262 度量面，验收靠单测钉 own 存在性与门禁抛错。

use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::value::JsValue;

/// 九原型 toSource 共享门禁：this 为 null / undefined 抛 TypeError（CheckObjectCoercible
/// 口径），其余原样返回 this 值供各臂消费。
fn to_source_check_this<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<JsValue, JsValue> {
    let this_val = vm.reg(args[0]);
    if this_val.is_null() || this_val.is_undefined() {
        return Err(crate::error::create_type_error(vm, "toSource called on null or undefined"));
    }
    Ok(this_val)
}

/// `Object.prototype.toSource`：门禁后恒返 `"{}"`。
pub fn to_source_object<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if let Err(err) = to_source_check_this(vm, args) {
        return NativeResult::Err(err);
    }
    NativeResult::Ok(vm.new_string_owned("{}".to_string()))
}

/// `Function.prototype.toSource`：引擎无函数源文本，门禁后恒返空串（不抛错）。
pub fn to_source_function<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if let Err(err) = to_source_check_this(vm, args) {
        return NativeResult::Err(err);
    }
    NativeResult::Ok(vm.new_string_owned(String::new()))
}

/// `Array.prototype.toSource`：门禁后恒返 `"[]"`。
pub fn to_source_array<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if let Err(err) = to_source_check_this(vm, args) {
        return NativeResult::Err(err);
    }
    NativeResult::Ok(vm.new_string_owned("[]".to_string()))
}

/// `String.prototype.toSource`：门禁后读 this 串值，双引号包裹返回。
///
/// # 边界与前提
/// - this 为字符串时直读单元文本；非字符串走完整 ToString（对象经 ToPrimitive，
///   用户转换异常原样传播）。
pub fn to_source_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = match to_source_check_this(vm, args) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let text = if this_val.is_string() {
        vm.lookup_str(this_val).unwrap_or_default()
    } else {
        match oxide_runtime_api::to_string_full(this_val, vm) {
            Ok(s) => s,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        }
    };
    NativeResult::Ok(vm.new_string_owned(format!("\"{}\"", text)))
}

/// `Boolean.prototype.toSource`：门禁后按 ToBoolean 判位，返 `"true"` / `"false"`。
pub fn to_source_boolean<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = match to_source_check_this(vm, args) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let b = oxide_runtime_api::to_boolean(this_val);
    NativeResult::Ok(vm.new_string_owned(if b { "true".to_string() } else { "false".to_string() }))
}

/// `Number.prototype.toSource`：门禁后按 ToNumber 取 double，十进制字面量返回。
///
/// # 边界与前提
/// - this 为 double 时直读；非 double 走完整 ToNumber（用户转换异常原样传播）。
pub fn to_source_number<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = match to_source_check_this(vm, args) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let d = if this_val.is_double() {
        this_val.as_double()
    } else {
        match oxide_runtime_api::to_number_full(this_val, vm) {
            Ok(n) => n,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        }
    };
    NativeResult::Ok(vm.number_to_string_cached(d))
}

/// `Date.prototype.toSource`：门禁后读时间戳（prop 0），返 `new Date(<ms>)` 形；
/// 非 Date 对象或时间戳缺失时回退 `new Date(NaN)`。
pub fn to_source_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = match to_source_check_this(vm, args) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let ms = if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() && unsafe { &*ptr }.is_date_obj() {
            let v = unsafe { &*ptr }.get_prop_at(0);
            if v.is_double() {
                v.as_double()
            } else {
                f64::NAN
            }
        } else {
            f64::NAN
        }
    } else {
        f64::NAN
    };
    let num_text = oxide_runtime_api::js_number_to_string(ms);
    NativeResult::Ok(vm.new_string_owned(format!("new Date({})", num_text)))
}

/// `RegExp.prototype.toSource`：门禁后读 `[[OriginalSource]]` / `[[OriginalFlags]]` 槽，
/// 返 `/source/flags` 形；非 RegExp 对象回退 `/(?:)/`。
pub fn to_source_regexp<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = match to_source_check_this(vm, args) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    if !this_val.is_object() {
        return NativeResult::Ok(vm.new_string_owned("/(?:)/".to_string()));
    }
    let ptr = this_val.as_js_object_ptr();
    if ptr.is_null() || !unsafe { &*ptr }.is_regexp_obj() {
        return NativeResult::Ok(vm.new_string_owned("/(?:)/".to_string()));
    }
    let re = unsafe { &*ptr };
    let source = vm.lookup_str(re.get_regexp_source()).unwrap_or_default();
    let flags = vm.lookup_str(re.get_regexp_flags()).unwrap_or_default();
    NativeResult::Ok(vm.new_string_owned(format!("/{}/{}", source, flags)))
}

/// `Error.prototype.toSource`：门禁后读 `message` 属性，返 `new Error("message")` 形；
/// 无 message 时 message 段为空串。
///
/// # 边界与前提
/// - message 经原型链 Get 读取（访问器触发，用户异常吞掉回落空串）；
/// - message 非 undefined 时走完整 ToString（Symbol 等不可转换场景回落空串）。
pub fn to_source_error<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = match to_source_check_this(vm, args) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };
    let mut msg = String::new();
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            let si_msg = vm.perm_intern("message");
            if let Ok(v) = vm.ordinary_get(obj, si_msg, this_val) {
                if !v.is_undefined() {
                    if let Ok(s) = oxide_runtime_api::to_string_full(v, vm) {
                        msg = s;
                    }
                }
            }
        }
    }
    NativeResult::Ok(vm.new_string_owned(format!("new Error({:?})", msg)))
}
