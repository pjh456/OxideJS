use std::sync::Arc;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_string, NativeResult, VmHost};
use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

fn create_error_object<H: VmHost>(host: &mut H, error_proto_ptr: *mut JsObject, name: &str, message: &str) -> JsValue {
    let obj = host
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(error_proto_ptr)));

    let sf = Arc::clone(host.kernel_core().perm_interner());
    let sh = Arc::clone(host.kernel_core().shape_forge());

    let si_message = sf.intern("message").0;
    let sh_message = sh.make_shape(EMPTY_SHAPE_ID, si_message);
    let msg_val = host.new_string(message);
    unsafe {
        (*obj).set_shape_id(sh_message);
        (*obj).push_prop(msg_val);
    }

    let si_name = sf.intern("name").0;
    let sh_name = sh.make_shape(sh_message, si_name);
    let name_val = host.new_string(name);
    unsafe {
        (*obj).set_shape_id(sh_name);
        (*obj).push_prop(name_val);
    }

    JsValue::from_js_object(obj)
}

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
    create_error_object(host, proto_ptr, kind, msg)
}

pub fn create_type_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "TypeError", msg)
}

pub fn create_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "Error", msg)
}

pub fn create_reference_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "ReferenceError", msg)
}

pub fn create_range_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "RangeError", msg)
}

pub fn create_syntax_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "SyntaxError", msg)
}

pub fn create_uri_error<H: VmHost>(host: &mut H, msg: &str) -> JsValue {
    create_kind_error(host, "URIError", msg)
}

fn get_msg<H: VmHost>(host: &mut H, args: &[u8]) -> String {
    if args.len() > 1 {
        to_string(host.reg(args[1]))
    } else {
        String::new()
    }
}

pub fn error_constructor<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let msg = get_msg(host, args);
    let proto_ptr = P::as_ptr(&host.session().builtin_world().error_proto) as *mut JsObject;
    NativeResult::Ok(create_error_object(host, proto_ptr, "Error", &msg))
}

pub fn type_error_constructor<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let msg = get_msg(host, args);
    let proto_ptr = P::as_ptr(&host.session().builtin_world().type_error_proto) as *mut JsObject;
    NativeResult::Ok(create_error_object(host, proto_ptr, "TypeError", &msg))
}

pub fn reference_error_constructor<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let msg = get_msg(host, args);
    let proto_ptr = P::as_ptr(&host.session().builtin_world().reference_error_proto) as *mut JsObject;
    NativeResult::Ok(create_error_object(host, proto_ptr, "ReferenceError", &msg))
}

pub fn range_error_constructor<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let msg = get_msg(host, args);
    let proto_ptr = P::as_ptr(&host.session().builtin_world().range_error_proto) as *mut JsObject;
    NativeResult::Ok(create_error_object(host, proto_ptr, "RangeError", &msg))
}

pub fn syntax_error_constructor<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let msg = get_msg(host, args);
    let proto_ptr = P::as_ptr(&host.session().builtin_world().syntax_error_proto) as *mut JsObject;
    NativeResult::Ok(create_error_object(host, proto_ptr, "SyntaxError", &msg))
}

pub fn uri_error_constructor<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let msg = get_msg(host, args);
    let proto_ptr = P::as_ptr(&host.session().builtin_world().uri_error_proto) as *mut JsObject;
    NativeResult::Ok(create_error_object(host, proto_ptr, "URIError", &msg))
}

pub fn eval_error_constructor<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let msg = get_msg(host, args);
    let proto_ptr = P::as_ptr(&host.session().builtin_world().eval_error_proto) as *mut JsObject;
    NativeResult::Ok(create_error_object(host, proto_ptr, "EvalError", &msg))
}

pub fn error_to_string<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult {
    let this_val = host.reg(args[0]);
    let (name, msg) = if this_val.is_object() {
        let obj = unsafe { &*this_val.as_js_object_ptr() };
        let name_val = obj
            .hash_props_vec()
            .and_then(|v| v.first())
            .copied()
            .unwrap_or(JsValue::undefined());
        let msg_val = obj
            .hash_props_vec()
            .and_then(|v| v.get(1))
            .copied()
            .unwrap_or(JsValue::undefined());
        (
            host.lookup_str(name_val).unwrap_or_else(|| "Error".to_string()),
            host.lookup_str(msg_val).unwrap_or_default(),
        )
    } else {
        ("Error".to_string(), String::new())
    };
    let result = oxide_runtime_api::format_error_message(&name, &msg);
    NativeResult::Ok(host.new_string(&result))
}

pub fn error_stack_getter<H: VmHost>(host: &mut H, _args: &[u8]) -> NativeResult {
    let names = host.call_stack_function_names();
    let mut lines: Vec<String> = names.iter().rev().map(|n| format!("    at {} (native)", n)).collect();
    lines.push("    at <anonymous> (native)".to_string());
    let result = lines.join("\n");
    NativeResult::Ok(host.new_string(&result))
}
