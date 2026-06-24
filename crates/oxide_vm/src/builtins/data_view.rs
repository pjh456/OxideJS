use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::builtins::array_buffer::array_buffer_data_ptr;
use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;

#[derive(Clone, Copy)]
pub(crate) struct DataViewData {
    buffer: JsValue,
    byte_offset: usize,
    byte_length: usize,
}

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

fn to_index(vm: &mut Vm, value: JsValue, msg: &str) -> Result<usize, JsValue> {
    let n = coercion::to_number(value);
    if n.is_nan() {
        return Ok(0);
    }
    if !n.is_finite() || n < 0.0 {
        return Err(crate::builtins::error::create_range_error(vm, msg));
    }
    Ok(n.trunc() as usize)
}

fn is_little_endian(vm: &mut Vm, args: &[u8], idx: usize) -> bool {
    args.get(idx).map(|reg| coercion::to_boolean(vm.reg(*reg))).unwrap_or(false)
}

fn numeric_arg(vm: &mut Vm, args: &[u8], idx: usize) -> f64 {
    args.get(idx).map(|reg| coercion::to_number(vm.reg(*reg))).unwrap_or(0.0)
}

fn set_named_prop(vm: &mut Vm, obj: &mut JsObject, name: &str, value: JsValue, attributes: PropAttributes) {
    let si = vm.kernel_core().perm_interner().intern(name).0;
    let _ = vm.define_data_property(obj, si, value, attributes);
}

fn get_data_view_data(vm: &mut Vm, this_val: JsValue) -> Result<DataViewData, JsValue> {
    if !this_val.is_object() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "DataView method called on incompatible receiver",
        ));
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "DataView internal state invalid"));
    }
    let obj = unsafe { &*obj_ptr };
    if !obj.is_data_view_obj() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "DataView method called on incompatible receiver",
        ));
    }
    let Some(data_ptr) = obj.native_fn() else {
        return Err(crate::builtins::error::create_type_error(vm, "DataView internal state invalid"));
    };
    let data_ptr = data_ptr.as_ptr() as *const DataViewData;
    if data_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "DataView internal state invalid"));
    }
    Ok(unsafe { *data_ptr })
}

fn checked_absolute_offset(vm: &mut Vm, view: DataViewData, offset: usize, width: usize) -> Result<usize, JsValue> {
    let Some(end) = offset.checked_add(width) else {
        return Err(crate::builtins::error::create_range_error(vm, "DataView offset out of bounds"));
    };
    if end > view.byte_length {
        return Err(crate::builtins::error::create_range_error(vm, "DataView offset out of bounds"));
    }
    Ok(view.byte_offset + offset)
}

fn read_bytes<const N: usize>(vm: &mut Vm, args: &[u8]) -> Result<[u8; N], JsValue> {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = get_data_view_data(vm, this_val)?;
    let offset = if args.len() > 1 {
        to_index(vm, vm.reg(args[1]), "DataView offset out of bounds")?
    } else {
        0
    };
    let abs = checked_absolute_offset(vm, view, offset, N)?;
    let buffer_ptr = array_buffer_data_ptr(vm, view.buffer)?;
    let buffer = unsafe { &*buffer_ptr };
    if abs + N > buffer.len() {
        return Err(crate::builtins::error::create_range_error(vm, "DataView offset out of bounds"));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&buffer[abs..abs + N]);
    Ok(out)
}

fn write_bytes<const N: usize>(vm: &mut Vm, args: &[u8], bytes: [u8; N]) -> Result<(), JsValue> {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = get_data_view_data(vm, this_val)?;
    let offset = if args.len() > 1 {
        to_index(vm, vm.reg(args[1]), "DataView offset out of bounds")?
    } else {
        0
    };
    let abs = checked_absolute_offset(vm, view, offset, N)?;
    let buffer_ptr = array_buffer_data_ptr(vm, view.buffer)?;
    let buffer = unsafe { &mut *buffer_ptr };
    if abs + N > buffer.len() {
        return Err(crate::builtins::error::create_range_error(vm, "DataView offset out of bounds"));
    }
    buffer[abs..abs + N].copy_from_slice(&bytes);
    Ok(())
}

pub fn data_view_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::builtins::error::create_type_error(vm, "DataView requires an ArrayBuffer"));
    }
    let buffer = vm.reg(args[1]);
    let buffer_ptr = native_try!(array_buffer_data_ptr(vm, buffer));
    let buffer_len = unsafe { (*buffer_ptr).len() };
    let byte_offset = if args.len() > 2 {
        native_try!(to_index(vm, vm.reg(args[2]), "DataView byteOffset out of bounds"))
    } else {
        0
    };
    if byte_offset > buffer_len {
        return NativeResult::Err(crate::builtins::error::create_range_error(vm, "DataView byteOffset out of bounds"));
    }
    let default_len = buffer_len - byte_offset;
    let byte_length = if args.len() > 3 {
        native_try!(to_index(vm, vm.reg(args[3]), "DataView byteLength out of bounds"))
    } else {
        default_len
    };
    if byte_offset + byte_length > buffer_len {
        return NativeResult::Err(crate::builtins::error::create_range_error(vm, "DataView byteLength out of bounds"));
    }

    let proto = vm.session().builtin_world().data_view_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_DATA_VIEW;
    let data = Box::into_raw(Box::new(DataViewData {
        buffer,
        byte_offset,
        byte_length,
    }));
    // SAFETY: DataView objects are not callable, so native_fn is an opaque Box<DataViewData>
    // pointer, matching ArrayBuffer and RegExp typed-storage patterns.
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(data as *const ()) }));
    set_named_prop(vm, &mut obj, "buffer", buffer, PropAttributes::new(false, false, false));
    set_named_prop(
        vm,
        &mut obj,
        "byteOffset",
        JsValue::int(byte_offset as i32),
        PropAttributes::new(false, false, false),
    );
    set_named_prop(
        vm,
        &mut obj,
        "byteLength",
        JsValue::int(byte_length as i32),
        PropAttributes::new(false, false, false),
    );
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn data_view_data_ptr(obj: &JsObject) -> Option<*mut DataViewData> {
    if !obj.is_data_view_obj() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut DataViewData)
}

pub(crate) fn data_view_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(ptr) = data_view_data_ptr(obj) else {
        return Vec::new();
    };
    if ptr.is_null() {
        return Vec::new();
    }
    let data = unsafe { *ptr };
    if data.buffer.is_object() {
        vec![data.buffer]
    } else {
        Vec::new()
    }
}

pub(crate) fn clone_data_view_native_with_rewrite<F>(old_obj: &JsObject, new_obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    let Some(ptr) = data_view_data_ptr(old_obj) else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    let mut data = unsafe { *ptr };
    data.buffer = rewrite(data.buffer);
    let cloned = Box::into_raw(Box::new(data));
    new_obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(cloned as *const ()) }));
}

pub(crate) fn rewrite_data_view_native<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    let Some(ptr) = data_view_data_ptr(obj) else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    unsafe {
        (*ptr).buffer = rewrite((*ptr).buffer);
    }
}

pub(crate) fn drop_data_view_native(obj: &mut JsObject) -> u64 {
    let Some(ptr) = data_view_data_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    unsafe { drop(Box::from_raw(ptr)) };
    obj.set_native_fn(None);
    std::mem::size_of::<DataViewData>() as u64
}

pub fn data_view_get_int8(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<1>(vm, args));
    NativeResult::Ok(JsValue::int(i8::from_ne_bytes(bytes) as i32))
}

pub fn data_view_get_uint8(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<1>(vm, args));
    NativeResult::Ok(JsValue::int(bytes[0] as i32))
}

pub fn data_view_get_int16(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<2>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        i16::from_le_bytes(bytes)
    } else {
        i16::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n as i32))
}

pub fn data_view_get_uint16(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<2>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n as i32))
}

pub fn data_view_get_int32(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<4>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        i32::from_le_bytes(bytes)
    } else {
        i32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n))
}

pub fn data_view_get_uint32(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<4>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n as f64))
}

pub fn data_view_get_float32(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<4>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        f32::from_le_bytes(bytes)
    } else {
        f32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n as f64))
}

pub fn data_view_get_float64(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<8>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        f64::from_le_bytes(bytes)
    } else {
        f64::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n))
}

pub fn data_view_get_big_int64(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<8>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        i64::from_le_bytes(bytes)
    } else {
        i64::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n as f64))
}

pub fn data_view_get_big_uint64(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<8>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n as f64))
}

pub fn data_view_set_int8(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32 as u8 as i8;
    native_try!(write_bytes(vm, args, value.to_ne_bytes()));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_uint8(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32 as u8;
    native_try!(write_bytes(vm, args, [value]));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_int16(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32 as u16 as i16;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_uint16(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32 as u16;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_int32(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_uint32(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as u32;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_float32(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as f32;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_float64(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2);
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_big_int64(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i64;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_set_big_uint64(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as u64;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

pub fn data_view_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    native_try!(get_data_view_data(vm, this_val));
    NativeResult::Ok(vm.new_string("[object DataView]"))
}
