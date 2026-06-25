use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

pub fn boolean_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let bool_val = if args.len() > 1 {
        let arg = vm.reg(args[1]);
        if arg.is_undefined() || arg.is_null() {
            false
        } else if arg.is_bool() {
            arg.as_bool()
        } else if arg.is_string() {
            // SAFETY: arg is a string value.
            !unsafe { (*arg.as_string_ptr()).is_empty() }
        } else if arg.is_object() {
            true
        } else if arg.is_int() {
            arg.as_int() != 0
        } else if arg.is_double() {
            arg.as_double() != 0.0
        } else {
            false
        }
    } else {
        false
    };

    let boolean_proto = vm.session().builtin_world().boolean_proto.as_ptr() as *mut JsObject;
    let is_ctor = if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if ptr.is_null() {
            false
        } else {
            let proto_ptr = unsafe { (*ptr).proto().as_js_object_ptr() };
            !proto_ptr.is_null() && std::ptr::eq(proto_ptr, boolean_proto)
        }
    } else {
        false
    };

    if !is_ctor {
        return NativeResult::Ok(JsValue::bool(bool_val));
    }

    let obj = unsafe { &mut *this_val.as_js_object_ptr() };
    obj.type_tag = JsObject::OBJ_TYPE_BOOLEAN_OBJ;
    obj.push_prop(JsValue::bool(bool_val));
    NativeResult::Ok(this_val)
}

pub fn boolean_prototype_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if this_val.is_bool() {
        return NativeResult::Ok(this_val);
    }
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Boolean.prototype.valueOf called on non-Boolean object",
        ));
    }
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    if !obj.is_boolean_obj() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Boolean.prototype.valueOf called on non-Boolean object",
        ));
    }
    let val = obj.get_prop_at(0);
    NativeResult::Ok(val)
}

pub fn boolean_prototype_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if this_val.is_bool() {
        return if this_val.as_bool() {
            NativeResult::Ok(vm.new_string("true"))
        } else {
            NativeResult::Ok(vm.new_string("false"))
        };
    }
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Boolean.prototype.toString called on non-Boolean object",
        ));
    }
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    if !obj.is_boolean_obj() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Boolean.prototype.toString called on non-Boolean object",
        ));
    }
    let val = obj.get_prop_at(0);
    let is_true = if val.is_bool() {
        val.as_bool()
    } else {
        !val.is_null() && !val.is_undefined()
    };
    if is_true {
        NativeResult::Ok(vm.new_string("true"))
    } else {
        NativeResult::Ok(vm.new_string("false"))
    }
}
