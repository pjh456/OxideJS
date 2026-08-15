use std::sync::Arc;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_string, NativeResult, VmHost};
use oxide_types::mem::P;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

fn set_own_message<H: VmHost>(host: &mut H, this: *mut JsObject, args: &[u8]) {
    if args.len() <= 1 {
        return;
    }
    let msg_val = host.reg(args[1]);
    if msg_val.is_undefined() {
        return;
    }
    let msg_str = to_string(msg_val);
    let sf = Arc::clone(host.kernel_core().perm_interner());
    let sh = Arc::clone(host.kernel_core().shape_forge());
    let si = sf.intern("message").0;
    let new_shape = sh.make_shape(EMPTY_SHAPE_ID, si);
    let perm_val = host.new_string(&msg_str);
    unsafe {
        (*this).set_shape_id(new_shape);
        let pos = (*this).push_prop(perm_val);
        // message 按规范为非枚举数据属性（CreateNonEnumerableDataPropertyOrThrow）。
        (*this).set_data_meta(pos, PropAttributes::new(true, false, true));
    }
}

/// 按错误类型名创建对应 Error 对象（message 非空时写为自身属性）。
/// 这是引擎内部构造错误的统一入口，供 VM/native 层抛错使用。
pub fn create_kind_error<H: VmHost>(host: &mut H, kind: &str, msg: &str) -> JsValue {
    let proto_ptr = match kind {
        "TypeError" => P::as_ptr(&host.session().builtin_world().type_error_proto) as *mut JsObject,
        "RangeError" => P::as_ptr(&host.session().builtin_world().range_error_proto) as *mut JsObject,
        "ReferenceError" => P::as_ptr(&host.session().builtin_world().reference_error_proto) as *mut JsObject,
        "SyntaxError" => P::as_ptr(&host.session().builtin_world().syntax_error_proto) as *mut JsObject,
        "URIError" => P::as_ptr(&host.session().builtin_world().uri_error_proto) as *mut JsObject,
        "EvalError" => P::as_ptr(&host.session().builtin_world().eval_error_proto) as *mut JsObject,
        _ => P::as_ptr(&host.session().builtin_world().error_proto) as *mut JsObject,
    };
    let obj = host
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
    let sf = Arc::clone(host.kernel_core().perm_interner());
    let sh = Arc::clone(host.kernel_core().shape_forge());
    if !msg.is_empty() {
        let si_msg = sf.intern("message").0;
        let shape = sh.make_shape(EMPTY_SHAPE_ID, si_msg);
        let msg_val = host.new_string(msg);
        unsafe {
            (*obj).set_shape_id(shape);
            let pos = (*obj).push_prop(msg_val);
            // message 按规范为非枚举数据属性，避免泄漏进 Object.keys/for-in/JSON。
            (*obj).set_data_meta(pos, PropAttributes::new(true, false, true));
        }
    }
    // 标记 Error 家族标签：Object.prototype.toString 据此输出 `[object Error]`。
    unsafe {
        (*obj).type_tag = JsObject::OBJ_TYPE_ERROR;
    }
    JsValue::from_js_object(obj)
}

/// 创建一个带指定 message 的 TypeError 对象。
pub fn create_type_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "TypeError", msg)
}

/// 创建一个带指定 message 的普通 Error 对象。
pub fn create_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "Error", msg)
}

/// 从错误文本（形如 `TypeError: msg` / `ReferenceError: msg`）解析类型前缀并创建
/// 对应 kind 的错误对象；无前缀时创建普通 Error。
///
/// # 使用场景
/// 异常传播链中错误对象被降级为文本（error_text 输出、`last_uncaught_value` 被
/// 嵌套覆盖后的兜底路径）时，据此恢复错误类型，避免全部塌缩成普通 Error。
pub fn create_from_text<H: VmHost>(host: &mut H, text: &str) -> JsValue {
    let text = text.strip_prefix("uncaught ").unwrap_or(text);
    for (kind, prefix) in [
        ("TypeError", "TypeError: "),
        ("ReferenceError", "ReferenceError: "),
        ("RangeError", "RangeError: "),
        ("SyntaxError", "SyntaxError: "),
        ("URIError", "URIError: "),
        ("EvalError", "EvalError: "),
    ] {
        if let Some(msg) = text.strip_prefix(prefix) {
            return create_kind_error(host, kind, msg);
        }
    }
    if let Some(msg) = text.strip_prefix("Error: ") {
        return create_kind_error(host, "Error", msg);
    }
    create_error(host, text)
}

/// 创建一个带指定 message 的 ReferenceError 对象。
pub fn create_reference_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "ReferenceError", msg)
}

/// 创建一个带指定 message 的 RangeError 对象。
pub fn create_range_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "RangeError", msg)
}

/// 创建一个带指定 message 的 SyntaxError 对象。
pub fn create_syntax_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "SyntaxError", msg)
}

/// 创建一个带指定 message 的 URIError 对象。
pub fn create_uri_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "URIError", msg)
}

macro_rules! error_ctor {
    ($name:ident, $proto_field:ident) => {
        /// 对应 Error 子类（如 `TypeError`）的构造函数：接收第一个实参作为 message，
        /// 返回原型链指向对应 prototype 的 Error 对象。
        ///
        /// # 注意事项
        /// 无论以 `new` 还是普通函数调用（如 `TypeError.call(obj)`）都新建对象，
        /// 忽略调用方传入的 this——与规范构造器语义一致。
        pub fn $name<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
            let proto_ptr = P::as_ptr(&host.session().builtin_world().$proto_field) as *mut JsObject;
            let this = host
                .epoch()
                .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr)));
            set_own_message(host, this, args);
            NativeResult::Ok(JsValue::from_js_object(this))
        }
    };
}

error_ctor!(error_constructor, error_proto);
error_ctor!(type_error_constructor, type_error_proto);
error_ctor!(reference_error_constructor, reference_error_proto);
error_ctor!(range_error_constructor, range_error_proto);
error_ctor!(syntax_error_constructor, syntax_error_proto);
error_ctor!(uri_error_constructor, uri_error_proto);
error_ctor!(eval_error_constructor, eval_error_proto);

/// `Error.prototype.toString`：按 `name: message` 拼接字符串；
/// 缺少 name/message 时按规范回退到 `"Error"` 或空串。
pub fn error_to_string<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let this_val = host.reg(args[0]);
    if !this_val.is_object() {
        let err = create_type_error(host, "Error.prototype.toString called on non-object");
        return NativeResult::Err(err);
    }
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    let sf = Arc::clone(host.kernel_core().perm_interner());
    let si_name = sf.intern("name").0;
    let si_msg = sf.intern("message").0;

    let name_str = match host.resolve_property(obj, si_name) {
        Some(v) if !v.is_undefined() => match host.coerce_primitive_bounded(v, true) {
            Ok(prim) => to_string(prim),
            Err(_) => {
                let err = create_type_error(host, "Cannot convert name to primitive value");
                return NativeResult::Err(err);
            }
        },
        _ => "Error".to_string(),
    };

    let msg_str = match host.resolve_property(obj, si_msg) {
        Some(v) if !v.is_undefined() => match host.coerce_primitive_bounded(v, true) {
            Ok(prim) => to_string(prim),
            Err(_) => {
                let err = create_type_error(host, "Cannot convert message to primitive value");
                return NativeResult::Err(err);
            }
        },
        _ => String::new(),
    };

    let result = if name_str.is_empty() {
        msg_str
    } else if msg_str.is_empty() {
        name_str
    } else {
        format!("{}: {}", name_str, msg_str)
    };
    NativeResult::Ok(host.new_string(&result))
}

/// `Error.prototype.stack` getter：输出 `name: message` 头后附调用栈函数名列表。
pub fn error_stack_getter<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let this_val = host.reg(args[0]);
    let (name_str, msg_str) = if this_val.is_object() {
        let obj = unsafe { &*this_val.as_js_object_ptr() };
        let sf = Arc::clone(host.kernel_core().perm_interner());
        let si_name = sf.intern("name").0;
        let si_msg = sf.intern("message").0;
        let n = host
            .resolve_property(obj, si_name)
            .and_then(|v| host.lookup_str(v))
            .unwrap_or_else(|| "Error".to_string());
        let m = host
            .resolve_property(obj, si_msg)
            .and_then(|v| host.lookup_str(v))
            .unwrap_or_default();
        (n, m)
    } else {
        ("Error".to_string(), String::new())
    };

    let header = oxide_runtime_api::format_error_message(&name_str, &msg_str);
    let mut result = header;
    let names = host.call_stack_function_names();
    for n in &names {
        result.push_str(&format!("\n    at {} (<unknown>:0:0)", n));
    }
    NativeResult::Ok(host.new_string(&result))
}
