use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;

pub(crate) const MAX_ARRAY_BUFFER_LENGTH: usize = 1 << 30;

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

fn type_error(vm: &mut Vm, msg: &str) -> NativeResult {
    NativeResult::Err(crate::builtins::error::create_type_error(vm, msg))
}

fn to_index(vm: &mut Vm, value: JsValue) -> Result<usize, JsValue> {
    let n = coercion::to_number(value, vm.kernel_core().string_forge().as_ref());
    if n.is_nan() {
        return Ok(0);
    }
    if !n.is_finite() || n < 0.0 || n > MAX_ARRAY_BUFFER_LENGTH as f64 {
        return Err(crate::builtins::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    Ok(n.trunc() as usize)
}

fn normalize_index(vm: &mut Vm, value: JsValue, len: usize) -> usize {
    let n = coercion::to_number(value, vm.kernel_core().string_forge().as_ref());
    if n.is_nan() {
        return 0;
    }
    let int = n.trunc() as isize;
    if int < 0 {
        len.saturating_sub((-int) as usize)
    } else {
        (int as usize).min(len)
    }
}

fn set_named_prop(vm: &mut Vm, obj: &mut JsObject, name: &str, value: JsValue, attributes: PropAttributes) {
    let si = vm.kernel_core().string_forge().intern(name).0;
    let _ = vm.define_data_property(obj, si, value, attributes);
}

pub(crate) fn new_array_buffer(vm: &mut Vm, data: Vec<u8>) -> *mut JsObject {
    let proto = vm.session().builtin_world().array_buffer_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
    let len = data.len();
    let data_ptr = Box::into_raw(Box::new(data));
    // SAFETY: ArrayBuffer objects are not callable, so their native_fn slot is reused
    // as an opaque Box<Vec<u8>> pointer, matching the existing RegExp storage pattern.
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(data_ptr as *const ()) }));
    set_named_prop(
        vm,
        &mut obj,
        "byteLength",
        JsValue::int(len as i32),
        PropAttributes::new(false, false, false),
    );
    vm.alloc_object(obj)
}

pub(crate) fn drop_array_buffer_native(obj: &mut JsObject) -> u64 {
    if !obj.is_array_buffer_obj() {
        return 0;
    }
    let Some(ptr) = obj.native_fn() else {
        return 0;
    };
    let data_ptr = ptr.as_ptr() as *mut Vec<u8>;
    if data_ptr.is_null() {
        return 0;
    }
    // SAFETY: ArrayBuffer stores `Box<Vec<u8>>` in native_fn.
    let data = unsafe { Box::from_raw(data_ptr) };
    let bytes = std::mem::size_of::<Vec<u8>>() + data.capacity();
    obj.set_native_fn(None);
    bytes as u64
}

pub(crate) fn array_buffer_data_ptr(vm: &mut Vm, this_val: JsValue) -> Result<*mut Vec<u8>, JsValue> {
    if !this_val.is_object() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "ArrayBuffer method called on incompatible receiver",
        ));
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    }
    let obj = unsafe { &*obj_ptr };
    if !obj.is_array_buffer_obj() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "ArrayBuffer method called on incompatible receiver",
        ));
    }
    let Some(data_ptr) = obj.native_fn() else {
        return Err(crate::builtins::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let data_ptr = data_ptr.as_ptr() as *mut Vec<u8>;
    if data_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    }
    Ok(data_ptr)
}

pub fn array_buffer_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let length = if args.len() > 1 {
        match to_index(vm, vm.reg(args[1])) {
            Ok(length) => length,
            Err(err) => return NativeResult::Err(err),
        }
    } else {
        0
    };
    NativeResult::Ok(JsValue::from_js_object(new_array_buffer(vm, vec![0; length])))
}

pub fn array_buffer_byte_length(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let data_ptr = native_try!(array_buffer_data_ptr(vm, this_val));
    NativeResult::Ok(JsValue::int(unsafe { (*data_ptr).len() } as i32))
}

pub fn array_buffer_slice(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let data_ptr = native_try!(array_buffer_data_ptr(vm, this_val));
    let data = unsafe { &*data_ptr };
    let len = data.len();
    let start = if args.len() > 1 { normalize_index(vm, vm.reg(args[1]), len) } else { 0 };
    let end = if args.len() > 2 { normalize_index(vm, vm.reg(args[2]), len) } else { len };
    let end = end.max(start);
    NativeResult::Ok(JsValue::from_js_object(new_array_buffer(vm, data[start..end].to_vec())))
}

pub fn array_buffer_is_view(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !value.is_object() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj = unsafe { &*ptr };
    NativeResult::Ok(JsValue::bool(obj.is_data_view_obj() || obj.is_typed_array_obj()))
}

pub fn array_buffer_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if array_buffer_data_ptr(vm, this_val).is_err() {
        return type_error(vm, "ArrayBuffer.prototype.toString called on incompatible receiver");
    }
    NativeResult::Ok(vm.intern("[object ArrayBuffer]"))
}
