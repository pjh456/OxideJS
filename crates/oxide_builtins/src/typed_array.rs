use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, TypedArrayKind};
use oxide_types::value::JsValue;

use crate::array_buffer::{array_buffer_data_ptr, new_array_buffer, MAX_ARRAY_BUFFER_LENGTH};

use oxide_runtime_api::{NativeResult, VmHost};

#[derive(Clone, Copy)]
pub(crate) struct TypedArrayData {
    pub kind: TypedArrayKind,
    pub buffer: JsValue,
    pub byte_offset: usize,
    pub length: usize,
}

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

fn type_error<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    crate::error::create_type_error(vm, msg)
}

fn range_error<H: VmHost>(vm: &mut H, msg: &str) -> JsValue {
    crate::error::create_range_error(vm, msg)
}

fn to_index<H: VmHost>(vm: &mut H, value: JsValue, msg: &str) -> Result<usize, JsValue> {
    let n = vm.coerce_number_bounded(value).unwrap_or(f64::NAN);
    if n.is_nan() {
        return Ok(0);
    }
    if !n.is_finite() || n < 0.0 {
        return Err(range_error(vm, msg));
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

fn typed_array_proto_ptr<H: VmHost>(vm: &mut H, kind: TypedArrayKind) -> *mut JsObject {
    let world = vm.session().builtin_world();
    match kind {
        TypedArrayKind::Int8 => world.int8array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::Uint8 => world.uint8array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::Uint8Clamped => world.uint8clampedarray_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::Int16 => world.int16array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::Uint16 => world.uint16array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::Int32 => world.int32array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::Uint32 => world.uint32array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::Float32 => world.float32array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::Float64 => world.float64array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::BigInt64 => world.bigint64array_proto.as_ptr() as *mut JsObject,
        TypedArrayKind::BigUint64 => world.biguint64array_proto.as_ptr() as *mut JsObject,
    }
}

fn create_typed_array<H: VmHost>(
    vm: &mut H, kind: TypedArrayKind, buffer: JsValue, byte_offset: usize, length: usize,
) -> *mut JsObject {
    let proto = typed_array_proto_ptr(vm, kind);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_TYPED_ARRAY;
    let data = Box::into_raw(Box::new(TypedArrayData {
        kind,
        buffer,
        byte_offset,
        length,
    }));
    // SAFETY: TypedArray 实例不可调用，native_fn 存不透明 `Box<TypedArrayData>`，
    // 与本 VM 中 ArrayBuffer/DataView 的类型化对象存储一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(data as *const ()) }));
    vm.alloc_object(obj)
}

fn typed_array_data_ptr(obj: &JsObject) -> Option<*mut TypedArrayData> {
    if !obj.is_typed_array_obj() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut TypedArrayData)
}

/// 收集 TypedArray 引用的底层 buffer（GC 根边）。
pub fn typed_array_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(ptr) = typed_array_data_ptr(obj) else {
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

/// 克隆 TypedArray 的视图数据到新对象，用 `rewrite` 改写 buffer 引用。
pub fn clone_typed_array_native_with_rewrite<F>(old_obj: &JsObject, new_obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    let Some(ptr) = typed_array_data_ptr(old_obj) else {
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

/// 原地重写 TypedArray 的 buffer 引用。
pub fn rewrite_typed_array_native<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    let Some(ptr) = typed_array_data_ptr(obj) else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    unsafe {
        (*ptr).buffer = rewrite((*ptr).buffer);
    }
}

/// 释放 TypedArray 的视图数据（`Box<TypedArrayData>`），返回释放字节数。
pub fn drop_typed_array_native(obj: &mut JsObject) -> u64 {
    let Some(ptr) = typed_array_data_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    unsafe { drop(Box::from_raw(ptr)) };
    obj.set_native_fn(None);
    std::mem::size_of::<TypedArrayData>() as u64
}

pub(crate) fn get_typed_array_data<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<TypedArrayData, JsValue> {
    if !this_val.is_object() {
        return Err(type_error(vm, "TypedArray method called on incompatible receiver"));
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(type_error(vm, "TypedArray internal state invalid"));
    }
    let obj = unsafe { &*obj_ptr };
    if !obj.is_typed_array_obj() {
        return Err(type_error(vm, "TypedArray method called on incompatible receiver"));
    }
    let Some(data_ptr) = obj.native_fn() else {
        return Err(type_error(vm, "TypedArray internal state invalid"));
    };
    let data_ptr = data_ptr.as_ptr() as *const TypedArrayData;
    if data_ptr.is_null() {
        return Err(type_error(vm, "TypedArray internal state invalid"));
    }
    Ok(unsafe { *data_ptr })
}

fn absolute_byte_offset(view: TypedArrayData, index: usize) -> usize {
    view.byte_offset + index * view.kind.bytes_per_element()
}

/// 读 TypedArray 指定整数索引的元素（供 VM 普通属性 get 的 typed 分支使用）。
/// 越界返回 undefined；内部状态非法返回 Err(String)。
pub fn typed_array_element_get<H: VmHost>(vm: &mut H, obj: &JsObject, index: u32) -> Result<JsValue, String> {
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = get_typed_array_data(vm, this_val).map_err(|e| format!("{e}"))?;
    if index as usize >= view.length {
        return Ok(JsValue::undefined());
    }
    let buffer_ptr = array_buffer_data_ptr(vm, view.buffer).map_err(|e| format!("{e}"))?;
    let buffer = unsafe { &*buffer_ptr };
    Ok(read_element(view.kind, buffer, absolute_byte_offset(view, index as usize)))
}

/// 若对象是 TypedArray 且属性键是整数索引，返回 `(索引, 视图长度)`；否则 `None`。
/// 供 VM 在 receiver ≠ TA 时裁决整数索引的写前语义（越界直接返回，界内落到 receiver）。
pub fn typed_array_integer_index<H: VmHost>(vm: &mut H, obj: &JsObject, prop_name_si: u32) -> Option<(usize, usize)> {
    let ptr = typed_array_data_ptr(obj)?;
    if ptr.is_null() {
        return None;
    }
    let view = unsafe { *ptr };
    let key = vm.kernel_core().perm_interner().lookup(prop_name_si)?;
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    let index = key.parse::<u32>().ok()?;
    Some((index as usize, view.length))
}

/// 写 TypedArray 指定整数索引的元素（供 VM 普通属性 set 的 typed 分支使用）。
/// 越界忽略（不创建属性、不报错）；内部状态非法返回 Err(String)。
pub fn typed_array_element_set<H: VmHost>(
    vm: &mut H, obj: &JsObject, index: u32, value: JsValue,
) -> Result<(), String> {
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = get_typed_array_data(vm, this_val).map_err(|e| format!("{e}"))?;
    write_typed_array_element(vm, view, index, value)
}

/// 定义 TypedArray 整数索引元素（defineProperty 语义）。
///
/// # 边界与前提
/// - 索引越界：拒绝定义（Err），与整数索引 exotic 对象的 [[DefineOwnProperty]] 一致
///
/// # 副作用
/// - 把值 ToNumber 后写入底层 buffer
// ponytail: 不校验 writable 描述符——调用方把"省略 writable"折叠为 false，
// 无法与显式 false 区分；TA 元素天然可写，直接写入。
pub fn typed_array_element_define<H: VmHost>(
    vm: &mut H, obj: &JsObject, index: u32, value: JsValue,
) -> Result<(), String> {
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = get_typed_array_data(vm, this_val).map_err(|e| format!("{e}"))?;
    if index as usize >= view.length {
        return Err("cannot define property: TypedArray index out of range".to_string());
    }
    write_typed_array_element(vm, view, index, value)
}

fn write_typed_array_element<H: VmHost>(
    vm: &mut H, view: TypedArrayData, index: u32, value: JsValue,
) -> Result<(), String> {
    // 先 ToNumber（valueOf 副作用先于越界判定触发），越界再静默忽略。
    let n = numeric_value(vm, value);
    if index as usize >= view.length {
        return Ok(());
    }
    let buffer_ptr = array_buffer_data_ptr(vm, view.buffer).map_err(|e| format!("{e}"))?;
    // SAFETY: buffer_ptr 经 array_buffer_data_ptr 校验为合法 ArrayBuffer。
    let buffer = unsafe { &mut *buffer_ptr };
    write_element(view.kind, buffer, absolute_byte_offset(view, index as usize), n);
    Ok(())
}

fn read_element(kind: TypedArrayKind, bytes: &[u8], offset: usize) -> JsValue {
    match kind {
        TypedArrayKind::Int8 => JsValue::int(bytes[offset] as i8 as i32),
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => JsValue::int(bytes[offset] as i32),
        TypedArrayKind::Int16 => JsValue::int(i16::from_ne_bytes(bytes[offset..offset + 2].try_into().unwrap()) as i32),
        TypedArrayKind::Uint16 => {
            JsValue::int(u16::from_ne_bytes(bytes[offset..offset + 2].try_into().unwrap()) as i32)
        }
        TypedArrayKind::Int32 => JsValue::int(i32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())),
        TypedArrayKind::Uint32 => {
            JsValue::float(u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap()) as f64)
        }
        TypedArrayKind::Float32 => {
            JsValue::float(f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap()) as f64)
        }
        TypedArrayKind::Float64 => JsValue::float(f64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())),
        TypedArrayKind::BigInt64 => {
            JsValue::float(i64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap()) as f64)
        }
        TypedArrayKind::BigUint64 => {
            JsValue::float(u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap()) as f64)
        }
    }
}

fn numeric_value<H: VmHost>(vm: &mut H, value: JsValue) -> f64 {
    vm.coerce_number_bounded(value).unwrap_or(f64::NAN)
}

fn write_element(kind: TypedArrayKind, bytes: &mut [u8], offset: usize, value: f64) {
    match kind {
        TypedArrayKind::Int8 => bytes[offset] = value as i32 as u8 as i8 as u8,
        TypedArrayKind::Uint8 => bytes[offset] = value as i32 as u8,
        TypedArrayKind::Uint8Clamped => bytes[offset] = value.clamp(0.0, 255.0).round() as u8,
        TypedArrayKind::Int16 => bytes[offset..offset + 2].copy_from_slice(&(value as i32 as u16 as i16).to_ne_bytes()),
        TypedArrayKind::Uint16 => bytes[offset..offset + 2].copy_from_slice(&(value as i32 as u16).to_ne_bytes()),
        TypedArrayKind::Int32 => bytes[offset..offset + 4].copy_from_slice(&(value as i32).to_ne_bytes()),
        TypedArrayKind::Uint32 => bytes[offset..offset + 4].copy_from_slice(&(value as u32).to_ne_bytes()),
        TypedArrayKind::Float32 => bytes[offset..offset + 4].copy_from_slice(&(value as f32).to_ne_bytes()),
        TypedArrayKind::Float64 => bytes[offset..offset + 8].copy_from_slice(&value.to_ne_bytes()),
        TypedArrayKind::BigInt64 => bytes[offset..offset + 8].copy_from_slice(&(value as i64).to_ne_bytes()),
        TypedArrayKind::BigUint64 => bytes[offset..offset + 8].copy_from_slice(&(value as u64).to_ne_bytes()),
    }
}

fn collect_array_like<H: VmHost>(vm: &mut H, value: JsValue) -> Result<Vec<JsValue>, JsValue> {
    if !value.is_object() {
        return Err(type_error(vm, "TypedArray source must be array-like or iterable"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return Err(type_error(vm, "TypedArray source must be array-like or iterable"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_array() {
        let len = obj.prop_count() as usize;
        return Ok((0..len).map(|i| obj.get_prop_at(i)).collect());
    }
    if obj.is_typed_array_obj() {
        let view = get_typed_array_data(vm, value)?;
        let buffer_ptr = array_buffer_data_ptr(vm, view.buffer)?;
        let buffer = unsafe { &*buffer_ptr };
        return Ok((0..view.length)
            .map(|i| read_element(view.kind, buffer, absolute_byte_offset(view, i)))
            .collect());
    }

    // 有 @@iterator 走迭代路径（含用户自定义迭代器），否则按 array-like 读 length 逐索引取值。
    if crate::iterator::peek_iterator_method(vm, value)? {
        let mut values = Vec::new();
        crate::iterator::iterate_elements(vm, value, |_vm, elem| {
            values.push(elem);
            Ok(())
        })?;
        return Ok(values);
    }

    let length_si = vm.kernel_core().perm_interner().intern("length").0;
    let len_val = vm
        .ordinary_get(obj, length_si, value)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    let n = oxide_runtime_api::to_number_full(len_val, vm).map_err(|e| crate::iterator::engine_error(vm, &e))?;
    let len = to_collect_len(n);
    let mut values = Vec::with_capacity(len);
    for i in 0..len {
        let key = vm.new_string(&i.to_string());
        let key_si = vm.property_key_si(key);
        let elem = vm
            .ordinary_get(obj, key_si, value)
            .map_err(|e| crate::iterator::engine_error(vm, &e))?;
        values.push(elem);
    }
    Ok(values)
}

/// 按 ToLength 语义把 length 数值夹到收集上限：NaN/非正取 0，超出密集上限截断。
fn to_collect_len(n: f64) -> usize {
    if n.is_nan() || n <= 0.0 {
        0
    } else {
        (n.min(9_007_199_254_740_991.0).trunc() as u64).min(oxide_types::object::MAX_DENSE_PROPS as u64) as usize
    }
}

fn typed_array_new<H: VmHost>(vm: &mut H, args: &[u8], kind: TypedArrayKind) -> NativeResult {
    let first = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::int(0) };
    let bpe = kind.bytes_per_element();

    let (buffer, byte_offset, length) = if first.is_int() || first.is_double() || first.is_undefined() {
        let len = native_try!(to_index(vm, first, "invalid TypedArray length"));
        let Some(byte_len) = len.checked_mul(bpe) else {
            return NativeResult::Err(range_error(vm, "invalid TypedArray length"));
        };
        if byte_len > MAX_ARRAY_BUFFER_LENGTH {
            return NativeResult::Err(range_error(vm, "invalid TypedArray length"));
        }
        let buffer = JsValue::from_js_object(new_array_buffer(vm, vec![0; byte_len]));
        (buffer, 0, len)
    } else if first.is_object() {
        let first_ptr = first.as_js_object_ptr();
        let first_obj = unsafe { &*first_ptr };
        if first_obj.is_array_buffer_obj() {
            let buffer_ptr = native_try!(array_buffer_data_ptr(vm, first));
            let buffer_len = unsafe { (*buffer_ptr).len() };
            let byte_offset = if args.len() > 2 {
                native_try!(to_index(vm, vm.reg(args[2]), "TypedArray byteOffset out of bounds"))
            } else {
                0
            };
            if byte_offset > buffer_len || byte_offset % bpe != 0 {
                return NativeResult::Err(range_error(vm, "TypedArray byteOffset out of bounds"));
            }
            let remaining = buffer_len - byte_offset;
            let length = if args.len() > 3 {
                native_try!(to_index(vm, vm.reg(args[3]), "TypedArray length out of bounds"))
            } else {
                remaining / bpe
            };
            let Some(byte_length) = length.checked_mul(bpe) else {
                return NativeResult::Err(range_error(vm, "TypedArray length out of bounds"));
            };
            if byte_length > remaining {
                return NativeResult::Err(range_error(vm, "TypedArray length out of bounds"));
            }
            (first, byte_offset, length)
        } else {
            let values = native_try!(collect_array_like(vm, first));
            let byte_len = values.len().saturating_mul(bpe);
            let buffer = JsValue::from_js_object(new_array_buffer(vm, vec![0; byte_len]));
            let buffer_ptr = native_try!(array_buffer_data_ptr(vm, buffer));
            let buffer_ref = unsafe { &mut *buffer_ptr };
            for (idx, value) in values.into_iter().enumerate() {
                let n = numeric_value(vm, value);
                write_element(kind, buffer_ref, idx * bpe, n);
            }
            (buffer, 0, byte_len / bpe)
        }
    } else {
        return NativeResult::Err(type_error(vm, "invalid TypedArray constructor argument"));
    };

    NativeResult::Ok(JsValue::from_js_object(create_typed_array(vm, kind, buffer, byte_offset, length)))
}

macro_rules! typed_array_ctor {
    ($name:ident, $kind:ident) => {
        /// 对应类型（如 `Int8Array`）的构造函数：支持数字长度、ArrayBuffer+offset+length
        /// 或 array-like 数据三种调用形式。
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            typed_array_new(vm, args, TypedArrayKind::$kind)
        }
    };
}

typed_array_ctor!(int8array_constructor, Int8);
typed_array_ctor!(uint8array_constructor, Uint8);
typed_array_ctor!(uint8clampedarray_constructor, Uint8Clamped);
typed_array_ctor!(int16array_constructor, Int16);
typed_array_ctor!(uint16array_constructor, Uint16);
typed_array_ctor!(int32array_constructor, Int32);
typed_array_ctor!(uint32array_constructor, Uint32);
typed_array_ctor!(float32array_constructor, Float32);
typed_array_ctor!(float64array_constructor, Float64);
typed_array_ctor!(bigint64array_constructor, BigInt64);
typed_array_ctor!(biguint64array_constructor, BigUint64);

/// `TypedArray.prototype.at(index)`：按索引取元素，支持负索引；越界返回 undefined。
pub fn typed_array_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let raw_index = if args.len() > 1 {
        numeric_value(vm, vm.reg(args[1])).trunc() as isize
    } else {
        0
    };
    let idx = if raw_index < 0 { view.length as isize + raw_index } else { raw_index };
    if idx < 0 || idx as usize >= view.length {
        return NativeResult::Ok(JsValue::undefined());
    }
    let buffer_ptr = native_try!(array_buffer_data_ptr(vm, view.buffer));
    let buffer = unsafe { &*buffer_ptr };
    NativeResult::Ok(read_element(view.kind, buffer, absolute_byte_offset(view, idx as usize)))
}

/// `TypedArray.prototype.fill(value, start, end)`：用给定值填充区间，返回 this。
pub fn typed_array_fill<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let start = if args.len() > 2 {
        normalize_index(vm, vm.reg(args[2]), view.length)
    } else {
        0
    };
    let end = if args.len() > 3 {
        normalize_index(vm, vm.reg(args[3]), view.length)
    } else {
        view.length
    };
    let n = numeric_value(vm, value);
    let buffer_ptr = native_try!(array_buffer_data_ptr(vm, view.buffer));
    let buffer = unsafe { &mut *buffer_ptr };
    for idx in start..end.max(start) {
        write_element(view.kind, buffer, absolute_byte_offset(view, idx), n);
    }
    NativeResult::Ok(this_val)
}

/// `TypedArray.prototype.slice(start, end)`：复制区间元素生成新同类型 TypedArray。
pub fn typed_array_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let start = if args.len() > 1 {
        normalize_index(vm, vm.reg(args[1]), view.length)
    } else {
        0
    };
    let end = if args.len() > 2 {
        normalize_index(vm, vm.reg(args[2]), view.length)
    } else {
        view.length
    };
    let count = end.max(start).saturating_sub(start);
    let bpe = view.kind.bytes_per_element();
    let src_ptr = native_try!(array_buffer_data_ptr(vm, view.buffer));
    let src = unsafe { &*src_ptr };
    let mut out = vec![0u8; count * bpe];
    let src_start = absolute_byte_offset(view, start);
    let src_end = src_start + count * bpe;
    out.copy_from_slice(&src[src_start..src_end]);
    let buffer = JsValue::from_js_object(new_array_buffer(vm, out));
    NativeResult::Ok(JsValue::from_js_object(create_typed_array(vm, view.kind, buffer, 0, count)))
}

/// `TypedArray.prototype.subarray(start, end)`：共享底层 buffer 创建区间子视图。
pub fn typed_array_subarray<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let start = if args.len() > 1 {
        normalize_index(vm, vm.reg(args[1]), view.length)
    } else {
        0
    };
    let end = if args.len() > 2 {
        normalize_index(vm, vm.reg(args[2]), view.length)
    } else {
        view.length
    };
    let count = end.max(start).saturating_sub(start);
    let byte_offset = view.byte_offset + start * view.kind.bytes_per_element();
    NativeResult::Ok(JsValue::from_js_object(create_typed_array(
        vm,
        view.kind,
        view.buffer,
        byte_offset,
        count,
    )))
}

/// `TypedArray.prototype.set(source, offset)`：从 array-like/另一个 TypedArray 拷贝元素；
/// 越界抛 RangeError。
pub fn typed_array_set<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "TypedArray.set source required"));
    }
    let source = vm.reg(args[1]);
    let offset = if args.len() > 2 {
        native_try!(to_index(vm, vm.reg(args[2]), "TypedArray.set offset out of bounds"))
    } else {
        0
    };
    let values = native_try!(collect_array_like(vm, source));
    if offset > view.length || values.len() > view.length - offset {
        return NativeResult::Err(range_error(vm, "TypedArray.set offset out of bounds"));
    }
    let numbers: Vec<f64> = values.into_iter().map(|v| numeric_value(vm, v)).collect();
    let buffer_ptr = native_try!(array_buffer_data_ptr(vm, view.buffer));
    let buffer = unsafe { &mut *buffer_ptr };
    for (i, n) in numbers.into_iter().enumerate() {
        write_element(view.kind, buffer, absolute_byte_offset(view, offset + i), n);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `%TypedArray%` 抽象构造器：不可 new 也不可调用，任何调用方式都抛 TypeError。
pub fn typed_array_abstract_constructor<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "TypedArray is not a constructor"))
}

/// `%TypedArray%.prototype.buffer` 访问器：返回视图引用的 ArrayBuffer。
pub fn typed_array_buffer_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    NativeResult::Ok(view.buffer)
}

/// `%TypedArray%.prototype.byteOffset` 访问器：返回视图相对 buffer 起始的字节偏移。
pub fn typed_array_byte_offset_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    NativeResult::Ok(JsValue::int(view.byte_offset as i32))
}

/// `%TypedArray%.prototype.byteLength` 访问器：返回视图占用的字节数。
pub fn typed_array_byte_length_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    NativeResult::Ok(JsValue::int((view.length * view.kind.bytes_per_element()) as i32))
}

/// `%TypedArray%.prototype.length` 访问器：返回元素个数。
pub fn typed_array_length_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    NativeResult::Ok(JsValue::int(view.length as i32))
}

/// `%TypedArray%.prototype[@@toStringTag]` 访问器：返回具体类型名（如 `Int16Array`），
/// 供 `Object.prototype.toString` 区分类型。
pub fn typed_array_to_string_tag_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    NativeResult::Ok(vm.new_string(view.kind.name()))
}

/// 按 TypedArrayCreateWithLength 语义构造 of/from 的结果对象：调用构造器 C 分配
/// 长度为 `len` 的 TypedArray，校验结果确为 TypedArray 且长度不小于 `len`。
///
/// # 边界与前提
/// - C 非可调用函数、调用抛错、返回非 TypedArray、或返回对象长度不足时抛 TypeError。
fn allocate_typed_array<H: VmHost>(vm: &mut H, c: JsValue, len: usize) -> Result<JsValue, JsValue> {
    if !c.is_object() || c.as_js_object_ptr().is_null() || !unsafe { &*c.as_js_object_ptr() }.is_function() {
        return Err(type_error(vm, "TypedArray.of/from requires a constructor"));
    }
    let result = vm
        .call_function_sync(c, JsValue::undefined(), &[JsValue::int(len as i32)])
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    if !result.is_object() || result.as_js_object_ptr().is_null() {
        return Err(type_error(vm, "TypedArray.of/from constructor did not return a TypedArray"));
    }
    let result_obj = unsafe { &*result.as_js_object_ptr() };
    if !result_obj.is_typed_array_obj() {
        return Err(type_error(vm, "TypedArray.of/from constructor did not return a TypedArray"));
    }
    let view = get_typed_array_data(vm, result)?;
    if view.length < len {
        return Err(type_error(
            vm,
            "TypedArray.of/from constructor returned a TypedArray with insufficient length",
        ));
    }
    Ok(result)
}

/// 按 IntegerIndexedElementSet 语义把元素写入 TypedArray：先 ToNumber（symbol 等
/// 不可转换值抛 TypeError），越界索引静默忽略。
fn set_typed_array_element<H: VmHost>(vm: &mut H, ta: JsValue, index: usize, value: JsValue) -> Result<(), JsValue> {
    let n = oxide_runtime_api::to_number_full(value, vm).map_err(|e| crate::iterator::engine_error(vm, &e))?;
    let obj = unsafe { &*ta.as_js_object_ptr() };
    typed_array_element_set(vm, obj, index as u32, JsValue::float(n)).map_err(|e| crate::iterator::engine_error(vm, &e))
}

/// `%TypedArray%.of(...items)`：以实参为元素构造一个以 `this`（构造器 C）为类型的
/// TypedArray；元素逐个 ToNumber 写入，不可转换（如 Symbol）抛 TypeError。
pub fn typed_array_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let c = vm.reg(args[0]);
    let len = args.len().saturating_sub(1);
    let new_obj = match allocate_typed_array(vm, c, len) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    for i in 0..len {
        let value = vm.reg(args[1 + i]);
        if let Err(e) = set_typed_array_element(vm, new_obj, i, value) {
            return NativeResult::Err(e);
        }
    }
    NativeResult::Ok(new_obj)
}

/// `%TypedArray%.from(source, mapfn?, thisArg?)`：从可迭代对象或 array-like 构造
/// 以 `this`（构造器 C）为类型的 TypedArray；`mapfn` 逐元素映射（`thisArg` 作回调
/// this），元素经 ToNumber 写入。
///
/// # 步骤
/// 1. `source` 为 null/undefined 抛 TypeError；`mapfn` 非 undefined 时须可调用。
/// 2. 经 `@@iterator` 判定可迭代：迭代路径逐元素收集（异常退出先 IteratorClose），
///    array-like 路径读 `length` 逐索引取值（缺失属性取 undefined）。
/// 3. 按长度构造结果对象，逐元素经 `mapfn` 映射后 ToNumber 写入。
pub fn typed_array_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let c = vm.reg(args[0]);
    let source = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if source.is_null() || source.is_undefined() {
        return NativeResult::Err(type_error(vm, "TypedArray.from requires an iterable or array-like object"));
    }
    let mapfn_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mapping = if mapfn_val.is_undefined() {
        None
    } else {
        match crate::array::require_callback(vm, mapfn_val) {
            Ok(cb) => Some(cb),
            Err(e) => return NativeResult::Err(e),
        }
    };
    let this_arg = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };

    let values = match crate::iterator::peek_iterator_method(vm, source) {
        Ok(true) => {
            let mut values = Vec::new();
            match crate::iterator::iterate_elements(vm, source, |_vm, elem| {
                values.push(elem);
                Ok(())
            }) {
                Ok(()) => values,
                Err(e) => return NativeResult::Err(e),
            }
        }
        Ok(false) => {
            let obj_val = match oxide_runtime_api::to_object(source, vm) {
                Ok(o) => o,
                Err(err) => return NativeResult::Err(crate::error::create_type_error(vm, &err)),
            };
            let obj = unsafe { &*obj_val.as_js_object_ptr() };
            let length_si = vm.kernel_core().perm_interner().intern("length").0;
            let len_val = match vm.ordinary_get(obj, length_si, obj_val) {
                Ok(v) => v,
                Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
            };
            let n = match oxide_runtime_api::to_number_full(len_val, vm) {
                Ok(n) => n,
                Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
            };
            let len = to_collect_len(n);
            let mut values = Vec::with_capacity(len);
            for i in 0..len {
                let key = vm.new_string(&i.to_string());
                let key_si = vm.property_key_si(key);
                match vm.ordinary_get(obj, key_si, obj_val) {
                    Ok(v) => values.push(v),
                    Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
                }
            }
            values
        }
        Err(e) => return NativeResult::Err(e),
    };

    let new_obj = match allocate_typed_array(vm, c, values.len()) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    for (k, elem) in values.into_iter().enumerate() {
        let mapped = match mapping {
            Some(cb) => match crate::array::invoke_native_callback(vm, cb, this_arg, &[elem, JsValue::int(k as i32)]) {
                NativeResult::Ok(m) => m,
                NativeResult::Err(e) => return NativeResult::Err(e),
                NativeResult::TailCall { .. } => {
                    return NativeResult::Err(type_error(vm, "unexpected tail call in TypedArray.from callback"))
                }
            },
            None => elem,
        };
        if let Err(e) = set_typed_array_element(vm, new_obj, k, mapped) {
            return NativeResult::Err(e);
        }
    }
    NativeResult::Ok(new_obj)
}
