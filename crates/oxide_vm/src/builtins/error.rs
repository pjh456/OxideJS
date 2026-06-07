use std::sync::Arc;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;

fn create_error_object(
    vm: &mut Vm,
    error_proto_ptr: *mut JsObject,
    name: &str,
    message: &str,
) -> JsValue {
    let obj = vm.epoch().alloc(JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(error_proto_ptr),
    ));

    let sf = Arc::clone(vm.kernel().string_forge());
    let sh = Arc::clone(vm.kernel().shape_forge());

    let si_message = sf.intern("message").0;
    let sh_message = sh.make_shape(EMPTY_SHAPE_ID, si_message);
    let msg_val = vm.intern(message);
    unsafe {
        (*obj).set_shape_id(sh_message);
        (*obj).push_prop(msg_val);
    }

    let si_name = sf.intern("name").0;
    let sh_name = sh.make_shape(sh_message, si_name);
    let name_val = vm.intern(name);
    unsafe {
        (*obj).set_shape_id(sh_name);
        (*obj).push_prop(name_val);
    }

    JsValue::from_js_object(obj)
}

pub fn create_type_error(vm: &mut Vm, msg: &str) -> JsValue {
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().type_error_proto) as *mut JsObject;
    create_error_object(vm, proto_ptr, "TypeError", msg)
}

pub fn create_reference_error(vm: &mut Vm, msg: &str) -> JsValue {
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().reference_error_proto) as *mut JsObject;
    create_error_object(vm, proto_ptr, "ReferenceError", msg)
}

pub fn create_range_error(vm: &mut Vm, msg: &str) -> JsValue {
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().range_error_proto) as *mut JsObject;
    create_error_object(vm, proto_ptr, "RangeError", msg)
}

pub fn create_syntax_error(vm: &mut Vm, msg: &str) -> JsValue {
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().syntax_error_proto) as *mut JsObject;
    create_error_object(vm, proto_ptr, "SyntaxError", msg)
}

fn get_msg(vm: &mut Vm, args: &[u8]) -> String {
    if args.len() > 1 {
        coercion::to_string(vm.kernel().string_forge().as_ref(), vm.reg(args[1]))
    } else {
        String::new()
    }
}

pub fn error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg = get_msg(vm, args);
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().error_proto) as *mut JsObject;
    Ok(create_error_object(vm, proto_ptr, "Error", &msg))
}

pub fn type_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg = get_msg(vm, args);
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().type_error_proto) as *mut JsObject;
    Ok(create_error_object(vm, proto_ptr, "TypeError", &msg))
}

pub fn reference_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg = get_msg(vm, args);
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().reference_error_proto) as *mut JsObject;
    Ok(create_error_object(vm, proto_ptr, "ReferenceError", &msg))
}

pub fn range_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg = get_msg(vm, args);
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().range_error_proto) as *mut JsObject;
    Ok(create_error_object(vm, proto_ptr, "RangeError", &msg))
}

pub fn syntax_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg = get_msg(vm, args);
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().syntax_error_proto) as *mut JsObject;
    Ok(create_error_object(vm, proto_ptr, "SyntaxError", &msg))
}

pub fn uri_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg = get_msg(vm, args);
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().uri_error_proto) as *mut JsObject;
    Ok(create_error_object(vm, proto_ptr, "URIError", &msg))
}

pub fn eval_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg = get_msg(vm, args);
    let proto_ptr = P::as_ptr(&vm.kernel().builtin_world().eval_error_proto) as *mut JsObject;
    Ok(create_error_object(vm, proto_ptr, "EvalError", &msg))
}
