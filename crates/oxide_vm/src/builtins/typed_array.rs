use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes, TypedArrayKind};
use oxide_types::value::JsValue;

use crate::builtins::array_buffer::{array_buffer_data_ptr, new_array_buffer, MAX_ARRAY_BUFFER_LENGTH};
use crate::coercion;
use crate::vm::Vm;
use oxide_runtime_api::NativeResult;

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

fn type_error(vm: &mut Vm, msg: &str) -> JsValue {
    crate::builtins::error::create_type_error(vm, msg)
}

fn range_error(vm: &mut Vm, msg: &str) -> JsValue {
    crate::builtins::error::create_range_error(vm, msg)
}

fn to_index(vm: &mut Vm, value: JsValue, msg: &str) -> Result<usize, JsValue> {
    let n = coercion::to_number(value);
    if n.is_nan() {
        return Ok(0);
    }
    if !n.is_finite() || n < 0.0 {
        return Err(range_error(vm, msg));
    }
    Ok(n.trunc() as usize)
}

fn normalize_index(_vm: &mut Vm, value: JsValue, len: usize) -> usize {
    let n = coercion::to_number(value);
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
    let si = vm.kernel_core().perm_interner().intern(name).0;
    let _ = vm.define_data_property(obj, si, value, attributes);
}

fn typed_array_proto_ptr(vm: &mut Vm, kind: TypedArrayKind) -> *mut JsObject {
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

fn create_typed_array(
    vm: &mut Vm, kind: TypedArrayKind, buffer: JsValue, byte_offset: usize, length: usize,
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
    // SAFETY: TypedArray instances are not callable; native_fn stores opaque Box<TypedArrayData>,
    // matching ArrayBuffer/DataView typed-object storage in this VM.
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(data as *const ()) }));

    let byte_length = length.saturating_mul(kind.bytes_per_element());
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
    set_named_prop(
        vm,
        &mut obj,
        "length",
        JsValue::int(length as i32),
        PropAttributes::new(false, false, false),
    );
    vm.alloc_object(obj)
}

fn typed_array_data_ptr(obj: &JsObject) -> Option<*mut TypedArrayData> {
    if !obj.is_typed_array_obj() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut TypedArrayData)
}

pub(crate) fn typed_array_native_edges(obj: &JsObject) -> Vec<JsValue> {
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

pub(crate) fn clone_typed_array_native_with_rewrite<F>(old_obj: &JsObject, new_obj: &mut JsObject, mut rewrite: F)
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

pub(crate) fn rewrite_typed_array_native<F>(obj: &mut JsObject, mut rewrite: F)
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

pub(crate) fn drop_typed_array_native(obj: &mut JsObject) -> u64 {
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

pub(crate) fn get_typed_array_data(vm: &mut Vm, this_val: JsValue) -> Result<TypedArrayData, JsValue> {
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

fn numeric_value(_vm: &mut Vm, value: JsValue) -> f64 {
    coercion::to_number(value)
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

fn collect_array_like(vm: &mut Vm, value: JsValue) -> Result<Vec<JsValue>, JsValue> {
    if !value.is_object() {
        return Err(type_error(vm, "TypedArray source must be array-like"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return Err(type_error(vm, "TypedArray source must be array-like"));
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
    Err(type_error(vm, "TypedArray source must be Array, ArrayBuffer, or TypedArray"))
}

fn typed_array_new(vm: &mut Vm, args: &[u8], kind: TypedArrayKind) -> NativeResult {
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
        pub fn $name(vm: &mut Vm, args: &[u8]) -> NativeResult {
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

pub fn typed_array_at(vm: &mut Vm, args: &[u8]) -> NativeResult {
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

pub fn typed_array_fill(vm: &mut Vm, args: &[u8]) -> NativeResult {
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

pub fn typed_array_slice(vm: &mut Vm, args: &[u8]) -> NativeResult {
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

pub fn typed_array_subarray(vm: &mut Vm, args: &[u8]) -> NativeResult {
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

pub fn typed_array_set(vm: &mut Vm, args: &[u8]) -> NativeResult {
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

pub fn typed_array_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    native_try!(get_typed_array_data(vm, this_val));
    NativeResult::Ok(vm.new_string("[object TypedArray]"))
}
