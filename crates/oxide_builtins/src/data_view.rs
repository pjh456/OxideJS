use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use num_bigint::BigInt;
use num_traits::ToPrimitive;

use crate::array_buffer::array_buffer_data_ptr;

use oxide_runtime_api::{NativeResult, VmHost};

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

fn to_index<H: VmHost>(vm: &mut H, value: JsValue, msg: &str) -> Result<usize, JsValue> {
    let n = vm.coerce_number_bounded(value).unwrap_or(f64::NAN);
    if n.is_nan() {
        return Ok(0);
    }
    if !n.is_finite() || n < 0.0 {
        return Err(crate::error::create_range_error(vm, msg));
    }
    Ok(n.trunc() as usize)
}

fn is_little_endian<H: VmHost>(vm: &mut H, args: &[u8], idx: usize) -> bool {
    args.get(idx)
        .map(|reg| oxide_runtime_api::to_boolean(vm.reg(*reg)))
        .unwrap_or(false)
}

fn numeric_arg<H: VmHost>(vm: &mut H, args: &[u8], idx: usize) -> f64 {
    args.get(idx)
        .map(|reg| vm.coerce_number_bounded(vm.reg(*reg)).unwrap_or(f64::NAN))
        .unwrap_or(0.0)
}

fn set_named_prop<H: VmHost>(vm: &mut H, obj: &mut JsObject, name: &str, value: JsValue, attributes: PropAttributes) {
    let si = vm.kernel_core().perm_interner().intern(name).0;
    let _ = vm.define_data_property(obj, si, value, attributes);
}

fn get_data_view_data<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<DataViewData, JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "DataView method called on incompatible receiver"));
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "DataView internal state invalid"));
    }
    let obj = unsafe { &*obj_ptr };
    if !obj.is_data_view_obj() {
        return Err(crate::error::create_type_error(vm, "DataView method called on incompatible receiver"));
    }
    let Some(data_ptr) = obj.native_fn() else {
        return Err(crate::error::create_type_error(vm, "DataView internal state invalid"));
    };
    let data_ptr = data_ptr.as_ptr() as *const DataViewData;
    if data_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "DataView internal state invalid"));
    }
    Ok(unsafe { *data_ptr })
}

fn checked_absolute_offset<H: VmHost>(
    vm: &mut H, view: DataViewData, offset: usize, width: usize,
) -> Result<usize, JsValue> {
    let Some(end) = offset.checked_add(width) else {
        return Err(crate::error::create_range_error(vm, "DataView offset out of bounds"));
    };
    if end > view.byte_length {
        return Err(crate::error::create_range_error(vm, "DataView offset out of bounds"));
    }
    Ok(view.byte_offset + offset)
}

fn read_bytes<const N: usize, H: VmHost>(vm: &mut H, args: &[u8]) -> Result<[u8; N], JsValue> {
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
        return Err(crate::error::create_range_error(vm, "DataView offset out of bounds"));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&buffer[abs..abs + N]);
    Ok(out)
}

fn write_bytes<const N: usize, H: VmHost>(vm: &mut H, args: &[u8], bytes: [u8; N]) -> Result<(), JsValue> {
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
        return Err(crate::error::create_range_error(vm, "DataView offset out of bounds"));
    }
    buffer[abs..abs + N].copy_from_slice(&bytes);
    Ok(())
}

/// `DataView(buffer, byteOffset, byteLength)` 构造逻辑：在 ArrayBuffer 上建立定点字节视图。
pub fn data_view_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "DataView requires an ArrayBuffer"));
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
        return NativeResult::Err(crate::error::create_range_error(vm, "DataView byteOffset out of bounds"));
    }
    let default_len = buffer_len - byte_offset;
    let byte_length = if args.len() > 3 {
        native_try!(to_index(vm, vm.reg(args[3]), "DataView byteLength out of bounds"))
    } else {
        default_len
    };
    if byte_offset + byte_length > buffer_len {
        return NativeResult::Err(crate::error::create_range_error(vm, "DataView byteLength out of bounds"));
    }

    let proto = vm.session().builtin_world().data_view_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_DATA_VIEW;
    let data = Box::into_raw(Box::new(DataViewData {
        buffer,
        byte_offset,
        byte_length,
    }));
    // SAFETY: DataView 对象不可调用，native_fn 存不透明 `Box<DataViewData>` 指针，
    // 与 ArrayBuffer / RegExp 的类型化存储模式一致。
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

/// 收集 DataView 引用的底层 buffer（GC 根边）。
pub fn data_view_native_edges(obj: &JsObject) -> Vec<JsValue> {
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

/// 克隆 DataView 视图数据到新对象，用 `rewrite` 改写 buffer 引用。
pub fn clone_data_view_native_with_rewrite<F>(old_obj: &JsObject, new_obj: &mut JsObject, mut rewrite: F)
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

/// 原地重写 DataView 的 buffer 引用。
pub fn rewrite_data_view_native<F>(obj: &mut JsObject, mut rewrite: F)
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

/// 只读核算 DataView 视图数据字节（不释放）。
pub fn data_view_native_size(obj: &JsObject) -> u64 {
    let Some(ptr) = data_view_data_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    std::mem::size_of::<DataViewData>() as u64
}

/// 释放 DataView 视图数据（`Box<DataViewData>`），返回释放字节数。
pub fn drop_data_view_native(obj: &mut JsObject) -> u64 {
    let bytes = data_view_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = data_view_data_ptr(obj) else {
        return 0;
    };
    // SAFETY: ptr 非空（data_view_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe { drop(Box::from_raw(ptr)) };
    obj.set_native_fn(None);
    bytes
}

/// `DataView.prototype.getInt8(byteOffset)`：读取 1 字节有符号整数。
pub fn data_view_get_int8<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<1, H>(vm, args));
    NativeResult::Ok(JsValue::int(i8::from_ne_bytes(bytes) as i32))
}

/// `DataView.prototype.getUint8(byteOffset)`：读取 1 字节无符号整数。
pub fn data_view_get_uint8<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<1, H>(vm, args));
    NativeResult::Ok(JsValue::int(bytes[0] as i32))
}

/// `DataView.prototype.getInt16(byteOffset, littleEndian)`：读取 2 字节有符号整数。
pub fn data_view_get_int16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<2, H>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        i16::from_le_bytes(bytes)
    } else {
        i16::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n as i32))
}

/// `DataView.prototype.getUint16(byteOffset, littleEndian)`：读取 2 字节无符号整数。
pub fn data_view_get_uint16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<2, H>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n as i32))
}

/// `DataView.prototype.getInt32(byteOffset, littleEndian)`：读取 4 字节有符号整数。
pub fn data_view_get_int32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<4, H>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        i32::from_le_bytes(bytes)
    } else {
        i32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n))
}

/// `DataView.prototype.getUint32(byteOffset, littleEndian)`：读取 4 字节无符号整数。
pub fn data_view_get_uint32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<4, H>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n as f64))
}

/// `DataView.prototype.getFloat32(byteOffset, littleEndian)`：读取 4 字节单精度浮点。
pub fn data_view_get_float32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<4, H>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        f32::from_le_bytes(bytes)
    } else {
        f32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n as f64))
}

/// `DataView.prototype.getFloat64(byteOffset, littleEndian)`：读取 8 字节双精度浮点。
pub fn data_view_get_float64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<8, H>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        f64::from_le_bytes(bytes)
    } else {
        f64::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n))
}

/// `DataView.prototype.getBigInt64(byteOffset, littleEndian)`：读取 8 字节有符号 64 位
/// 整数并返回 BigInt 值。
pub fn data_view_get_big_int64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<8, H>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        i64::from_le_bytes(bytes)
    } else {
        i64::from_be_bytes(bytes)
    };
    NativeResult::Ok(vm.new_bigint(BigInt::from(n)))
}

/// `DataView.prototype.getBigUint64(byteOffset, littleEndian)`：读取 8 字节无符号
/// 64 位整数并返回 BigInt 值。
pub fn data_view_get_big_uint64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let bytes = native_try!(read_bytes::<8, H>(vm, args));
    let n = if is_little_endian(vm, args, 2) {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    };
    NativeResult::Ok(vm.new_bigint(BigInt::from(n)))
}

/// `DataView.prototype.setInt8(byteOffset, value)`：写入 1 字节有符号整数。
pub fn data_view_set_int8<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32 as u8 as i8;
    native_try!(write_bytes(vm, args, value.to_ne_bytes()));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setUint8(byteOffset, value)`：写入 1 字节无符号整数。
pub fn data_view_set_uint8<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32 as u8;
    native_try!(write_bytes(vm, args, [value]));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setInt16(byteOffset, value, littleEndian)`：写入 2 字节有符号整数。
pub fn data_view_set_int16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32 as u16 as i16;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setUint16(byteOffset, value, littleEndian)`：写入 2 字节无符号整数。
pub fn data_view_set_uint16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32 as u16;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setInt32(byteOffset, value, littleEndian)`：写入 4 字节有符号整数。
pub fn data_view_set_int32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as i32;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setUint32(byteOffset, value, littleEndian)`：写入 4 字节无符号整数。
pub fn data_view_set_uint32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as u32;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setFloat32(byteOffset, value, littleEndian)`：写入单精度浮点。
pub fn data_view_set_float32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2) as f32;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setFloat64(byteOffset, value, littleEndian)`：写入双精度浮点。
pub fn data_view_set_float64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = numeric_arg(vm, args, 2);
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setBigInt64(byteOffset, value, littleEndian)`：按 ToBigInt
/// 语义接收值（Number 入参抛 TypeError），取低 64 位位模式写入 8 字节有符号整数。
pub fn data_view_set_big_int64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let b =
        native_try!(oxide_runtime_api::to_bigint_full(value, vm).map_err(|e| crate::iterator::engine_error(vm, &e)));
    let low = low64(vm, b) as i64;
    let bytes = if is_little_endian(vm, args, 3) {
        low.to_le_bytes()
    } else {
        low.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setBigUint64(byteOffset, value, littleEndian)`：按 ToBigInt
/// 语义接收值（Number 入参抛 TypeError），取低 64 位位模式写入 8 字节无符号整数。
pub fn data_view_set_big_uint64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let b =
        native_try!(oxide_runtime_api::to_bigint_full(value, vm).map_err(|e| crate::iterator::engine_error(vm, &e)));
    let low = low64(vm, b);
    let bytes = if is_little_endian(vm, args, 3) {
        low.to_le_bytes()
    } else {
        low.to_be_bytes()
    };
    native_try!(write_bytes(vm, args, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// 取 BigInt 值低 64 位位模式：与 2^64-1 掩码后恒非负且可转 u64。
fn low64<H: VmHost>(vm: &mut H, val: JsValue) -> u64 {
    let v = vm.bigint_value(val);
    (v & (BigInt::from(u64::MAX)))
        .to_u64()
        .expect("与 u64::MAX 掩码后恒在 u64 范围")
}

/// `DataView.prototype.toString`：校验 receiver 后返回 `[object DataView]`。
pub fn data_view_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    native_try!(get_data_view_data(vm, this_val));
    NativeResult::Ok(vm.new_string("[object DataView]"))
}
