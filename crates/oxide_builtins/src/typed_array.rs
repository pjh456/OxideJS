use num_bigint::BigInt;
use num_traits::ToPrimitive;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, TypedArrayKind};
use oxide_types::private_key::{int_key_value, is_int_key};
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
    let n = match vm.coerce_number_bounded(value) {
        Ok(n) => n,
        Err(e) => return Err(crate::iterator::engine_error(vm, &e)),
    };
    if n.is_nan() {
        return Ok(0);
    }
    if !n.is_finite() || n < 0.0 {
        return Err(range_error(vm, msg));
    }
    Ok(n.trunc() as usize)
}

/// 按 ToIntegerOrInfinity 语义把索引归一化到 `[0, len]`（越界夹取）；符号等不可
/// 转换值透传异常。
fn normalize_index<H: VmHost>(vm: &mut H, value: JsValue, len: usize) -> Result<usize, JsValue> {
    let n = match vm.coerce_number_bounded(value) {
        Ok(n) => n,
        Err(e) => return Err(crate::iterator::engine_error(vm, &e)),
    };
    if n.is_nan() {
        return Ok(0);
    }
    let int = n.trunc() as isize;
    if int < 0 {
        Ok(len.saturating_sub((-int) as usize))
    } else {
        Ok((int as usize).min(len))
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

/// 只读核算 TypedArray 视图数据字节（不释放）。
pub fn typed_array_native_size(obj: &JsObject) -> u64 {
    let Some(ptr) = typed_array_data_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    std::mem::size_of::<TypedArrayData>() as u64
}

/// 释放 TypedArray 的视图数据（`Box<TypedArrayData>`），返回释放字节数。
pub fn drop_typed_array_native(obj: &mut JsObject) -> u64 {
    let bytes = typed_array_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = typed_array_data_ptr(obj) else {
        return 0;
    };
    // SAFETY: ptr 非空（typed_array_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe { drop(Box::from_raw(ptr)) };
    obj.set_native_fn(None);
    bytes
}

/// 取接收者的 TypedArrayData 视图（按值返回，非借出）。
///
/// # 边界与前提
/// - 接收者非对象或不是 TypedArray 对象时抛 TypeError（接收者不兼容）；
/// - 对象指针为空、`native_fn` 槽为空值或存空指针时抛 TypeError（内部状态无效）。
///
/// # 注意事项
/// - 返回的是解引用后的拷贝（`Ok(*data_ptr)`），底层 buffer 仍由对象持有，
///   调用方不得据此延长其生命周期。
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
    Ok(read_element(vm, view.kind, buffer, absolute_byte_offset(view, index as usize)))
}

/// 若对象是 TypedArray 且属性键是整数索引，返回 `(索引, 视图长度)`；否则 `None`。
/// 供 VM 在 receiver ≠ TA 时裁决整数索引的写前语义（越界直接返回，界内落到 receiver）。
pub fn typed_array_integer_index<H: VmHost>(vm: &mut H, obj: &JsObject, prop_name_si: u32) -> Option<(usize, usize)> {
    let ptr = typed_array_data_ptr(obj)?;
    if ptr.is_null() {
        return None;
    }
    let view = unsafe { *ptr };
    let index = if is_int_key(prop_name_si) {
        int_key_value(prop_name_si)
    } else {
        let key = vm.kernel_core().perm_interner().lookup(prop_name_si)?;
        if key.is_empty() || (key.len() > 1 && key.starts_with('0')) {
            return None;
        }
        key.parse::<u32>().ok()?
    };
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
    // 先按元素类型转换（valueOf 副作用先于越界判定触发），越界再静默忽略。
    let elem = match ta_element_value(vm, view.kind, value) {
        Ok(v) => v,
        Err(err) => {
            // 转换失败恢复为可捕获的 JS 异常：主 dispatch 下就地展开到外围 catch，
            // builtin 内部展开到调用方 try 处理器；uncaught 时以文本上抛。
            let text = element_error_text(vm, err);
            return vm.raise_type_error(&text);
        }
    };
    if index as usize >= view.length {
        return Ok(());
    }
    let buffer_ptr = array_buffer_data_ptr(vm, view.buffer).map_err(|e| format!("{e}"))?;
    // SAFETY: buffer_ptr 经 array_buffer_data_ptr 校验为合法 ArrayBuffer。
    let buffer = unsafe { &mut *buffer_ptr };
    write_element(vm, view.kind, buffer, absolute_byte_offset(view, index as usize), elem);
    Ok(())
}

/// 读 TypedArray 元素并转为对应 JS 值：BigInt 类型读为 BigInt 值（i64/u64 位模式
/// 原样搬运，无精度损失），数值类型按位模式读为 Number。
fn read_element<H: VmHost>(vm: &mut H, kind: TypedArrayKind, bytes: &[u8], offset: usize) -> JsValue {
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
            let n = i64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap());
            vm.new_bigint(BigInt::from(n))
        }
        TypedArrayKind::BigUint64 => {
            let n = u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap());
            vm.new_bigint(BigInt::from(n))
        }
    }
}

/// 元素写入统一转换入口：BigInt 类型走 ToBigInt，数值类型走 ToNumber。
///
/// 数值类型显式拒绝 BigInt 值（规范 ToNumber(BigInt) 抛 TypeError），避免
/// 经 f64 近似的静默精度丢失。
fn ta_element_value<H: VmHost>(vm: &mut H, kind: TypedArrayKind, value: JsValue) -> Result<JsValue, JsValue> {
    if is_bigint_kind(kind) {
        return oxide_runtime_api::to_bigint_full(value, vm).map_err(|e| crate::iterator::engine_error(vm, &e));
    }
    let prim = oxide_runtime_api::to_primitive(value, oxide_runtime_api::ToPrimitiveHint::Number, vm)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    if prim.is_bigint() {
        return Err(type_error(vm, "Cannot convert a BigInt value to a number"));
    }
    let n = oxide_runtime_api::to_number_full(prim, vm).map_err(|e| crate::iterator::engine_error(vm, &e))?;
    Ok(JsValue::float(n))
}

/// 元素类型是否为 BigInt 语义（BigInt64/BigUint64）。
fn is_bigint_kind(kind: TypedArrayKind) -> bool {
    matches!(kind, TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64)
}

/// 把异常 JsValue 格式化为 `Kind: message` 文本（属性写路径的 String 错误契约用）。
fn element_error_text<H: VmHost>(vm: &mut H, err: JsValue) -> String {
    if let Some(s) = vm.lookup_str(err) {
        return s;
    }
    if err.is_object() {
        let obj = unsafe { &*err.as_js_object_ptr() };
        let name_si = vm.kernel_core().perm_interner().intern("name").0;
        let message_si = vm.kernel_core().perm_interner().intern("message").0;
        let name = vm
            .resolve_property(obj, name_si)
            .and_then(|v| vm.lookup_str(v))
            .unwrap_or_else(|| "Error".to_string());
        let message = vm
            .resolve_property(obj, message_si)
            .and_then(|v| vm.lookup_str(v))
            .unwrap_or_default();
        if name.is_empty() {
            message
        } else if message.is_empty() {
            name
        } else {
            format!("{name}: {message}")
        }
    } else {
        format!("{err}")
    }
}

/// 把已按元素类型转换的值（数值 kind 为 Number、BigInt kind 为 BigInt）按位模式
/// 截断写入底层 buffer。转换由调用方 [`ta_element_value`] 完成，本函数无副作用。
fn write_element<H: VmHost>(vm: &mut H, kind: TypedArrayKind, bytes: &mut [u8], offset: usize, value: JsValue) {
    let n = oxide_runtime_api::to_number(value);
    match kind {
        TypedArrayKind::Int8 => bytes[offset] = n as i32 as u8 as i8 as u8,
        TypedArrayKind::Uint8 => bytes[offset] = n as i32 as u8,
        TypedArrayKind::Uint8Clamped => bytes[offset] = n.clamp(0.0, 255.0).round() as u8,
        TypedArrayKind::Int16 => bytes[offset..offset + 2].copy_from_slice(&(n as i32 as u16 as i16).to_ne_bytes()),
        TypedArrayKind::Uint16 => bytes[offset..offset + 2].copy_from_slice(&(n as i32 as u16).to_ne_bytes()),
        TypedArrayKind::Int32 => bytes[offset..offset + 4].copy_from_slice(&(n as i32).to_ne_bytes()),
        TypedArrayKind::Uint32 => bytes[offset..offset + 4].copy_from_slice(&(n as u32).to_ne_bytes()),
        TypedArrayKind::Float32 => bytes[offset..offset + 4].copy_from_slice(&(n as f32).to_ne_bytes()),
        TypedArrayKind::Float64 => bytes[offset..offset + 8].copy_from_slice(&n.to_ne_bytes()),
        TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64 => {
            // 取低 64 位位模式：与 2^64-1 掩码后恒非负且可转 u64，BigInt64 按位
            // 模式重解释为 i64（二进制补码）。
            let v = vm.bigint_value(value);
            let low = (v & (BigInt::from(u64::MAX)))
                .to_u64()
                .expect("与 u64::MAX 掩码后恒在 u64 范围");
            let bytes64 = if matches!(kind, TypedArrayKind::BigInt64) {
                (low as i64).to_ne_bytes()
            } else {
                low.to_ne_bytes()
            };
            bytes[offset..offset + 8].copy_from_slice(&bytes64);
        }
    }
}

/// 收集源对象的元素值。
///
/// # 步骤
/// 1. Array/TypedArray 源走元素区直读（不经迭代协议）。
/// 2. `consult_iterator` 为 true 且 `@@iterator` 解析到可调用时走迭代路径
///    （含用户自定义迭代器）；否则按 array-like 读 `length` 逐索引取值。
///
/// # 边界
/// 源须为对象，否则抛 TypeError。
fn collect_array_like<H: VmHost>(vm: &mut H, value: JsValue, consult_iterator: bool) -> Result<Vec<JsValue>, JsValue> {
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
            .map(|i| read_element(vm, view.kind, buffer, absolute_byte_offset(view, i)))
            .collect());
    }

    // 咨询迭代器的入口（构造器）仅在 @@iterator 解析到可调用时走迭代路径，
    // 否则落 array-like 读 length 逐索引取值（方法解析为 null/undefined 时同此）。
    if consult_iterator && crate::iterator::peek_iterator_method(vm, value)? {
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

    let (buffer, byte_offset, length) = if first.is_object() {
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
            let values = native_try!(collect_array_like(vm, first, true));
            let byte_len = values.len().saturating_mul(bpe);
            let buffer = JsValue::from_js_object(new_array_buffer(vm, vec![0; byte_len]));
            let buffer_ptr = native_try!(array_buffer_data_ptr(vm, buffer));
            let buffer_ref = unsafe { &mut *buffer_ptr };
            for (idx, value) in values.into_iter().enumerate() {
                let elem = native_try!(ta_element_value(vm, kind, value));
                write_element(vm, kind, buffer_ref, idx * bpe, elem);
            }
            (buffer, 0, byte_len / bpe)
        }
    } else {
        // 非对象第一参数统一按 ToIndex 语义处理（bool→0/1、null→0、BigInt→数值、
        // 字符串→解析、undefined→0）；Symbol 按规范抛 TypeError。
        if first.is_symbol() {
            return NativeResult::Err(type_error(vm, "invalid TypedArray length"));
        }
        let len = native_try!(to_index(vm, first, "invalid TypedArray length"));
        let Some(byte_len) = len.checked_mul(bpe) else {
            return NativeResult::Err(range_error(vm, "invalid TypedArray length"));
        };
        if byte_len > MAX_ARRAY_BUFFER_LENGTH {
            return NativeResult::Err(range_error(vm, "invalid TypedArray length"));
        }
        let buffer = JsValue::from_js_object(new_array_buffer(vm, vec![0; byte_len]));
        (buffer, 0, len)
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
        native_try!(ta_to_number(vm, vm.reg(args[1]))).trunc() as isize
    } else {
        0
    };
    let idx = if raw_index < 0 { view.length as isize + raw_index } else { raw_index };
    if idx < 0 || idx as usize >= view.length {
        return NativeResult::Ok(JsValue::undefined());
    }
    let buffer_ptr = native_try!(array_buffer_data_ptr(vm, view.buffer));
    let buffer = unsafe { &*buffer_ptr };
    NativeResult::Ok(read_element(vm, view.kind, buffer, absolute_byte_offset(view, idx as usize)))
}

/// `TypedArray.prototype.fill(value, start, end)`：用给定值填充区间，返回 this。
pub fn typed_array_fill<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    // 值只转换一次（valueOf 副作用一次），转换结果写入每个目标元素。
    let elem = native_try!(ta_element_value(vm, view.kind, value));
    let start = if args.len() > 2 {
        native_try!(normalize_index(vm, vm.reg(args[2]), view.length))
    } else {
        0
    };
    let end = if args.len() > 3 && !vm.reg(args[3]).is_undefined() {
        native_try!(normalize_index(vm, vm.reg(args[3]), view.length))
    } else {
        view.length
    };
    let buffer_ptr = native_try!(array_buffer_data_ptr(vm, view.buffer));
    let buffer = unsafe { &mut *buffer_ptr };
    for idx in start..end.max(start) {
        write_element(vm, view.kind, buffer, absolute_byte_offset(view, idx), elem);
    }
    NativeResult::Ok(this_val)
}

/// `TypedArray.prototype.slice(start, end)`：复制区间元素生成新同类型 TypedArray。
pub fn typed_array_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let start = if args.len() > 1 {
        native_try!(normalize_index(vm, vm.reg(args[1]), view.length))
    } else {
        0
    };
    let end = if args.len() > 2 && !vm.reg(args[2]).is_undefined() {
        native_try!(normalize_index(vm, vm.reg(args[2]), view.length))
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
        native_try!(normalize_index(vm, vm.reg(args[1]), view.length))
    } else {
        0
    };
    let end = if args.len() > 2 && !vm.reg(args[2]).is_undefined() {
        native_try!(normalize_index(vm, vm.reg(args[2]), view.length))
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
///
/// # 边界
/// 源为 TypedArray 时直读元素区；否则恒按 array-like 索引读
/// （不咨询源的 `@@iterator`，即便其可调用）。
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
    let values = native_try!(collect_array_like(vm, source, false));
    if offset > view.length || values.len() > view.length - offset {
        return NativeResult::Err(range_error(vm, "TypedArray.set offset out of bounds"));
    }
    let mut converted = Vec::with_capacity(values.len());
    for v in values {
        converted.push(native_try!(ta_element_value(vm, view.kind, v)));
    }
    let buffer_ptr = native_try!(array_buffer_data_ptr(vm, view.buffer));
    let buffer = unsafe { &mut *buffer_ptr };
    for (i, elem) in converted.into_iter().enumerate() {
        write_element(vm, view.kind, buffer, absolute_byte_offset(view, offset + i), elem);
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

/// 按 IntegerIndexedElementSet 语义把元素写入 TypedArray：先按元素类型转换
/// （BigInt 类型走 ToBigInt、数值类型走 ToNumber，symbol 等不可转换值抛 TypeError）。
fn set_typed_array_element<H: VmHost>(vm: &mut H, ta: JsValue, index: usize, value: JsValue) -> Result<(), JsValue> {
    let view = get_typed_array_data(vm, ta)?;
    if index >= view.length {
        return Ok(());
    }
    let elem = ta_element_value(vm, view.kind, value)?;
    let buffer_ptr = array_buffer_data_ptr(vm, view.buffer)?;
    let buffer = unsafe { &mut *buffer_ptr };
    write_element(vm, view.kind, buffer, absolute_byte_offset(view, index), elem);
    Ok(())
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

/// 读 TypedArray 指定索引的元素（视图 data 已取出的形式，供原型方法内部使用）；
/// 越界返回 undefined。
fn ta_read<H: VmHost>(vm: &mut H, view: TypedArrayData, index: usize) -> Result<JsValue, JsValue> {
    if index >= view.length {
        return Ok(JsValue::undefined());
    }
    let buffer_ptr = array_buffer_data_ptr(vm, view.buffer)?;
    let buffer = unsafe { &*buffer_ptr };
    Ok(read_element(vm, view.kind, buffer, absolute_byte_offset(view, index)))
}

/// 把已转换的值写入 TypedArray 指定索引（视图 data 已取出的形式，供原型方法内部使用）。
fn ta_write<H: VmHost>(vm: &mut H, view: TypedArrayData, index: usize, value: JsValue) -> Result<(), JsValue> {
    let buffer_ptr = array_buffer_data_ptr(vm, view.buffer)?;
    let buffer = unsafe { &mut *buffer_ptr };
    write_element(vm, view.kind, buffer, absolute_byte_offset(view, index), value);
    Ok(())
}

/// 把一组已按元素类型转换的值写成同类型的新 TypedArray
/// （map/filter/toReversed/toSorted/with 共用）。
fn create_ta_from_values<H: VmHost>(
    vm: &mut H, kind: TypedArrayKind, values: Vec<JsValue>,
) -> Result<*mut JsObject, JsValue> {
    let bpe = kind.bytes_per_element();
    let len = values.len();
    let buffer = JsValue::from_js_object(new_array_buffer(vm, vec![0; len * bpe]));
    let buffer_ptr = array_buffer_data_ptr(vm, buffer)?;
    let buffer_ref = unsafe { &mut *buffer_ptr };
    for (idx, v) in values.into_iter().enumerate() {
        write_element(vm, kind, buffer_ref, idx * bpe, v);
    }
    Ok(create_typed_array(vm, kind, buffer, 0, len))
}

/// 调用 TypedArray 回调并收敛异常：tail call 一律视为内部错误。
fn invoke_cb<H: VmHost>(vm: &mut H, cb: JsValue, this_arg: JsValue, cb_args: &[JsValue]) -> Result<JsValue, JsValue> {
    match crate::array::invoke_native_callback(vm, cb, this_arg, cb_args) {
        NativeResult::Ok(v) => Ok(v),
        NativeResult::Err(e) => Err(e),
        NativeResult::TailCall { .. } => Err(type_error(vm, "unexpected tail call in TypedArray callback")),
    }
}

/// ToNumber 并恢复引擎错误为原始异常值（索引类参数转换用；元素转换走
/// [`ta_element_value`] 按类型分流）。
fn ta_to_number<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    oxide_runtime_api::to_number_full(value, vm).map_err(|e| crate::iterator::engine_error(vm, &e))
}

/// `%TypedArray%.prototype.forEach(callback, thisArg)`：对每个元素调用 callback，返回 undefined。
pub fn typed_array_for_each<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `%TypedArray%.prototype.map(callback, thisArg)`：对每个元素调用 callback，
/// 结果按元素类型转换后写入同类型的新 TypedArray。
pub fn typed_array_map<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mut values = Vec::with_capacity(view.length);
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        let mapped = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        values.push(native_try!(ta_element_value(vm, view.kind, mapped)));
    }
    let new_obj = native_try!(create_ta_from_values(vm, view.kind, values));
    NativeResult::Ok(JsValue::from_js_object(new_obj))
}

/// `%TypedArray%.prototype.filter(callback, thisArg)`：保留 callback 为真的元素，
/// 组成同类型的新 TypedArray。
pub fn typed_array_filter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mut values = Vec::new();
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        let result = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        if oxide_runtime_api::to_boolean(result) {
            values.push(elem);
        }
    }
    let new_obj = native_try!(create_ta_from_values(vm, view.kind, values));
    NativeResult::Ok(JsValue::from_js_object(new_obj))
}

/// `%TypedArray%.prototype.reduce(callback, initialValue)`：从左到右累计归约；
/// 空数组且无初始值抛 TypeError。
pub fn typed_array_reduce<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if view.length == 0 && args.len() < 3 {
        return NativeResult::Err(type_error(vm, "Reduce of empty array with no initial value"));
    }
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let (mut accumulator, start_idx) = if args.len() > 2 {
        (vm.reg(args[2]), 0usize)
    } else {
        (native_try!(ta_read(vm, view, 0)), 1)
    };
    for i in start_idx..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        accumulator = native_try!(invoke_cb(
            vm,
            callback,
            JsValue::undefined(),
            &[accumulator, elem, JsValue::int(i as i32), this_val],
        ));
    }
    NativeResult::Ok(accumulator)
}

/// `%TypedArray%.prototype.reduceRight(callback, initialValue)`：从右到左累计归约。
pub fn typed_array_reduce_right<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if view.length == 0 && args.len() < 3 {
        return NativeResult::Err(type_error(vm, "Reduce of empty array with no initial value"));
    }
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let (mut accumulator, start_idx): (JsValue, i32) = if args.len() > 2 {
        (vm.reg(args[2]), view.length as i32 - 1)
    } else {
        (native_try!(ta_read(vm, view, view.length - 1)), view.length as i32 - 2)
    };
    for i in (0..=start_idx).rev() {
        let elem = native_try!(ta_read(vm, view, i as usize));
        accumulator = native_try!(invoke_cb(
            vm,
            callback,
            JsValue::undefined(),
            &[accumulator, elem, JsValue::int(i), this_val],
        ));
    }
    NativeResult::Ok(accumulator)
}

/// `%TypedArray%.prototype.every(callback, thisArg)`：所有元素满足 callback 才返回 true。
pub fn typed_array_every<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        let result = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        if !oxide_runtime_api::to_boolean(result) {
            return NativeResult::Ok(JsValue::bool(false));
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

/// `%TypedArray%.prototype.some(callback, thisArg)`：任一元素满足 callback 返回 true。
pub fn typed_array_some<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        let result = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        if oxide_runtime_api::to_boolean(result) {
            return NativeResult::Ok(JsValue::bool(true));
        }
    }
    NativeResult::Ok(JsValue::bool(false))
}

/// `%TypedArray%.prototype.find(callback, thisArg)`：返回首个 callback 为真的元素，否则 undefined。
pub fn typed_array_find<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        let result = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        if oxide_runtime_api::to_boolean(result) {
            return NativeResult::Ok(elem);
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `%TypedArray%.prototype.findIndex(callback, thisArg)`：返回首个 callback 为真的索引，否则 -1。
pub fn typed_array_find_index<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        let result = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        if oxide_runtime_api::to_boolean(result) {
            return NativeResult::Ok(JsValue::int(i as i32));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `%TypedArray%.prototype.findLast(callback, thisArg)`：从后往前返回首个 callback 为真的元素。
pub fn typed_array_find_last<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in (0..view.length).rev() {
        let elem = native_try!(ta_read(vm, view, i));
        let result = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        if oxide_runtime_api::to_boolean(result) {
            return NativeResult::Ok(elem);
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `%TypedArray%.prototype.findLastIndex(callback, thisArg)`：从后往前返回首个 callback
/// 为真的索引，否则 -1。
pub fn typed_array_find_last_index<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    for i in (0..view.length).rev() {
        let elem = native_try!(ta_read(vm, view, i));
        let result = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        if oxide_runtime_api::to_boolean(result) {
            return NativeResult::Ok(JsValue::int(i as i32));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// 规范化 indexOf/includes 的 fromIndex：NaN 视为 0，负值折算后从 0 夹取，正值夹到长度。
fn normalize_from_index<H: VmHost>(vm: &mut H, value: JsValue, len: usize) -> Result<usize, JsValue> {
    let n = match vm.coerce_number_bounded(value) {
        Ok(n) => n,
        Err(e) => return Err(crate::iterator::engine_error(vm, &e)),
    };
    if n.is_nan() {
        return Ok(0);
    }
    let int = n.trunc();
    if int >= 0.0 {
        Ok((int as usize).min(len))
    } else {
        let from = len as f64 + int;
        if from < 0.0 {
            Ok(0)
        } else {
            Ok(from as usize)
        }
    }
}

/// `%TypedArray%.prototype.indexOf(searchElement, fromIndex)`：用严格相等查找首个匹配索引。
pub fn typed_array_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if view.length == 0 || args.len() < 2 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let target = vm.reg(args[1]);
    let from_index = if args.len() >= 3 {
        native_try!(normalize_from_index(vm, vm.reg(args[2]), view.length))
    } else {
        0
    };
    for i in from_index..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        if oxide_runtime_api::strict_equality(elem, target) {
            return NativeResult::Ok(JsValue::int(i as i32));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `%TypedArray%.prototype.lastIndexOf(searchElement, fromIndex)`：从后往前查找首个匹配索引。
pub fn typed_array_last_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if view.length == 0 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let target = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let from_index: isize = if args.len() >= 3 {
        let v = vm.reg(args[2]);
        let f = match vm.coerce_number_bounded(v) {
            Ok(n) => n,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
        if f.is_nan() {
            return NativeResult::Ok(JsValue::int(-1));
        }
        let f = f.trunc();
        if f >= 0.0 {
            (f as isize).min(view.length as isize - 1)
        } else {
            view.length as isize + f as isize
        }
    } else {
        view.length as isize - 1
    };
    if from_index < 0 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    for i in (0..=from_index as usize).rev() {
        let elem = native_try!(ta_read(vm, view, i));
        if oxide_runtime_api::strict_equality(elem, target) {
            return NativeResult::Ok(JsValue::int(i as i32));
        }
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `%TypedArray%.prototype.includes(searchElement, fromIndex)`：用 SameValueZero 判断是否包含
/// （NaN 视为存在、+0/-0 视为相同）。
pub fn typed_array_includes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if view.length == 0 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let target = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let from_index = if args.len() >= 3 {
        native_try!(normalize_from_index(vm, vm.reg(args[2]), view.length))
    } else {
        0
    };
    for i in from_index..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        if oxide_runtime_api::same_value_zero(elem, target) {
            return NativeResult::Ok(JsValue::bool(true));
        }
    }
    NativeResult::Ok(JsValue::bool(false))
}

/// `%TypedArray%.prototype.join(separator)`：用分隔符连接元素字符串（元素恒为数值）。
pub fn typed_array_join<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let sep = if args.len() > 1 {
        native_try!(
            oxide_runtime_api::to_string_full(vm.reg(args[1]), vm).map_err(|e| crate::iterator::engine_error(vm, &e))
        )
    } else {
        ",".to_string()
    };
    let mut parts = Vec::with_capacity(view.length);
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        parts.push(oxide_runtime_api::to_string(elem));
    }
    NativeResult::Ok(vm.new_string(&parts.join(&sep)))
}

/// `%TypedArray%.prototype.values()`：返回迭代元素值的迭代器。
pub fn typed_array_values<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    native_try!(get_typed_array_data(vm, this_val));
    // TA 迭代器与 Array 共享 %ArrayIteratorPrototype%（规范 CreateArrayIterator
    // 同族），next 由原型提供；接收者合法性已在上方校验。
    match crate::array::make_array_iterator(vm, this_val, crate::array::ARRAY_ITER_KIND_VALUES) {
        Ok(iter) => NativeResult::Ok(iter),
        Err(err) => NativeResult::Err(err),
    }
}

/// `%TypedArray%.prototype.keys()`：返回迭代元素索引的迭代器。
pub fn typed_array_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    native_try!(get_typed_array_data(vm, this_val));
    match crate::array::make_array_iterator(vm, this_val, crate::array::ARRAY_ITER_KIND_KEYS) {
        Ok(iter) => NativeResult::Ok(iter),
        Err(err) => NativeResult::Err(err),
    }
}

/// `%TypedArray%.prototype.entries()`：返回迭代 `[index, element]` 对的迭代器。
pub fn typed_array_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    native_try!(get_typed_array_data(vm, this_val));
    match crate::array::make_array_iterator(vm, this_val, crate::array::ARRAY_ITER_KIND_ENTRIES) {
        Ok(iter) => NativeResult::Ok(iter),
        Err(err) => NativeResult::Err(err),
    }
}

/// 默认数值排序比较：NaN 视为最大排到末尾，其余按数值升序。
fn default_ta_order(a: f64, b: f64) -> std::cmp::Ordering {
    if a.is_nan() && b.is_nan() {
        std::cmp::Ordering::Equal
    } else if a.is_nan() {
        std::cmp::Ordering::Greater
    } else if b.is_nan() {
        std::cmp::Ordering::Less
    } else {
        a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// `%TypedArray%.prototype.sort(comparefn)`：原地排序，默认按数值升序（NaN 排末尾；
/// BigInt 类型按 BigInt 值升序）。比较器回调收到的是元素原值（BigInt 类型为
/// BigInt，数值类型为 Number）。
pub fn typed_array_sort<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let comparator = if args.len() > 1 {
        let c = vm.reg(args[1]);
        if c.is_undefined() {
            None
        } else {
            Some(native_try!(crate::array::require_callback(vm, c)))
        }
    } else {
        None
    };
    let mut vals: Vec<JsValue> = Vec::with_capacity(view.length);
    for i in 0..view.length {
        vals.push(native_try!(ta_read(vm, view, i)));
    }
    let mut sort_error = None;
    vals.sort_by(|a, b| {
        if sort_error.is_some() {
            return std::cmp::Ordering::Equal;
        }
        if let Some(cb) = comparator {
            match crate::array::invoke_native_callback(vm, cb, JsValue::undefined(), &[*a, *b]) {
                NativeResult::Ok(r) => {
                    let n = oxide_runtime_api::to_number(r);
                    if n.is_nan() || n == 0.0 {
                        std::cmp::Ordering::Equal
                    } else if n < 0.0 {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
                NativeResult::Err(err) => {
                    sort_error = Some(err);
                    std::cmp::Ordering::Equal
                }
                NativeResult::TailCall { .. } => {
                    sort_error =
                        Some(crate::error::create_type_error(vm, "unexpected tail call in typed array callback"));
                    std::cmp::Ordering::Equal
                }
            }
        } else if view.kind == TypedArrayKind::BigInt64 {
            // BigInt64 元素恒在 i64 范围（写入按 mod 2^64 截断为有符号位模式），按有符号序比较。
            let a_int = vm.bigint_value(*a).to_i64().unwrap_or(0);
            let b_int = vm.bigint_value(*b).to_i64().unwrap_or(0);
            a_int.cmp(&b_int)
        } else if is_bigint_kind(view.kind) {
            // BigUint64 元素恒在 u64 范围，按数值序比较。
            let a_uint = vm.bigint_value(*a).to_u64().unwrap_or(0);
            let b_uint = vm.bigint_value(*b).to_u64().unwrap_or(0);
            a_uint.cmp(&b_uint)
        } else {
            default_ta_order(oxide_runtime_api::to_number(*a), oxide_runtime_api::to_number(*b))
        }
    });
    if let Some(err) = sort_error {
        return NativeResult::Err(err);
    }
    for (i, v) in vals.into_iter().enumerate() {
        native_try!(ta_write(vm, view, i, v));
    }
    NativeResult::Ok(this_val)
}

/// `%TypedArray%.prototype.reverse()`：原地反转元素顺序，返回 this。
pub fn typed_array_reverse<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let mut i = 0;
    let mut j = view.length.saturating_sub(1);
    while i < j {
        let tmp = native_try!(ta_read(vm, view, i));
        let jtmp = native_try!(ta_read(vm, view, j));
        native_try!(ta_write(vm, view, j, tmp));
        native_try!(ta_write(vm, view, i, jtmp));
        i += 1;
        j = j.saturating_sub(1);
    }
    NativeResult::Ok(this_val)
}

/// `%TypedArray%.prototype.copyWithin(target, start, end)`：在数组内部复制元素区间，返回 this。
pub fn typed_array_copy_within<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let len = view.length;
    let target = if args.len() > 1 {
        native_try!(normalize_index(vm, vm.reg(args[1]), len))
    } else {
        0
    };
    let start = if args.len() > 2 {
        native_try!(normalize_index(vm, vm.reg(args[2]), len))
    } else {
        0
    };
    let end = if args.len() > 3 && !vm.reg(args[3]).is_undefined() {
        native_try!(normalize_index(vm, vm.reg(args[3]), len))
    } else {
        len
    };
    let count = end.max(start).saturating_sub(start).min(len.saturating_sub(target));
    // 目标区间前移与源区间重叠时逆序遍历，避免覆盖未读的源元素。
    let (mut from, mut to, direction) = if start < target && target < start + count {
        (start + count - 1, target + count - 1, -1isize)
    } else {
        (start, target, 1isize)
    };
    for _ in 0..count {
        let elem = native_try!(ta_read(vm, view, from));
        native_try!(ta_write(vm, view, to, elem));
        from = (from as isize + direction) as usize;
        to = (to as isize + direction) as usize;
    }
    NativeResult::Ok(this_val)
}

/// 以元素为 receiver 调用其 `toLocaleString` 方法，返回结果（失败透传原异常）。
///
/// # 注意事项
/// `toLocaleString` 方法缺失时回退为 ToString 结果（无 Intl 的最小实现）。
fn invoke_element_to_locale_string<H: VmHost>(vm: &mut H, element: JsValue) -> Result<JsValue, JsValue> {
    let obj_val = oxide_runtime_api::to_object(element, vm).map_err(|e| crate::iterator::engine_error(vm, &e))?;
    let obj = unsafe { &*obj_val.as_js_object_ptr() };
    let method_si = vm.kernel_core().perm_interner().intern("toLocaleString").0;
    let method = vm
        .ordinary_get(obj, method_si, element)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    if !crate::iterator::is_callable(method) {
        return Ok(vm.new_string(&oxide_runtime_api::to_string(element)));
    }
    match vm.call_function_sync(method, element, &[]) {
        Ok(r) => Ok(r),
        Err(e) => Err(crate::iterator::engine_error(vm, &e)),
    }
}

/// `%TypedArray%.prototype.toLocaleString()`：元素逐个调用 `toLocaleString` 后用 `,` 连接。
pub fn typed_array_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if view.length == 0 {
        return NativeResult::Ok(vm.new_string(""));
    }
    let mut parts = Vec::with_capacity(view.length);
    for i in 0..view.length {
        let elem = native_try!(ta_read(vm, view, i));
        let r = native_try!(invoke_element_to_locale_string(vm, elem));
        let s =
            native_try!(oxide_runtime_api::to_string_full(r, vm).map_err(|e| crate::iterator::engine_error(vm, &e)));
        parts.push(s);
    }
    NativeResult::Ok(vm.new_string(&parts.join(",")))
}

/// `%TypedArray%.prototype.toReversed()`：返回元素反转的同类型新 TypedArray（原对象不变）。
pub fn typed_array_to_reversed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let mut values = Vec::with_capacity(view.length);
    for i in (0..view.length).rev() {
        values.push(native_try!(ta_read(vm, view, i)));
    }
    let new_obj = native_try!(create_ta_from_values(vm, view.kind, values));
    NativeResult::Ok(JsValue::from_js_object(new_obj))
}

/// `%TypedArray%.prototype.toSorted(comparefn)`：返回元素排序后的同类型新 TypedArray
/// （原对象不变）。
pub fn typed_array_to_sorted<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let comparator = if args.len() > 1 {
        let c = vm.reg(args[1]);
        if c.is_undefined() {
            None
        } else {
            Some(native_try!(crate::array::require_callback(vm, c)))
        }
    } else {
        None
    };
    let mut vals: Vec<JsValue> = Vec::with_capacity(view.length);
    for i in 0..view.length {
        vals.push(native_try!(ta_read(vm, view, i)));
    }
    let mut sort_error = None;
    vals.sort_by(|a, b| {
        if sort_error.is_some() {
            return std::cmp::Ordering::Equal;
        }
        if let Some(cb) = comparator {
            match crate::array::invoke_native_callback(vm, cb, JsValue::undefined(), &[*a, *b]) {
                NativeResult::Ok(r) => {
                    let n = oxide_runtime_api::to_number(r);
                    if n.is_nan() || n == 0.0 {
                        std::cmp::Ordering::Equal
                    } else if n < 0.0 {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
                NativeResult::Err(err) => {
                    sort_error = Some(err);
                    std::cmp::Ordering::Equal
                }
                NativeResult::TailCall { .. } => {
                    sort_error =
                        Some(crate::error::create_type_error(vm, "unexpected tail call in typed array callback"));
                    std::cmp::Ordering::Equal
                }
            }
        } else if view.kind == TypedArrayKind::BigInt64 {
            let a_int = vm.bigint_value(*a).to_i64().unwrap_or(0);
            let b_int = vm.bigint_value(*b).to_i64().unwrap_or(0);
            a_int.cmp(&b_int)
        } else if is_bigint_kind(view.kind) {
            let a_uint = vm.bigint_value(*a).to_u64().unwrap_or(0);
            let b_uint = vm.bigint_value(*b).to_u64().unwrap_or(0);
            a_uint.cmp(&b_uint)
        } else {
            default_ta_order(oxide_runtime_api::to_number(*a), oxide_runtime_api::to_number(*b))
        }
    });
    if let Some(err) = sort_error {
        return NativeResult::Err(err);
    }
    let new_obj = native_try!(create_ta_from_values(vm, view.kind, vals));
    NativeResult::Ok(JsValue::from_js_object(new_obj))
}

/// `%TypedArray%.prototype.with(index, value)`：返回替换指定索引元素后的同类型新 TypedArray；
/// 负索引从尾部折算，折算后越界抛 RangeError。
pub fn typed_array_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "TypedArray.prototype.with requires an index"));
    }
    // ToIntegerOrInfinity(index)，负值折算为 len + index。
    let raw = native_try!(ta_to_number(vm, vm.reg(args[1])));
    let relative_index = if raw.is_nan() || raw == 0.0 {
        0.0
    } else if raw.is_infinite() {
        raw
    } else {
        raw.trunc()
    };
    let actual_index = if relative_index >= 0.0 {
        relative_index
    } else {
        view.length as f64 + relative_index
    };
    // 先按元素类型转换 value（可触发副作用/抛错），再做索引范围校验。
    let value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let replacement = native_try!(ta_element_value(vm, view.kind, value));
    if actual_index.is_nan() || actual_index < 0.0 || actual_index >= view.length as f64 {
        return NativeResult::Err(range_error(vm, "Invalid typed array index"));
    }
    let index = actual_index as usize;
    let mut values = Vec::with_capacity(view.length);
    for i in 0..view.length {
        if i == index {
            values.push(replacement);
            continue;
        }
        values.push(native_try!(ta_read(vm, view, i)));
    }
    let new_obj = native_try!(create_ta_from_values(vm, view.kind, values));
    NativeResult::Ok(JsValue::from_js_object(new_obj))
}

/// `%TypedArray%.from(source, mapfn?, thisArg?)`：从可迭代对象或 array-like 构造
/// 以 `this`（构造器 C）为类型的 TypedArray；`mapfn` 逐元素映射（`thisArg` 作回调
/// this），元素按结果类型转换（BigInt 类型走 ToBigInt、数值类型走 ToNumber）写入。
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
