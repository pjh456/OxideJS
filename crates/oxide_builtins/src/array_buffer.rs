use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

pub(crate) const MAX_ARRAY_BUFFER_LENGTH: usize = 1 << 30;

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

fn type_error<H: VmHost>(vm: &mut H, msg: &str) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, msg))
}

fn to_index<H: VmHost>(vm: &mut H, value: JsValue) -> Result<usize, JsValue> {
    let n = vm.coerce_number_bounded(value).unwrap_or(f64::NAN);
    if n.is_nan() {
        return Ok(0);
    }
    if !n.is_finite() || n < 0.0 || n > MAX_ARRAY_BUFFER_LENGTH as f64 {
        return Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    Ok(n.trunc() as usize)
}

fn normalize_index<H: VmHost>(vm: &mut H, value: JsValue, len: usize) -> usize {
    let n = vm.coerce_number_bounded(value).unwrap_or(f64::NAN);
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

fn set_named_prop<H: VmHost>(vm: &mut H, obj: &mut JsObject, name: &str, value: JsValue, attributes: PropAttributes) {
    let si = vm.kernel_core().perm_interner().intern(name).0;
    let _ = vm.define_data_property(obj, si, value, attributes);
}

pub(crate) fn new_array_buffer<H: VmHost>(vm: &mut H, data: Vec<u8>) -> *mut JsObject {
    let proto = vm.session().builtin_world().array_buffer_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
    let len = data.len();
    let data_ptr = Box::into_raw(Box::new(data));
    // SAFETY: ArrayBuffer 对象不可调用，故 native_fn 槽复用作不透明 `Box<Vec<u8>>`
    // 指针，与既有 RegExp 存储模式一致。
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

fn array_buffer_vec_ptr(obj: &JsObject) -> Option<*mut Vec<u8>> {
    if !obj.is_array_buffer_obj() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut Vec<u8>)
}

/// 克隆 ArrayBuffer 的字节数据到新对象（跨 epoch 克隆流程用）。
pub fn clone_array_buffer_native(old_obj: &JsObject, new_obj: &mut JsObject) {
    let Some(ptr) = array_buffer_vec_ptr(old_obj) else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    let cloned = unsafe { (&*ptr).clone() };
    let cloned_ptr = Box::into_raw(Box::new(cloned));
    new_obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(cloned_ptr as *const ()) }));
}

/// 只读核算 ArrayBuffer 字节缓冲字节（不释放）。
pub fn array_buffer_native_size(obj: &JsObject) -> u64 {
    let Some(data_ptr) = array_buffer_vec_ptr(obj) else {
        return 0;
    };
    if data_ptr.is_null() {
        return 0;
    }
    unsafe { (std::mem::size_of::<Vec<u8>>() + (*data_ptr).capacity()) as u64 }
}

/// 释放 ArrayBuffer 的字节缓冲（native_fn 槽中的 `Box<Vec<u8>>`），返回字节数。
pub fn drop_array_buffer_native(obj: &mut JsObject) -> u64 {
    let bytes = array_buffer_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(data_ptr) = array_buffer_vec_ptr(obj) else {
        return 0;
    };
    // SAFETY: data_ptr 非空（array_buffer_native_size 已验证），Box::from_raw 恰好释放一次。
    let data = unsafe { Box::from_raw(data_ptr) };
    drop(data);
    obj.set_native_fn(None);
    bytes
}

pub(crate) fn array_buffer_data_ptr<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<*mut Vec<u8>, JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer method called on incompatible receiver"));
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    }
    let obj = unsafe { &*obj_ptr };
    if !obj.is_array_buffer_obj() {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer method called on incompatible receiver"));
    }
    let Some(data_ptr) = obj.native_fn() else {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let data_ptr = data_ptr.as_ptr() as *mut Vec<u8>;
    if data_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    }
    Ok(data_ptr)
}

/// `ArrayBuffer(length)` 构造逻辑：分配 length 字节（0 填充）的新缓冲区。
pub fn array_buffer_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
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

/// `ArrayBuffer.prototype.byteLength` getter：返回缓冲区字节数。
pub fn array_buffer_byte_length<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let data_ptr = native_try!(array_buffer_data_ptr(vm, this_val));
    NativeResult::Ok(JsValue::int(unsafe { (*data_ptr).len() } as i32))
}

/// `ArrayBuffer.prototype.slice(start, end)`：复制字节区间生成新 ArrayBuffer。
pub fn array_buffer_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let data_ptr = native_try!(array_buffer_data_ptr(vm, this_val));
    let data = unsafe { &*data_ptr };
    let len = data.len();
    let start = if args.len() > 1 { normalize_index(vm, vm.reg(args[1]), len) } else { 0 };
    let end = if args.len() > 2 { normalize_index(vm, vm.reg(args[2]), len) } else { len };
    let end = end.max(start);
    NativeResult::Ok(JsValue::from_js_object(new_array_buffer(vm, data[start..end].to_vec())))
}

/// `ArrayBuffer.isView(value)`：参数是 DataView 或 TypedArray 才返回 true。
pub fn array_buffer_is_view<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
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

/// `ArrayBuffer.prototype.toString`：校验 receiver 后返回 `[object ArrayBuffer]`。
pub fn array_buffer_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if array_buffer_data_ptr(vm, this_val).is_err() {
        return type_error(vm, "ArrayBuffer.prototype.toString called on incompatible receiver");
    }
    NativeResult::Ok(vm.new_string("[object ArrayBuffer]"))
}
