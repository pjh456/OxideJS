use num_bigint::BigInt;
use num_traits::ToPrimitive;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, TypedArrayKind};
use oxide_types::private_key::{int_key_value, is_int_key, make_well_known_symbol_key, WELL_KNOWN_SYMBOL_SPECIES};
use oxide_types::value::JsValue;

use crate::array_buffer::{
    buffer_payload, buffer_payload_ptr, default_array_buffer_proto, new_array_buffer, MAX_ARRAY_BUFFER_LENGTH,
};

use oxide_runtime_api::{NativeResult, VmHost};

#[derive(Clone, Copy)]
pub(crate) struct TypedArrayData {
    pub kind: TypedArrayKind,
    pub buffer: JsValue,
    pub byte_offset: usize,
    /// `length` 的两种口径：`auto_length` 为 true（构造省略长度、subarray 省略
    /// end 落于可缩放缓冲）时视图长度随底层 buffer 实时伸缩；false 时
    /// `length` 为定长，buffer 收缩只触发越界，不改长度。
    pub length: usize,
    pub auto_length: bool,
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

/// 取指定类型的内建构造器值（`[[TypedArrayName]]` → world 槽），species 默认臂
/// 的构造目标。
fn typed_array_ctor_value<H: VmHost>(vm: &mut H, kind: TypedArrayKind) -> JsValue {
    let world = vm.session().builtin_world();
    let ctor = match kind {
        TypedArrayKind::Int8 => world.int8array_constructor.clone(),
        TypedArrayKind::Uint8 => world.uint8array_constructor.clone(),
        TypedArrayKind::Uint8Clamped => world.uint8clampedarray_constructor.clone(),
        TypedArrayKind::Int16 => world.int16array_constructor.clone(),
        TypedArrayKind::Uint16 => world.uint16array_constructor.clone(),
        TypedArrayKind::Int32 => world.int32array_constructor.clone(),
        TypedArrayKind::Uint32 => world.uint32array_constructor.clone(),
        TypedArrayKind::Float32 => world.float32array_constructor.clone(),
        TypedArrayKind::Float64 => world.float64array_constructor.clone(),
        TypedArrayKind::BigInt64 => world.bigint64array_constructor.clone(),
        TypedArrayKind::BigUint64 => world.biguint64array_constructor.clone(),
    };
    JsValue::from_js_object(ctor.as_ptr() as *mut JsObject)
}

/// 把 TypedArray 实例数据（类型标签 + 视图内部槽）写入指定对象；构造调用与
/// 普通调用建对象共用同一物化路径。
///
/// # 边界与前提
/// - 对象须为空槽位（构造帧分配的 this / 新建对象）；重复物化会泄漏旧数据盒。
fn materialize_typed_array(
    obj: &mut JsObject, kind: TypedArrayKind, buffer: JsValue, byte_offset: usize, length: usize, auto_length: bool,
) {
    obj.type_tag = JsObject::OBJ_TYPE_TYPED_ARRAY;
    let data = Box::into_raw(Box::new(TypedArrayData {
        kind,
        buffer,
        byte_offset,
        length,
        auto_length,
    }));
    // SAFETY: TypedArray 实例不可调用，native_fn 存不透明 `Box<TypedArrayData>`，
    // 与本 VM 中 ArrayBuffer/DataView 的类型化对象存储一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(data as *const ()) }));
}

fn create_typed_array<H: VmHost>(
    vm: &mut H, kind: TypedArrayKind, buffer: JsValue, byte_offset: usize, length: usize, auto_length: bool,
) -> *mut JsObject {
    let proto = typed_array_proto_ptr(vm, kind);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    materialize_typed_array(&mut obj, kind, buffer, byte_offset, length, auto_length);
    vm.alloc_object(obj)
}

/// TypedArrayCreateSameType 单长度参数形态：按 O 的类型建同长度新对象
/// （规范上忽略 species，toReversed/toSorted/with 的交付语义）。定长新缓冲，
/// 不随任何 buffer 伸缩。
fn create_same_type_typed_array<H: VmHost>(vm: &mut H, kind: TypedArrayKind, length: usize) -> *mut JsObject {
    let bpe = kind.bytes_per_element();
    let buffer =
        JsValue::from_js_object(new_array_buffer(vm, vec![0; length * bpe], 0, default_array_buffer_proto(vm)));
    create_typed_array(vm, kind, buffer, 0, length, false)
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

/// 读视图引用 buffer 的当前字节长度；ArrayBuffer/SharedArrayBuffer 双认，
/// buffer 两标签之外、无载荷或已 detach（`data` 为 `None`）时返回 `None`。
fn ta_buffer_byte_length(view: TypedArrayData) -> Option<usize> {
    let buffer_ptr = view.buffer.as_js_object_ptr();
    if buffer_ptr.is_null() {
        return None;
    }
    // SAFETY: buffer 对象与视图同生命周期，此处只读载荷存活位与长度。
    let payload_ptr = buffer_payload_ptr(unsafe { &*buffer_ptr })?;
    if payload_ptr.is_null() {
        return None;
    }
    // SAFETY: payload_ptr 经 buffer_payload_ptr 校验为合法载荷盒。
    unsafe { (*payload_ptr).data.as_ref().map(|data| data.len()) }
}

/// 规范 TypedArrayLength：已 detach 恒 0；定长视图取静态长度（哪怕 buffer
/// 已收缩到越界）；auto 视图按 buffer 当前字节数实时折算。
fn ta_spec_length(view: TypedArrayData) -> usize {
    let buffer_len = ta_buffer_byte_length(view);
    if view.auto_length {
        return buffer_len
            .map(|len| len.saturating_sub(view.byte_offset) / view.kind.bytes_per_element())
            .unwrap_or(0);
    }
    if buffer_len.is_none() {
        return 0;
    }
    view.length
}

/// 越界判定：detach 恒越界；auto 视图 offset 超 buffer 即越界；定长视图
/// 窗口尾部超 buffer 即越界。
fn ta_is_oob(view: TypedArrayData) -> bool {
    let Some(buffer_len) = ta_buffer_byte_length(view) else {
        return true;
    };
    if view.auto_length {
        return view.byte_offset > buffer_len;
    }
    view.byte_offset.saturating_add(view.length * view.kind.bytes_per_element()) > buffer_len
}

/// live 长度（元素面消费口径）：越界（含 detach）一律 0，界内取规范
/// TypedArrayLength。
pub(crate) fn ta_live_length(view: TypedArrayData) -> usize {
    if ta_is_oob(view) {
        return 0;
    }
    ta_spec_length(view)
}

/// ValidateTypedArray 入口校验：越界视图（含 detach）抛 TypeError；原地
/// 写方法（`writable`）加查 buffer 写守卫（immutable 抛 TypeError），只读
/// 方法不查。
pub(crate) fn ta_validate<H: VmHost>(
    vm: &mut H, view: TypedArrayData, writable: bool,
) -> Result<TypedArrayData, JsValue> {
    if ta_is_oob(view) {
        return Err(type_error(vm, "TypedArray is detached or out of bounds"));
    }
    if writable {
        let buffer_ptr = view.buffer.as_js_object_ptr();
        if !buffer_ptr.is_null() {
            // SAFETY: buffer 对象与视图同生命周期，此处只读写守卫位。
            if let Some(payload_ptr) = buffer_payload_ptr(unsafe { &*buffer_ptr }) {
                if !payload_ptr.is_null() && unsafe { (*payload_ptr).immutable } {
                    return Err(type_error(vm, "ArrayBuffer is immutable"));
                }
            }
        }
    }
    Ok(view)
}

/// ToIntegerOrInfinity 裸转换（不夹取）：NaN → +0，±Infinity 保留，有限值
/// 截断；转换异常（valueOf 抛错）原值上抛。
pub(crate) fn ta_to_integer_or_infinity<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    let n = match vm.coerce_number_bounded(value) {
        Ok(n) => n,
        Err(e) => return Err(crate::iterator::engine_error(vm, &e)),
    };
    Ok(if n.is_nan() { 0.0 } else { n.trunc() })
}

/// 把 ToIntegerOrInfinity 结果夹到 `[0, len]`：+Infinity → len，负值从尾部
/// 折算（max(len + k, 0)，slice/fill/copyWithin/subarray 共用语义），正值夹
/// 到 len。
fn clamp_index_to_len(raw: f64, len: usize) -> usize {
    if raw.is_infinite() {
        return if raw > 0.0 { len } else { 0 };
    }
    let int = raw as isize;
    if int < 0 {
        len.saturating_sub(int.unsigned_abs())
    } else {
        (int as usize).min(len)
    }
}

/// 读 TypedArray 指定整数索引的元素（供 VM 普通属性 get 的 typed 分支使用）。
/// 越界（含 detach）一律返回 undefined（规范 [[Get]] 静默臂）；内部状态
/// 非法返回 Err(String)（防御背板）。
pub fn typed_array_element_get<H: VmHost>(vm: &mut H, obj: &JsObject, index: u32) -> Result<JsValue, String> {
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = get_typed_array_data(vm, this_val).map_err(|e| format!("{e}"))?;
    // live 边界：buffer 收缩越界后界内读 undefined（detached 同口径）。
    if index as usize >= ta_live_length(view) {
        return Ok(JsValue::undefined());
    }
    let buffer_ptr = view.buffer.as_js_object_ptr();
    if buffer_ptr.is_null() {
        return Err("TypedArray buffer internal state invalid".to_string());
    }
    // SAFETY: buffer 对象与视图同生命周期，此处只读载荷存活位与字节切片。
    let Some(payload_ptr) = buffer_payload_ptr(unsafe { &*buffer_ptr }) else {
        // 防御背板：构造期 buffer 必为 ArrayBuffer/SharedArrayBuffer 之一。
        return Err("TypedArray buffer internal state invalid".to_string());
    };
    let Some(buffer) = unsafe { &*payload_ptr }.data.as_deref() else {
        // 载荷缺失即 detach（live 界判后不可达，防御背板）：按 [[Get]] 读 undefined。
        return Ok(JsValue::undefined());
    };
    Ok(read_element(vm, view.kind, buffer, absolute_byte_offset(view, index as usize)))
}

// ── 统一数值键门 ─────────────────────────────────────────────────────────

/// TypedArray 统一数值键门三态：`Ordinary` 非数字串（走普通属性路径）；
/// `NumericInvalid` 数字无效（负/分数/±Infinity/NaN/越界，含 "-0" 特例）；
/// `NumericValid` 界内整数索引（携带索引）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaIndexGate {
    Ordinary,
    NumericInvalid,
    NumericValid(u32),
}

/// 统一数值键门：消费方唯一入口。谓词 `P === ToString(ToNumber(P))` 全形态
/// round-trip（"NaN"/±"Infinity" 天然归数字无效臂），`"-0"`（trim 后）独立
/// 特例归数字无效；整数键免字符串解析短路。内部取 length 值传递，消费方
/// 零视图接触。
///
/// # 边界与前提
/// - 非 TypedArray 对象、symbol 键与查不到的字符串键一律 `Ordinary`（防御，
///   调用方均已先判）。
pub fn ta_index_gate<H: VmHost>(vm: &H, obj: &JsObject, key_si: u32) -> TaIndexGate {
    if !obj.is_typed_array_obj() {
        return TaIndexGate::Ordinary;
    }
    let length = ta_view_length(vm, obj);
    if is_int_key(key_si) {
        return ta_index_gate_int(int_key_value(key_si), length);
    }
    let Some(key) = vm.kernel_core().perm_interner().lookup(key_si) else {
        return TaIndexGate::Ordinary;
    };
    ta_index_gate_from_text(key, length)
}

/// 取 TypedArray 视图的 live 长度（供只需 length 不需门的枚举面与
/// receiver 界判消费方）；非 TypedArray 或内部状态无效返回 0。越界
/// （detach、定长窗口被 buffer 收缩裁掉、auto offset 超 buffer）统一 0，
/// 界内键经门归数字无效。
pub fn ta_view_length<H: VmHost>(_vm: &H, obj: &JsObject) -> usize {
    let Some(ptr) = typed_array_data_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: ptr 非空，为 TypedArray 对象 native_fn 槽内的 Box<TypedArrayData>，
    // 与对象同生命周期，此处只读拷贝视图字段。
    let view = unsafe { *ptr };
    ta_live_length(view)
}

/// 纯核（无 VM）整数键臂：界内有效，越界数字无效（length 0 时全无效）。
pub(crate) fn ta_index_gate_int(index: u32, length: usize) -> TaIndexGate {
    if (index as usize) < length {
        TaIndexGate::NumericValid(index)
    } else {
        TaIndexGate::NumericInvalid
    }
}

/// 纯核（无 VM）文本键臂：先 "-0" 特例，再 round-trip 判定；命中数字臂且
/// 界内整数时携带索引。
pub(crate) fn ta_index_gate_from_text(text: &str, length: usize) -> TaIndexGate {
    let t = text
        .trim_start_matches(crate::number::is_js_ws)
        .trim_end_matches(crate::number::is_js_ws);
    if t == "-0" {
        return TaIndexGate::NumericInvalid;
    }
    let n = ta_to_number_text(t);
    // round-trip 对原文比较（含空白）：`" 1"`/`"1 "` 不成立归 Ordinary。
    if text != oxide_runtime_api::js_number_to_string(n) {
        return TaIndexGate::Ordinary;
    }
    if n.is_nan() || n.is_infinite() || n.fract() != 0.0 || n < 0.0 || n >= length as f64 {
        return TaIndexGate::NumericInvalid;
    }
    TaIndexGate::NumericValid(n as u32)
}

/// 纯文本的整串 ToNumber（供门 round-trip）：空串 +0；`±Infinity` 前缀取
/// 专名；十进制前缀须消费整串否则 NaN（hex/八/二进制与下划线文本全 NaN）。
fn ta_to_number_text(t: &str) -> f64 {
    if t.is_empty() {
        return 0.0;
    }
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if rest.starts_with("Infinity") {
        return if neg { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    match crate::number::parse_decimal_prefix(rest) {
        Some((value, consumed)) if consumed == rest.len() => {
            if neg {
                -value
            } else {
                value
            }
        }
        _ => f64::NAN,
    }
}

/// 写 TypedArray 指定整数索引的元素（供 VM 普通属性 set 的 typed 分支使用）。
/// 先按元素类型转换（副作用/抛错先于界判触发），越界（含 detach）静默忽略
/// （不创建属性、不报错）；内部状态非法返回 Err(String)（防御背板）。
pub fn typed_array_element_set<H: VmHost>(
    vm: &mut H, obj: &JsObject, index: u32, value: JsValue,
) -> Result<(), String> {
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = get_typed_array_data(vm, this_val).map_err(|e| format!("{e}"))?;
    write_typed_array_element(vm, view, index, value)
}

/// 数值键纯强转入口：按元素类型 ToBigInt/ToNumber（副作用/抛错先触发），
/// 不做任何写入。供 [[Set]] 数字无效臂（含 detach：live 长 0 时全数值键落
/// 此臂）消费——强转后结果丢弃。
///
/// # 步骤
/// 1. 取视图（内部状态非法返回防御背板 Err）。
/// 2. 强转：深度 0 抛错已就地展开到外围 catch，pc 守卫即停。
/// 3. 强转结果丢弃。
///
/// # 边界与前提
/// - 调用方须已判定键落门数字无效臂（或 receiver-TA 同臂）；本入口不重判键。
///
/// # 副作用
/// - 执行用户代码（valueOf/toString/toPrimitive），可能抛错；抛错时深度 0
///   已 unwind、深度 >0 以 kind 前缀文本 `Err` 返回。
pub fn typed_array_numeric_key_convert_only<H: VmHost>(
    vm: &mut H, obj: &JsObject, value: JsValue,
) -> Result<(), String> {
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = get_typed_array_data(vm, this_val).map_err(|e| format!("{e}"))?;
    let pc_before = vm.pc();
    let converted = ta_element_value(vm, view.kind, value);
    // pc 守卫：深度 0 强转抛错已展开，直接返回由 dispatch 执行 catch。
    if vm.pc() != pc_before {
        return Ok(());
    }
    match converted {
        Ok(_) => Ok(()),
        // 深度 >0：kind 前缀文本由原生调用边界恢复为异常对象。
        Err(err) => vm.raise_captured(err),
    }
}

/// TypedArray receiver 上的 [[Set]] 写路由：门按 receiver 自身取。界内规范
/// 键强转后写 receiver 元素（可抛、kind 保真，越界静默）；数字无效键强转后
/// 丢弃；非数字键落 receiver 的普通属性路径。
///
/// # 边界与前提
/// - 调用方须已确认 receiver 是 TypedArray 且 receiver ≠ 写入基对象。
///
/// # 副作用
/// - 可能写 receiver buffer / 创建 receiver 属性；强转副作用与自写臂同
///   （深度 0 抛错已 unwind 到外围 catch，深度 >0 以 `Err` 文本传播）。
pub fn typed_array_receiver_set<H: VmHost>(
    vm: &mut H, receiver: &mut JsObject, key_si: u32, value: JsValue,
) -> Result<(), String> {
    match ta_index_gate(vm, receiver, key_si) {
        TaIndexGate::NumericValid(index) => typed_array_element_set(vm, receiver, index, value),
        TaIndexGate::NumericInvalid => typed_array_numeric_key_convert_only(vm, receiver, value),
        TaIndexGate::Ordinary => {
            vm.set_or_create_prop_value(receiver, key_si, value);
            Ok(())
        }
    }
}

/// 定义 TypedArray 整数索引元素（defineProperty 语义）。
///
/// # 步骤
/// 1. 取视图（内部状态非法返回防御背板 Err）。
/// 2. 界判：越界（含 detach：live 长 0）拒绝定义。
/// 3. 清 uncaught 槽后强转写入；强转失败把原值转存 define 专用槽。
///
/// # 边界与前提
/// - 索引越界：拒绝定义（Err），与整数索引 exotic 对象的 [[DefineOwnProperty]] 一致
///
/// # 副作用
/// - 把值 ToNumber 后写入底层 buffer
/// - 强转期用户回调抛错时原值入 define 专用槽，Object/Reflect 入口据此
///   原值重抛
// ponytail: 不校验 writable 描述符——调用方把"省略 writable"折叠为 false，
// 无法与显式 false 区分；TA 元素天然可写，直接写入。
pub fn typed_array_element_define<H: VmHost>(
    vm: &mut H, obj: &JsObject, index: u32, value: JsValue,
) -> Result<(), String> {
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = get_typed_array_data(vm, this_val).map_err(|e| format!("{e}"))?;
    if index as usize >= ta_live_length(view) {
        return Err("cannot define property: TypedArray index out of range".to_string());
    }
    // 强转前清 uncaught 槽（length 路径同纪律）：失败后槽内值必为本次强转
    // 产生的原值。
    vm.clear_uncaught_value();
    let r = write_typed_array_element(vm, view, index, value);
    if r.is_err() {
        vm.move_uncaught_to_pending_length();
    }
    r
}

fn write_typed_array_element<H: VmHost>(
    vm: &mut H, view: TypedArrayData, index: u32, value: JsValue,
) -> Result<(), String> {
    // 先按元素类型转换（valueOf 副作用先于越界判定触发），越界再静默忽略。
    let pc_before = vm.pc();
    let converted = ta_element_value(vm, view.kind, value);
    // pc 守卫：深度 0 强转抛错已就地展开到外围 catch，转换结果为残值——
    // 继续即假值写（undefined 强转 NaN）或二次抛错，直接返回由 dispatch
    // 执行 catch。
    if vm.pc() != pc_before {
        return Ok(());
    }
    let elem = match converted {
        Ok(v) => v,
        // 转换失败恢复为可捕获的 JS 异常：深度 0 原值入通道就地展开到外围
        // catch（kind 保真），深度 >0 以 kind 前缀文本由原生调用边界恢复。
        Err(err) => {
            // 原值放回 uncaught 槽：define 通道专用槽转存依赖槽内原值
            // （set 通道经文本消费，槽不改变其可观测行为）。
            vm.restore_uncaught_value(Some(err));
            return vm.raise_captured(err);
        }
    };
    // live 边界：越界（含 detach）静默不写（规范 IntegerIndexedElementSet 的
    // IsValidIndexedAccess 臂），转换副作用已先发生。
    if index as usize >= ta_live_length(view) {
        return Ok(());
    }
    let buffer_ptr = view.buffer.as_js_object_ptr();
    if buffer_ptr.is_null() {
        return Err("TypedArray buffer internal state invalid".to_string());
    }
    // SAFETY: buffer 对象与视图同生命周期，此处只写载荷字节切片。
    let Some(payload_ptr) = buffer_payload_ptr(unsafe { &*buffer_ptr }) else {
        // 防御背板：构造期 buffer 必为 ArrayBuffer/SharedArrayBuffer 之一。
        return Err("TypedArray buffer internal state invalid".to_string());
    };
    let Some(buffer) = unsafe { &mut *payload_ptr }.data.as_deref_mut() else {
        // 载荷缺失即 detach（live 界判后不可达，防御背板）：转换副作用已先
        // 发生，写按 [[Set]] 静默 no-op。
        return Ok(());
    };
    write_element(vm, view.kind, buffer, absolute_byte_offset(view, index as usize), elem);
    Ok(())
}

/// 读 TypedArray 元素并转为对应 JS 值：BigInt 类型读为 BigInt 值（i64/u64 位模式
/// 原样搬运，无精度损失），数值类型按位模式读为 Number。
pub(crate) fn read_element<H: VmHost>(vm: &mut H, kind: TypedArrayKind, bytes: &[u8], offset: usize) -> JsValue {
    // 底层缓冲可能已被 resize 收缩：元素字节区越界时按 undefined 读
    // （live 视图越界语义），不切片 panic。
    if offset + kind.bytes_per_element() > bytes.len() {
        return JsValue::undefined();
    }
    // 安全读取：先校验 slice 长度匹配目标类型字节数，避免 try_into  panic。
    macro_rules! read_bytes {
        ($slice:expr, $ty:ty) => {{
            let s = $slice;
            let expected = std::mem::size_of::<$ty>();
            if s.len() == expected {
                unsafe { std::ptr::read(s.as_ptr() as *const $ty) }
            } else {
                return JsValue::undefined();
            }
        }};
    }
    match kind {
        TypedArrayKind::Int8 => JsValue::int(bytes[offset] as i8 as i32),
        TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => JsValue::int(bytes[offset] as i32),
        TypedArrayKind::Int16 => JsValue::int(read_bytes!(&bytes[offset..offset + 2], i16) as i32),
        TypedArrayKind::Uint16 => JsValue::int(read_bytes!(&bytes[offset..offset + 2], u16) as i32),
        TypedArrayKind::Int32 => JsValue::int(read_bytes!(&bytes[offset..offset + 4], i32)),
        TypedArrayKind::Uint32 => JsValue::float(read_bytes!(&bytes[offset..offset + 4], u32) as f64),
        TypedArrayKind::Float32 => JsValue::float(read_bytes!(&bytes[offset..offset + 4], f32) as f64),
        TypedArrayKind::Float64 => JsValue::float(read_bytes!(&bytes[offset..offset + 8], f64)),
        TypedArrayKind::BigInt64 => {
            let n = read_bytes!(&bytes[offset..offset + 8], i64);
            vm.new_bigint(BigInt::from(n))
        }
        TypedArrayKind::BigUint64 => {
            let n = read_bytes!(&bytes[offset..offset + 8], u64);
            vm.new_bigint(BigInt::from(n))
        }
    }
}

/// 元素写入统一转换入口：BigInt 类型走 ToBigInt，数值类型走 ToNumber。
///
/// 数值类型显式拒绝 BigInt 值（规范 ToNumber(BigInt) 抛 TypeError），避免
/// 经 f64 近似的静默精度丢失。
pub(crate) fn ta_element_value<H: VmHost>(
    vm: &mut H, kind: TypedArrayKind, value: JsValue,
) -> Result<JsValue, JsValue> {
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

/// 把异常 JsValue 格式化为 `Kind: message` 文本（属性写路径的 String 错误
/// 契约用；原生调用边界据此经 `create_from_text` 恢复 kind）。
pub fn element_error_text<H: VmHost>(vm: &mut H, err: JsValue) -> String {
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
pub(crate) fn write_element<H: VmHost>(
    vm: &mut H, kind: TypedArrayKind, bytes: &mut [u8], offset: usize, value: JsValue,
) {
    // 底层缓冲可能已被 resize 收缩：元素字节区越界时静默不写
    // （live 视图越界语义），不切片 panic。
    if offset + kind.bytes_per_element() > bytes.len() {
        return;
    }
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
            // 模式重解释为 i64（二进制补码）。非 BigInt 值按 ToBigInt 语义落 0
            // （越界读哨值回写路径，防对非 BigInt 取位模式）。
            static ZERO_BI: std::sync::LazyLock<BigInt> = std::sync::LazyLock::new(|| BigInt::from(0_i64));
            let v: &BigInt = if value.is_bigint() { vm.bigint_value(value) } else { &ZERO_BI };
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
        let payload_ptr = buffer_payload(vm, view.buffer)?;
        // SAFETY: payload_ptr 经 buffer_payload 校验为合法缓冲区载荷（AB/SAB 双认）。
        let Some(buffer) = unsafe { &*payload_ptr }.data.as_deref() else {
            // 源 buffer detach：按规范的源缓冲校验步抛 TypeError；各入口的
            // 校验/活长界判先行，此臂为防御背板。
            return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
        };
        // 源视图按 live 长度收集：buffer 收缩越界后视同空源（与构造器
        // TypedArrayCreate 的数组元素收集口径一致）。
        return Ok((0..ta_live_length(view))
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

    // 字符串源（装箱串）的 length 与索引未物化，直接按 UTF-16 单元取值：
    // 与 %TypedArray%.from 的 array-like 臂同读法，避免落通用 length 读空。
    if let Some(units) = crate::array::string_arraylike_units(vm, value) {
        let mut values = Vec::with_capacity(units.len());
        for &unit in &units {
            values.push(crate::array::unit_string_value(vm, unit));
        }
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
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    // 调用形态取构造入口标记（NEW/SUPER native 臂与 construct_with 三入口调用前
    // 置位、普通调用入口调用前清零），不推断 new.target 寄存器：native 调用与
    // 调用方共享寄存器文件，类构造器帧内 new.target 槽残留类构造器对象，按
    // 寄存器推断会把成员式普通调用误判为构造（双物化 receiver、旧数据盒泄漏）。
    // 构造形态下视图数据物化到 receiver（其原型 = new.target.prototype，派生类
    // super() 与 species 构造由此拿到子类实例）；普通形态按规范 TypedArrayCreate
    // 忽略 this、自建新对象。
    let in_construct = vm.constructing_native() && this_val.is_object();
    let first = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::int(0) };
    let bpe = kind.bytes_per_element();

    let (buffer, byte_offset, length, auto_length) = if first.is_object() {
        let first_ptr = first.as_js_object_ptr();
        let first_obj = unsafe { &*first_ptr };
        // ArrayBuffer/SharedArrayBuffer 双认：长度/偏移/长度校验与定长臂同构；
        // SAB 无 detach 产出路径，载荷恒在（防御背板保留）。
        if first_obj.is_array_buffer_obj() || first_obj.is_shared_array_buffer_obj() {
            let Some(payload_ptr) = buffer_payload_ptr(first_obj) else {
                return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
            };
            // SAFETY: payload_ptr 经 buffer_payload_ptr 校验为合法缓冲区载荷。
            let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
                // 首参 buffer 载荷缺失（AB 已 detach；SAB 为防御背板）：按规范的
                // ArrayBuffer 校验步抛 TypeError（构造器入口的 detach 守卫即本臂）。
                return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
            };
            let buffer_len = data.len();
            let byte_offset = if args.len() > 2 {
                native_try!(to_index(vm, vm.reg(args[2]), "TypedArray byteOffset out of bounds"))
            } else {
                0
            };
            if byte_offset > buffer_len || byte_offset % bpe != 0 {
                return NativeResult::Err(range_error(vm, "TypedArray byteOffset out of bounds"));
            }
            let remaining = buffer_len - byte_offset;
            // 规范以 "length is not undefined" 分支：显式 undefined 与省略同义，
            // 取整缓冲剩余长度（live 全长），不落入 ToIndex(undefined)=0。
            // auto 口径：省略/显式 undefined → 长度随 buffer 伸缩；显式给定
            // 定死。
            let auto_length = args.len() <= 3 || vm.reg(args[3]).is_undefined();
            let length = if auto_length {
                remaining / bpe
            } else {
                native_try!(to_index(vm, vm.reg(args[3]), "TypedArray length out of bounds"))
            };
            let Some(byte_length) = length.checked_mul(bpe) else {
                return NativeResult::Err(range_error(vm, "TypedArray length out of bounds"));
            };
            if byte_length > remaining {
                return NativeResult::Err(range_error(vm, "TypedArray length out of bounds"));
            }
            (first, byte_offset, length, auto_length)
        } else {
            let values = native_try!(collect_array_like(vm, first, true));
            let byte_len = values.len().saturating_mul(bpe);
            let buffer =
                JsValue::from_js_object(new_array_buffer(vm, vec![0; byte_len], 0, default_array_buffer_proto(vm)));
            let payload_ptr = native_try!(buffer_payload(vm, buffer));
            // SAFETY: payload_ptr 经 buffer_payload 校验为合法缓冲区载荷（AB/SAB 双认）。
            let Some(buffer_ref) = unsafe { &mut *payload_ptr }.data.as_deref_mut() else {
                // 新建 buffer 载荷恒存活（无人可 detach），此臂为防御背板。
                return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
            };
            for (idx, value) in values.into_iter().enumerate() {
                let elem = native_try!(ta_element_value(vm, kind, value));
                write_element(vm, kind, buffer_ref, idx * bpe, elem);
            }
            (buffer, 0, byte_len / bpe, false)
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
        let buffer =
            JsValue::from_js_object(new_array_buffer(vm, vec![0; byte_len], 0, default_array_buffer_proto(vm)));
        (buffer, 0, len, false)
    };

    if in_construct {
        // SAFETY: receiver 是构造帧分配的 this（本 session 存活），此处只写
        // type_tag 与 native_fn 两个槽位，与调用方无别名。
        let obj = unsafe { &mut *this_val.as_js_object_ptr() };
        materialize_typed_array(obj, kind, buffer, byte_offset, length, auto_length);
        NativeResult::Ok(this_val)
    } else {
        NativeResult::Ok(JsValue::from_js_object(create_typed_array(
            vm,
            kind,
            buffer,
            byte_offset,
            length,
            auto_length,
        )))
    }
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
    native_try!(ta_validate(vm, view, false));
    // 负索引折算与越界判定走规范 TypedArrayLength 口径（auto 随 buffer、定长
    // 静态）。
    let len = ta_spec_length(view);
    let idx: isize = if args.len() > 1 {
        let n = native_try!(ta_to_number(vm, vm.reg(args[1])));
        // ToIntegerOrInfinity：NaN 归 0；±Infinity 不截断，负无穷必然越界、
        // 正无穷归 len（同样越界），避免饱和成 isize::MIN 后加长度下溢。
        if n.is_nan() {
            0
        } else if n.is_infinite() {
            if n > 0.0 {
                len as isize
            } else {
                -1
            }
        } else {
            let k = n.trunc() as isize;
            if k < 0 {
                len as isize + k
            } else {
                k
            }
        }
    } else {
        0
    };
    if idx < 0 || idx as usize >= len {
        return NativeResult::Ok(JsValue::undefined());
    }
    // 定长视图越界（含转换窗口内收缩）：元素窗口起点已到/过 buffer 尾抛
    // TypeError；窗口起点仍在 buffer 内（含 detach）读回 undefined（语料库
    // 双验收口径）。auto 视图经 live 门控自然越界读 undefined。
    if !view.auto_length && ta_is_oob(view) {
        if let Some(buffer_len) = ta_buffer_byte_length(view) {
            if view.byte_offset + (idx as usize) * view.kind.bytes_per_element() >= buffer_len {
                return NativeResult::Err(type_error(vm, "TypedArray is detached or out of bounds"));
            }
        }
        return NativeResult::Ok(JsValue::undefined());
    }
    NativeResult::Ok(native_try!(ta_read(vm, view, idx as usize)))
}

/// `TypedArray.prototype.fill(value, start, end)`：用给定值填充区间，返回 this。
pub fn typed_array_fill<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    // 入口校验先于任何参数转换（detach 时参数副作用不可观测）；见证长度
    // 取入口口径（value 转换可伸缩 buffer，只影响二次校验后的 live 重读）。
    native_try!(ta_validate(vm, view, true));
    let len = ta_spec_length(view);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    // 值只转换一次（valueOf 副作用一次），转换结果写入每个目标元素。
    let elem = native_try!(ta_element_value(vm, view.kind, value));
    let start_raw = if args.len() > 2 {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    let end_raw = if args.len() > 3 && !vm.reg(args[3]).is_undefined() {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[3])))
    } else {
        f64::INFINITY
    };
    // 转换窗口内的 detach 由二次校验捕获；收缩只收 end（仅填仍适用的前缀）。
    native_try!(ta_validate(vm, view, true));
    let start = clamp_index_to_len(start_raw, len);
    let end = clamp_index_to_len(end_raw, len).min(ta_spec_length(view));
    let payload_ptr = native_try!(buffer_payload(vm, view.buffer));
    // SAFETY: payload_ptr 经 buffer_payload 校验为合法缓冲区载荷（AB/SAB 双认）。
    let Some(buffer) = unsafe { &mut *payload_ptr }.data.as_deref_mut() else {
        // 转换窗口内 detach：填充写退化为静默 no-op（循环内无重入窗，单次
        // 判即封窗），按 [[Set]] 语义正常返回 this。
        return NativeResult::Ok(this_val);
    };
    for idx in start..end.max(start) {
        write_element(vm, view.kind, buffer, absolute_byte_offset(view, idx), elem);
    }
    NativeResult::Ok(this_val)
}

/// TypedArraySpeciesCreate(O, argumentList)：读 O 的 constructor / @@species
/// 决定构造目标，按 TypedArrayCreate 语义校验构造结果。
///
/// # 步骤
/// 1. 完整 Get `O.constructor`（原型链 / 访问器 / 异常传播）；undefined 回退
///    接收者类型的内建构造器，其余非对象（null/基元）抛 TypeError。
/// 2. 读 C 的 `@@species`（null 归一为 undefined）；undefined 回退内建构造器，
///    非构造器抛 TypeError。
/// 3. Construct(S, argumentList)：native 值传递 / 字节码压构造帧（派生类
///    super() 与 new.target 传播），构造器抛出值原样上抛。
/// 4. 校验结果确为 TypedArray 对象；`expect_len` 为 Some 时长度不足抛
///    TypeError（argumentList 为单 Number 的 ValidateTypedArray 形态）。
///
/// # 边界与前提
/// - 默认臂（内建构造器）结果必为接收者类型的实例；species 臂交付构造器
///   返回对象原值，不做身份改写。
/// - 用户 getter / 构造器执行窗口内 O 由寄存器根保活，Rust 局部值拷贝
///   跨窗口有效（该窗口内无 GC 安全点）。
///
/// # 参数
/// - `writable`：~write~ 访问模式（map/filter/slice 取 true，subarray
///   等纯视图方法取 false）；true 时结果过 immutable 写守卫（交付源
///   buffer 子视图豁免）。
fn typed_array_species_create<H: VmHost>(
    vm: &mut H, o_val: JsValue, kind: TypedArrayKind, args: Vec<JsValue>, expect_len: Option<usize>, writable: bool,
) -> Result<JsValue, JsValue> {
    let o_ptr = o_val.as_js_object_ptr();
    let o_obj = unsafe { &*o_ptr };
    let ctor_key = vm.kernel_core().perm_interner().intern("constructor").0;
    let c = match vm.ordinary_get(o_obj, ctor_key, o_val) {
        Ok(v) => v,
        Err(msg) => return Err(crate::iterator::engine_error(vm, &msg)),
    };
    let target = if c.is_undefined() {
        typed_array_ctor_value(vm, kind)
    } else if !c.is_object() {
        return Err(type_error(vm, "Species constructor not a constructor"));
    } else {
        let species_key = make_well_known_symbol_key(WELL_KNOWN_SYMBOL_SPECIES);
        let s = match vm.ordinary_get(unsafe { &*c.as_js_object_ptr() }, species_key, c) {
            Ok(v) => v,
            Err(msg) => return Err(crate::iterator::engine_error(vm, &msg)),
        };
        let s = if s.is_null() { JsValue::undefined() } else { s };
        if s.is_undefined() {
            typed_array_ctor_value(vm, kind)
        } else if !crate::array::is_constructor_value(s) {
            return Err(type_error(vm, "Species constructor not a constructor"));
        } else {
            s
        }
    };
    let result = vm.construct_ctor(target, &args)?;
    if !result.is_object() || result.as_js_object_ptr().is_null() {
        return Err(type_error(vm, "Species constructor did not return a TypedArray"));
    }
    let view = get_typed_array_data(vm, result)?;
    // 规范 TypedArrayCreate：构造结果过 ValidateTypedArray（构造器窗口内
    // detach/越界结果抛 TypeError，与 count 无关）。写守卫（~write~ 访问
    // 模式的 immutable 检查）仅在结果交付的不是源 buffer 时生效：构造器
    // 返回源 buffer 子视图正常交付（语料库双验收）。
    let write_guard = writable
        && match get_typed_array_data(vm, o_val) {
            Ok(src) => src.buffer != view.buffer,
            Err(_) => true,
        };
    ta_validate(vm, view, write_guard)?;
    // 长度不足校验取 live 口径（auto 视图的静态长度在 buffer 再伸缩后
    // 失真）；语料库的 immutable-destination 族红即由本条触发（结果长度
    // 小于请求数）。
    if let Some(expect) = expect_len {
        if ta_live_length(view) < expect {
            return Err(type_error(vm, "Species constructor returned a TypedArray with insufficient length"));
        }
    }
    Ok(result)
}

/// `TypedArray.prototype.slice(start, end)`：复制区间元素生成新同类型 TypedArray。
///
/// # 步骤
/// 1. start/end 转换（副作用可 detach）后入口校验，见证长度夹取得 count，
///    经 TypedArraySpeciesCreate(O, « count ») 建目标（零初始化）。
/// 2. 构造窗口后再校验一次：species 构造器可收缩/分离源 buffer。定长视图
///    被裁到越界抛 TypeError；auto 视图按新 live 长度收口。
/// 3. 逐元素读→写交错拷贝：源索引越界（构造期收缩）时跳过写入，目标保持
///    零初始化；别名级联须看到前一轮写入。
pub fn typed_array_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    // 入口校验先于 start/end 转换（detach 时转换副作用不可观测），
    // 见证长度取入口口径。
    native_try!(ta_validate(vm, view, false));
    let len = ta_spec_length(view);
    let start_raw = if args.len() > 1 {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[1])))
    } else {
        0.0
    };
    let end_raw = if args.len() > 2 && !vm.reg(args[2]).is_undefined() {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[2])))
    } else {
        f64::INFINITY
    };
    let start = clamp_index_to_len(start_raw, len);
    let end = clamp_index_to_len(end_raw, len);
    let count = end.max(start).saturating_sub(start);
    let a = native_try!(typed_array_species_create(
        vm,
        this_val,
        view.kind,
        vec![JsValue::int(count as i32)],
        Some(count),
        true,
    ));
    // 逐元素读→写交错：别名级联（目标与源共享 buffer）时本轮读看到前一轮
    // 写入；源索引已越界（构造器收缩源 buffer）时跳过写入，目标保持
    // 零初始化。count 为 0 时整段跳过（含构造器 detach 窗口，规范
    // "If count > 0" 臂）。
    if count > 0 {
        // species 构造窗口后再校验源视图（构造器可 resize/detach 源 buffer）。
        native_try!(ta_validate(vm, view, false));
        for k in 0..count {
            if start + k < ta_live_length(view) {
                let elem = native_try!(ta_read(vm, view, start + k));
                native_try!(set_typed_array_element(vm, a, k, elem));
            }
        }
    }
    NativeResult::Ok(a)
}

/// `TypedArray.prototype.subarray(start, end)`：共享底层 buffer 创建区间子视图。
///
/// # 步骤
/// 1. 归一 start/end 得 count 与字节偏移。
/// 2. TypedArraySpeciesCreate(O, « buffer, byteOffset, count »)：多参
///    argumentList 不查长度；默认臂经内建构造器同三参形态产出共享视图。
pub fn typed_array_subarray<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    // 见证长度先于 start/end 转换：调用时越界/detach 不抛（源长度记 0），
    // 转换窗口内的 detach 只影响 species 构造，不改入口口径。
    let len = ta_live_length(view);
    let start_raw = if args.len() > 1 {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[1])))
    } else {
        0.0
    };
    let end_undefined = args.len() <= 2 || vm.reg(args[2]).is_undefined();
    let end_raw = if end_undefined {
        f64::INFINITY
    } else {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[2])))
    };
    let start = clamp_index_to_len(start_raw, len);
    let end = clamp_index_to_len(end_raw, len);
    let count = end.max(start).saturating_sub(start);
    let byte_offset = view.byte_offset + start * view.kind.bytes_per_element();
    // 2 参构造（省略 count，子视图随 buffer 伸缩）仅当 end 未给、源视图为
    // auto 且 buffer 可缩放；定长视图恒三参定死区间（窗口超 buffer 由构造器
    // 抛 RangeError）。
    let resizable = match buffer_payload(vm, view.buffer) {
        Ok(p) => unsafe { &*p }.max_byte_length != 0,
        Err(_) => false,
    };
    let ctor_args = if end_undefined && view.auto_length && resizable {
        vec![view.buffer, JsValue::int(byte_offset as i32)]
    } else {
        vec![view.buffer, JsValue::int(byte_offset as i32), JsValue::int(count as i32)]
    };
    let result = native_try!(typed_array_species_create(vm, this_val, view.kind, ctor_args, None, false));
    NativeResult::Ok(result)
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
    // 入口校验（含 immutable 写守卫）先于任何参数副作用；offset 按
    // ToIntegerOrInfinity 取整（负值 / +∞ 抛 RangeError），转换窗口内的
    // detach 由二次校验捕获。
    native_try!(ta_validate(vm, view, true));
    let offset_raw = if args.len() > 2 {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    if offset_raw < 0.0 || offset_raw.is_infinite() {
        return NativeResult::Err(range_error(vm, "TypedArray.set offset out of bounds"));
    }
    let offset = offset_raw as usize;
    native_try!(ta_validate(vm, view, true));
    let len = ta_spec_length(view);
    // 源长度在二次校验后读：TA 源按 live 口径，array-like 源读 `length`
    // 属性（ToObject 装箱基元源）。
    let source_obj =
        native_try!(oxide_runtime_api::to_object(source, vm).map_err(|e| crate::iterator::engine_error(vm, &e)));
    let (source_len, source_kind) = match get_typed_array_data(vm, source_obj) {
        Ok(sview) => {
            native_try!(ta_validate(vm, sview, false));
            // 同 buffer 重叠：规范先克隆源字节区，按 live 口径快照源元素，
            // 循环中读快照（重叠写不污染未读源位）。
            if sview.buffer == view.buffer {
                let snap: Vec<JsValue> = native_try!((0..ta_live_length(sview))
                    .map(|i| ta_read(vm, sview, i))
                    .collect::<Result<_, _>>());
                (snap.len(), SetSourceKind::TypedArraySnapshot(snap))
            } else {
                (ta_live_length(sview), SetSourceKind::TypedArray(sview))
            }
        }
        Err(_) => (native_try!(set_array_like_length(vm, source_obj)), SetSourceKind::ArrayLike(source_obj)),
    };
    if (source_len as u64) + (offset as u64) > len as u64 {
        return NativeResult::Err(range_error(vm, "TypedArray.set offset out of bounds"));
    }
    let payload_ptr = native_try!(buffer_payload(vm, view.buffer));
    // SAFETY: payload_ptr 经 buffer_payload 校验为合法缓冲区载荷（AB/SAB 双认）。
    let mut buffer = unsafe { &mut *payload_ptr }.data.as_deref_mut();
    // 逐元素惰性读源→转换→写：循环中的 detach/收缩使后续写入静默失效
    // （目标越界不写，与规范 SetValueInBuffer 边界无操作同语义）；载荷缺失
    // 等同 detach，全循环退化为源读副作用与转换、零写入。
    for k in 0..source_len {
        let value = match &source_kind {
            SetSourceKind::TypedArray(sview) => native_try!(ta_read(vm, *sview, k)),
            SetSourceKind::TypedArraySnapshot(snap) => snap[k],
            SetSourceKind::ArrayLike(src) => native_try!(set_source_get(vm, *src, k)),
        };
        let elem = native_try!(ta_element_value(vm, view.kind, value));
        if offset + k < ta_live_length(view) {
            if let Some(buffer) = buffer.as_deref_mut() {
                write_element(vm, view.kind, buffer, absolute_byte_offset(view, offset + k), elem);
            }
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `set` 的源形态：TA 源直读元素区（同 buffer 先快照）；array-like 源
/// 按索引键逐取。
enum SetSourceKind {
    TypedArray(TypedArrayData),
    TypedArraySnapshot(Vec<JsValue>),
    ArrayLike(JsValue),
}

/// array-like 源的 `length` 属性读 + ToLength 收敛（NaN/负取 0，上限 2^53-1）。
fn set_array_like_length<H: VmHost>(vm: &mut H, source: JsValue) -> Result<usize, JsValue> {
    let si = vm.kernel_core().perm_interner().intern("length").0;
    let ptr = source.as_js_object_ptr();
    let len_val = unsafe { vm.ordinary_get(&*ptr, si, source) }.map_err(|e| crate::iterator::engine_error(vm, &e))?;
    let n = oxide_runtime_api::to_number_full(len_val, vm).map_err(|e| crate::iterator::engine_error(vm, &e))?;
    if n.is_nan() || n <= 0.0 {
        return Ok(0);
    }
    Ok((n.min(9_007_199_254_740_991.0).trunc() as u64) as usize)
}

/// array-like 源第 k 个元素：Array 走密集槽，字符串源按 UTF-16 单元，
/// 其余按 ToString(k) 键普通 Get。
fn set_source_get<H: VmHost>(vm: &mut H, source: JsValue, k: usize) -> Result<JsValue, JsValue> {
    let ptr = source.as_js_object_ptr();
    if !ptr.is_null() {
        let obj = unsafe { &*ptr };
        if obj.is_array() {
            return Ok(obj.get_prop_at(k));
        }
        if let Some(units) = crate::array::string_arraylike_units(vm, source) {
            return Ok(crate::array::unit_string_value(vm, units[k.min(units.len() - 1)]));
        }
    }
    let key = vm.new_string(&k.to_string());
    let key_si = vm.property_key_si(key);
    if ptr.is_null() {
        return Ok(JsValue::undefined());
    }
    unsafe { vm.ordinary_get(&*ptr, key_si, source) }.map_err(|e| crate::iterator::engine_error(vm, &e))
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

/// `%TypedArray%.prototype.byteOffset` 访问器：返回视图相对 buffer 起始的
/// 字节偏移；越界视图（含 detach）读 0。
pub fn typed_array_byte_offset_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let offset = if ta_is_oob(view) { 0 } else { view.byte_offset };
    NativeResult::Ok(JsValue::int(offset as i32))
}

/// `%TypedArray%.prototype.byteLength` 访问器：返回视图占用的字节数（live
/// 长度 × 元素字节数；越界视图读 0）。
pub fn typed_array_byte_length_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    let byte_length = ta_live_length(view) * view.kind.bytes_per_element();
    NativeResult::Ok(JsValue::int(byte_length as i32))
}

/// `%TypedArray%.prototype.length` 访问器：返回元素个数（live 口径；越界
/// 视图读 0）。
pub fn typed_array_length_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    NativeResult::Ok(JsValue::int(ta_live_length(view) as i32))
}

/// `%TypedArray%.prototype[@@toStringTag]` 访问器：返回具体类型名（如
/// `Int16Array`），供 `Object.prototype.toString` 区分类型。
///
/// # 边界与前提
/// - this 非对象、无 `[[TypedArrayName]]` 内部槽（如原型自身）或内部状态无效时
///   返回 undefined（规范语义，不抛错）。
pub fn typed_array_to_string_tag_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = match get_typed_array_data(vm, this_val) {
        Ok(view) => view,
        Err(_) => return NativeResult::Ok(JsValue::undefined()),
    };
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
    // 构造（new）形态调用：派生类的 super() 与 new.target 传播只在构造帧成立，
    // 普通调用会在派生类上抛 "super() used outside class constructor"；
    // 构造器抛出值经 Err 原样上抛。
    let result = vm.construct_ctor(c, &[JsValue::int(len as i32)])?;
    if !result.is_object() || result.as_js_object_ptr().is_null() {
        return Err(type_error(vm, "TypedArray.of/from constructor did not return a TypedArray"));
    }
    let result_obj = unsafe { &*result.as_js_object_ptr() };
    if !result_obj.is_typed_array_obj() {
        return Err(type_error(vm, "TypedArray.of/from constructor did not return a TypedArray"));
    }
    let view = get_typed_array_data(vm, result)?;
    // 结果长校验取 live 口径（规范 TypedArrayCreateFromConstructor：长度取
    // TypedArrayLength 实时值，构造器窗口内 auto 视图可已收缩）；越界（含
    // detach）恒 0，自然落入长度不足 TypeError。
    if ta_live_length(view) < len {
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
    // 先转换（副作用/抛错先于越界判定），越界（含 detach）静默不写。
    let elem = ta_element_value(vm, view.kind, value)?;
    if index >= ta_live_length(view) {
        return Ok(());
    }
    let payload_ptr = buffer_payload(vm, view.buffer)?;
    // SAFETY: payload_ptr 经 buffer_payload 校验为合法缓冲区载荷（AB/SAB 双认）。
    let Some(buffer) = unsafe { &mut *payload_ptr }.data.as_deref_mut() else {
        // 载荷缺失即 detach（live 界判后不可达，防御背板）：转换副作用已先
        // 发生，写按 [[Set]] 静默 no-op。
        return Ok(());
    };
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
/// 越界（含 detach）一律返回 undefined，BigInt 类型不特殊化（规范
/// Get 的越界语义全类型同口径）。
fn ta_read<H: VmHost>(vm: &mut H, view: TypedArrayData, index: usize) -> Result<JsValue, JsValue> {
    if index >= ta_live_length(view) {
        return Ok(JsValue::undefined());
    }
    let payload_ptr = buffer_payload(vm, view.buffer)?;
    // SAFETY: payload_ptr 经 buffer_payload 校验为合法缓冲区载荷（AB/SAB 双认）。
    let Some(buffer) = unsafe { &*payload_ptr }.data.as_deref() else {
        // 载荷缺失即 detach（live 界判后不可达，防御背板）：按 [[Get]] 读
        // undefined。
        return Ok(JsValue::undefined());
    };
    Ok(read_element(vm, view.kind, buffer, absolute_byte_offset(view, index)))
}

/// 把已转换的值写入 TypedArray 指定索引（视图 data 已取出的形式，供原型方法内部使用）。
fn ta_write<H: VmHost>(vm: &mut H, view: TypedArrayData, index: usize, value: JsValue) -> Result<(), JsValue> {
    // 越界（含循环中的 detach/收缩）静默失效，与规范 SetValueInBuffer
    // 边界无操作同语义。
    if index >= ta_live_length(view) {
        return Ok(());
    }
    let payload_ptr = buffer_payload(vm, view.buffer)?;
    // SAFETY: payload_ptr 经 buffer_payload 校验为合法缓冲区载荷（AB/SAB 双认）。
    let Some(buffer) = unsafe { &mut *payload_ptr }.data.as_deref_mut() else {
        // 载荷缺失即 detach（live 界判后不可达，防御背板）：写静默 no-op。
        return Ok(());
    };
    write_element(vm, view.kind, buffer, absolute_byte_offset(view, index), value);
    Ok(())
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
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // 入口见证：循环长度一次捕获，回调期 buffer 伸缩不改本轮范围。
    let len = ta_spec_length(view);
    for i in 0..len {
        let elem = native_try!(ta_read(vm, view, i));
        native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `%TypedArray%.prototype.map(callback, thisArg)`：对每个元素调用 callback，
/// 结果经 species 构造的目标 TypedArray 按元素类型转换写入。
pub fn typed_array_map<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // 入口见证：目标长度 = 源见证长度，回调期 buffer 伸缩不改本轮范围。
    let len = ta_spec_length(view);
    // 目标对象经 species 构造（长度 = 源长度）；逐元素"回调 → 写入"
    // 交错，目标与源别名时后续回调看到前一轮写入值。
    let a = native_try!(typed_array_species_create(
        vm,
        this_val,
        view.kind,
        vec![JsValue::int(len as i32)],
        Some(len),
        true,
    ));
    for i in 0..len {
        let elem = native_try!(ta_read(vm, view, i));
        let mapped = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        native_try!(set_typed_array_element(vm, a, i, mapped));
    }
    NativeResult::Ok(a)
}

/// `%TypedArray%.prototype.filter(callback, thisArg)`：保留 callback 为真的元素，
/// 组成经 species 构造的目标 TypedArray。
pub fn typed_array_filter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // 先全量回调得通过元素（规范序：count 先于目标构造），再经 species
    // 构造目标（长度 = 通过数）并顺序写入；循环走入口见证长度。
    let len = ta_spec_length(view);
    let mut kept = Vec::new();
    for i in 0..len {
        let elem = native_try!(ta_read(vm, view, i));
        let result = native_try!(invoke_cb(vm, callback, this_arg, &[elem, JsValue::int(i as i32), this_val]));
        if oxide_runtime_api::to_boolean(result) {
            kept.push(elem);
        }
    }
    let a = native_try!(typed_array_species_create(
        vm,
        this_val,
        view.kind,
        vec![JsValue::int(kept.len() as i32)],
        Some(kept.len()),
        true,
    ));
    for (k, elem) in kept.into_iter().enumerate() {
        native_try!(set_typed_array_element(vm, a, k, elem));
    }
    NativeResult::Ok(a)
}

/// `%TypedArray%.prototype.reduce(callback, initialValue)`：从左到右累计归约；
/// 空数组且无初始值抛 TypeError。
pub fn typed_array_reduce<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    native_try!(ta_validate(vm, view, false));
    // 见证长度一次捕获：空检与循环同口径（shrink 到 0 的 auto 视图按空抛错）。
    let len = ta_spec_length(view);
    if len == 0 && args.len() < 3 {
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
    for i in start_idx..len {
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
    native_try!(ta_validate(vm, view, false));
    // 见证长度一次捕获：空检与循环同口径（shrink 到 0 的 auto 视图按空抛错）。
    let len = ta_spec_length(view);
    if len == 0 && args.len() < 3 {
        return NativeResult::Err(type_error(vm, "Reduce of empty array with no initial value"));
    }
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let (mut accumulator, start_idx): (JsValue, i32) = if args.len() > 2 {
        (vm.reg(args[2]), len as i32 - 1)
    } else {
        (native_try!(ta_read(vm, view, len - 1)), len as i32 - 2)
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
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let len = ta_spec_length(view);
    for i in 0..len {
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
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let len = ta_spec_length(view);
    for i in 0..len {
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
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let len = ta_spec_length(view);
    for i in 0..len {
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
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let len = ta_spec_length(view);
    for i in 0..len {
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
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let len = ta_spec_length(view);
    for i in (0..len).rev() {
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
    native_try!(ta_validate(vm, view, false));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "callback is not a function"));
    }
    let callback = native_try!(crate::array::require_callback(vm, vm.reg(args[1])));
    let this_arg = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let len = ta_spec_length(view);
    for i in (0..len).rev() {
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
    native_try!(ta_validate(vm, view, false));
    let len = ta_spec_length(view);
    if len == 0 || args.len() < 2 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let target = vm.reg(args[1]);
    let from_index = if args.len() >= 3 {
        native_try!(normalize_from_index(vm, vm.reg(args[2]), len))
    } else {
        0
    };
    for i in from_index..len {
        let elem = native_try!(ta_read(vm, view, i));
        // 越界（转换窗口内 detach/收缩）视同 HasProperty 为假：跳过，
        // 不参与相等比较（in-bounds 元素恒非 undefined）。
        if elem.is_undefined() {
            continue;
        }
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
    native_try!(ta_validate(vm, view, false));
    let len = ta_spec_length(view);
    if len == 0 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let target = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let from_index: isize = if args.len() >= 3 {
        let v = vm.reg(args[2]);
        let f = match vm.coerce_number_bounded(v) {
            Ok(n) => n,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
        // ToIntegerOrInfinity：NaN 从 0 起找；±Infinity 不截断，正无穷封顶
        // len-1、负无穷直接越界返回 -1，避免饱和成 isize::MIN 后加长度下溢。
        if f.is_nan() {
            0
        } else if f.is_infinite() {
            if f > 0.0 {
                len as isize - 1
            } else {
                -1
            }
        } else {
            let f = f.trunc() as isize;
            if f >= 0 {
                f.min(len as isize - 1)
            } else {
                len as isize + f
            }
        }
    } else {
        len as isize - 1
    };
    if from_index < 0 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    for i in (0..=from_index as usize).rev() {
        let elem = native_try!(ta_read(vm, view, i));
        // 越界（转换窗口内 detach/收缩）视同 HasProperty 为假：跳过，
        // 不参与相等比较（in-bounds 元素恒非 undefined）。
        if elem.is_undefined() {
            continue;
        }
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
    native_try!(ta_validate(vm, view, false));
    let len = ta_spec_length(view);
    if len == 0 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let target = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let from_index = if args.len() >= 3 {
        native_try!(normalize_from_index(vm, vm.reg(args[2]), len))
    } else {
        0
    };
    for i in from_index..len {
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
    // 见证先于分隔符转换：转换可伸缩 buffer，循环长度取入口口径（起始长度），
    // 伸缩后的新元素不进本轮。
    native_try!(ta_validate(vm, view, false));
    let len = ta_spec_length(view);
    // 分隔符缺省或 undefined 时取 ","（规范：undefined 视同未给）；
    // 其余值过完整 ToString（null → "null"）。
    let sep = if args.len() > 1 && !vm.reg(args[1]).is_undefined() {
        native_try!(
            oxide_runtime_api::to_string_full(vm.reg(args[1]), vm).map_err(|e| crate::iterator::engine_error(vm, &e))
        )
    } else {
        ",".to_string()
    };
    let mut parts = Vec::with_capacity(len);
    for i in 0..len {
        let elem = native_try!(ta_read(vm, view, i));
        // 越界/detach 读回 undefined（或 null）时拼空串，与 Array.join 同
        // 口径。
        let part = if elem.is_undefined() || elem.is_null() {
            String::new()
        } else {
            oxide_runtime_api::to_string(elem)
        };
        parts.push(part);
    }
    NativeResult::Ok(vm.new_string(&parts.join(&sep)))
}

/// `%TypedArray%.prototype.values()`：返回迭代元素值的迭代器。
pub fn typed_array_values<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    native_try!(ta_validate(vm, view, false));
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
    let view = native_try!(get_typed_array_data(vm, this_val));
    native_try!(ta_validate(vm, view, false));
    match crate::array::make_array_iterator(vm, this_val, crate::array::ARRAY_ITER_KIND_KEYS) {
        Ok(iter) => NativeResult::Ok(iter),
        Err(err) => NativeResult::Err(err),
    }
}

/// `%TypedArray%.prototype.entries()`：返回迭代 `[index, element]` 对的迭代器。
pub fn typed_array_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    native_try!(ta_validate(vm, view, false));
    match crate::array::make_array_iterator(vm, this_val, crate::array::ARRAY_ITER_KIND_ENTRIES) {
        Ok(iter) => NativeResult::Ok(iter),
        Err(err) => NativeResult::Err(err),
    }
}

/// 默认数值排序比较（规范 CompareTypedArrayElements 无比较器臂）：NaN
/// 视为最大排到末尾；-0 排在 +0 之前；其余按数值升序。
fn default_ta_order(a: f64, b: f64) -> std::cmp::Ordering {
    if a.is_nan() && b.is_nan() {
        return std::cmp::Ordering::Equal;
    }
    if a.is_nan() {
        return std::cmp::Ordering::Greater;
    }
    if b.is_nan() {
        return std::cmp::Ordering::Less;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() && !b.is_sign_negative() {
            std::cmp::Ordering::Less
        } else if b.is_sign_negative() && !a.is_sign_negative() {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        };
    }
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
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
    // 原地写方法：入口校验（detach/越界/immutable 抛 TypeError），比较器
    // 检查先于校验（规范序）。
    native_try!(ta_validate(vm, view, true));
    let len = ta_spec_length(view);
    let mut vals: Vec<JsValue> = Vec::with_capacity(len);
    for i in 0..len {
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
                    // 规范 CompareTypedArrayElements：结果过完整 ToNumber
                    // （对象触发 toPrimitive），NaN 视同 +0；异常上抛。
                    let n = match oxide_runtime_api::to_number_full(r, vm) {
                        Ok(n) => n,
                        Err(e) => {
                            sort_error = Some(crate::iterator::engine_error(vm, &e));
                            return std::cmp::Ordering::Equal;
                        }
                    };
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
    native_try!(ta_validate(vm, view, true));
    let len = ta_spec_length(view);
    let mut i = 0;
    let mut j = len.saturating_sub(1);
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
    // 入口校验先于三索引转换（detach 时转换副作用不可观测），见证长度
    // 取入口口径（转换窗口内的 detach 不影响入口长度）。
    native_try!(ta_validate(vm, view, true));
    let len = ta_spec_length(view);
    let target_raw = if args.len() > 1 {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[1])))
    } else {
        0.0
    };
    let start_raw = if args.len() > 2 {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    let end_raw = if args.len() > 3 && !vm.reg(args[3]).is_undefined() {
        native_try!(ta_to_integer_or_infinity(vm, vm.reg(args[3])))
    } else {
        f64::INFINITY
    };
    let target = clamp_index_to_len(target_raw, len);
    let start = clamp_index_to_len(start_raw, len);
    let end = clamp_index_to_len(end_raw, len);
    let mut count = end.max(start).saturating_sub(start).min(len.saturating_sub(target));
    if count > 0 {
        // 转换窗口内的 detach 由二次校验捕获；收缩后的 live 长度再收
        // 一次 count（仅拷仍适用的最长前缀）。
        native_try!(ta_validate(vm, view, true));
        let len2 = ta_spec_length(view);
        count = count.min(len2.saturating_sub(start)).min(len2.saturating_sub(target));
    }
    // 目标区间前移与源区间重叠时逆序遍历，避免覆盖未读的源元素。
    let (mut from, mut to, direction) = if count > 0 && start < target && target < start + count {
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
    native_try!(ta_validate(vm, view, false));
    let len = ta_spec_length(view);
    if len == 0 {
        return NativeResult::Ok(vm.new_string(""));
    }
    let mut parts = Vec::with_capacity(len);
    for i in 0..len {
        let elem = native_try!(ta_read(vm, view, i));
        let r = native_try!(invoke_element_to_locale_string(vm, elem));
        let s =
            native_try!(oxide_runtime_api::to_string_full(r, vm).map_err(|e| crate::iterator::engine_error(vm, &e)));
        parts.push(s);
    }
    NativeResult::Ok(vm.new_string(&parts.join(",")))
}

/// `%TypedArray%.prototype.toReversed()`：返回元素反转的 TypedArrayCreateSameType
/// 新 TypedArray（同类型、忽略 species；原对象不变）。
pub fn typed_array_to_reversed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    native_try!(ta_validate(vm, view, false));
    let len = ta_spec_length(view);
    let a = JsValue::from_js_object(create_same_type_typed_array(vm, view.kind, len));
    for i in 0..len {
        let elem = native_try!(ta_read(vm, view, len - 1 - i));
        native_try!(set_typed_array_element(vm, a, i, elem));
    }
    NativeResult::Ok(a)
}

/// `%TypedArray%.prototype.toSorted(comparefn)`：返回元素排序后的
/// TypedArrayCreateSameType 新 TypedArray（同类型、忽略 species；原对象不变）。
pub fn typed_array_to_sorted<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    native_try!(ta_validate(vm, view, false));
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
    let len = ta_spec_length(view);
    let mut vals: Vec<JsValue> = Vec::with_capacity(len);
    for i in 0..len {
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
                    // 规范 CompareTypedArrayElements：结果过完整 ToNumber
                    // （对象触发 toPrimitive），NaN 视同 +0；异常上抛。
                    let n = match oxide_runtime_api::to_number_full(r, vm) {
                        Ok(n) => n,
                        Err(e) => {
                            sort_error = Some(crate::iterator::engine_error(vm, &e));
                            return std::cmp::Ordering::Equal;
                        }
                    };
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
    let a = JsValue::from_js_object(create_same_type_typed_array(vm, view.kind, vals.len()));
    for (k, v) in vals.into_iter().enumerate() {
        native_try!(set_typed_array_element(vm, a, k, v));
    }
    NativeResult::Ok(a)
}

/// `%TypedArray%.prototype.with(index, value)`：返回替换指定索引元素后的
/// TypedArrayCreateSameType 新 TypedArray（同类型、忽略 species）。
/// 负索引从尾部折算；value 的类型转换（ToNumber/ToBigInt，副作用对后续源元素读
/// 可见）先于越界 RangeError。
pub fn typed_array_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_typed_array_data(vm, this_val));
    if args.len() < 2 {
        return NativeResult::Err(type_error(vm, "TypedArray.prototype.with requires an index"));
    }
    native_try!(ta_validate(vm, view, false));
    let len = ta_spec_length(view);
    // ToIntegerOrInfinity(index)，负值折算为 len + index。
    let raw = native_try!(ta_to_number(vm, vm.reg(args[1])));
    let relative_index = if raw.is_nan() || raw == 0.0 {
        0.0
    } else if raw.is_infinite() {
        raw
    } else {
        raw.trunc()
    };
    let actual_index = if relative_index >= 0.0 { relative_index } else { len as f64 + relative_index };
    // 规范序：value 先转换（副作用可伸缩 buffer），越界检查（IsValidIntegerIndex
    // 口径）在转换后对当前 live 长度做；结果长度取入口见证。
    let value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let replacement = native_try!(ta_element_value(vm, view.kind, value));
    if actual_index.is_nan() || actual_index < 0.0 || actual_index >= ta_live_length(view) as f64 {
        return NativeResult::Err(range_error(vm, "Invalid typed array index"));
    }
    let index = actual_index as usize;
    let a = JsValue::from_js_object(create_same_type_typed_array(vm, view.kind, len));
    // 目标索引写已转换的 value（副作用已先于源读发生），其余索引顺序拷源元素；
    // 索引超结果长度时写入自然无操作（SetElement 边界语义）。
    for i in 0..len {
        let elem = if i == index { replacement } else { native_try!(ta_read(vm, view, i)) };
        native_try!(set_typed_array_element(vm, a, i, elem));
    }
    NativeResult::Ok(a)
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
            // 字符串源（原始串/装箱串）的 length 与索引未物化，直接按 UTF-16 单元取值。
            if let Some(units) = crate::array::string_arraylike_units(vm, source) {
                let mut values = Vec::with_capacity(units.len());
                for &unit in &units {
                    values.push(crate::array::unit_string_value(vm, unit));
                }
                values
            } else {
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

// ── Uint8Array base64/hex 六方法族 ─────────────────────────────────────────────

/// ValidateUint8Array：接收者须为 kind 恰为 Uint8 的 TypedArray 对象；
/// Uint8Clamped 与其他种类一律抛 TypeError。
fn validate_uint8_array<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<TypedArrayData, JsValue> {
    let view = get_typed_array_data(vm, this_val)?;
    if view.kind != TypedArrayKind::Uint8 {
        return Err(type_error(vm, "Uint8Array method called on incompatible receiver"));
    }
    Ok(view)
}

/// GetOptionsObject：undefined 直通；其余值过 ToObject，失败抛 TypeError。
fn ta_options_object<H: VmHost>(vm: &mut H, value: JsValue) -> Result<Option<JsValue>, JsValue> {
    if value.is_undefined() {
        return Ok(None);
    }
    match oxide_runtime_api::to_object(value, vm) {
        Ok(obj) => Ok(Some(obj)),
        Err(e) => Err(type_error(vm, &e)),
    }
}

/// 读只收字符串原始值的选项字段（alphabet/lastChunkHandling）：undefined 取
/// 默认值；装箱串/抛错 toString 对象等一律抛 TypeError，不做 ToPrimitive。
fn ta_string_option<H: VmHost>(
    vm: &mut H, opts: &JsObject, opts_val: JsValue, key: &str, default: &'static str,
) -> Result<String, JsValue> {
    let key_si = vm.string_key_si(key);
    let v = vm
        .ordinary_get(opts, key_si, opts_val)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    if v.is_undefined() {
        return Ok(default.to_string());
    }
    if !v.is_string() {
        return Err(type_error(vm, &format!("{key} must be a string")));
    }
    Ok(vm.lookup_str(v).unwrap_or_default())
}

/// ToBoolean：仅 null/undefined/false/NaN/±0 为假。
fn ta_to_boolean(v: JsValue) -> bool {
    if v.is_null() || v.is_undefined() {
        return false;
    }
    if v.is_bool() {
        return v.as_bool();
    }
    if v.is_int() {
        return v.as_int() != 0;
    }
    if v.is_double() {
        let d = v.as_double();
        return !d.is_nan() && d != 0.0;
    }
    true
}

/// 取选项中的 alphabet 值并归一到字母表枚举；非 "base64"/"base64url" 抛 TypeError。
fn ta_alphabet_option<H: VmHost>(
    vm: &mut H, opts: &JsObject, opts_val: JsValue,
) -> Result<crate::ta_codec::Base64Alphabet, JsValue> {
    let s = ta_string_option(vm, opts, opts_val, "alphabet", "base64")?;
    match s.as_str() {
        "base64url" => Ok(crate::ta_codec::Base64Alphabet::Url),
        "base64" => Ok(crate::ta_codec::Base64Alphabet::Standard),
        _ => Err(type_error(vm, "alphabet must be 'base64' or 'base64url'")),
    }
}

/// 取选项中的 lastChunkHandling 值并归一到模式枚举；非三取值抛 TypeError。
fn ta_handling_option<H: VmHost>(
    vm: &mut H, opts: &JsObject, opts_val: JsValue,
) -> Result<crate::ta_codec::LastChunkHandling, JsValue> {
    let s = ta_string_option(vm, opts, opts_val, "lastChunkHandling", "loose")?;
    match s.as_str() {
        "strict" => Ok(crate::ta_codec::LastChunkHandling::Strict),
        "stop-before-partial" => Ok(crate::ta_codec::LastChunkHandling::StopBeforePartial),
        "loose" => Ok(crate::ta_codec::LastChunkHandling::Loose),
        _ => Err(type_error(vm, "lastChunkHandling must be 'loose', 'strict', or 'stop-before-partial'")),
    }
}

/// setFromBase64/fromBase64 共用入口：string 类型检查先于选项读取，
/// 选项读完跑 FromBase64 状态机，已解码字节经 `write` 逐块写出；
/// `max_len` 为 None 时静态方法无界。
fn ta_decode_base64<H: VmHost>(
    vm: &mut H, string: JsValue, options: JsValue, max_len: Option<usize>, write: &mut dyn FnMut(usize, u8),
) -> Result<(usize, usize), JsValue> {
    if !string.is_string() {
        return Err(type_error(vm, "string argument required"));
    }
    let opts = ta_options_object(vm, options)?;
    let mut alphabet = crate::ta_codec::Base64Alphabet::Standard;
    let mut handling = crate::ta_codec::LastChunkHandling::Loose;
    if let Some(opts_val) = opts {
        let obj = unsafe { &*opts_val.as_js_object_ptr() };
        // SAFETY: opts_val 来自 ToObject 成功路径，必为活动对象指针。
        alphabet = ta_alphabet_option(vm, obj, opts_val)?;
        handling = ta_handling_option(vm, obj, opts_val)?;
    }
    let units = vm.string_units(string).into_owned();
    match crate::ta_codec::decode_base64(&units, alphabet, handling, max_len, write) {
        Ok(r) => Ok(r),
        Err(_) => Err(crate::error::create_syntax_error(vm, "invalid base64 input")),
    }
}

/// setFrom* 结果对象 `{ read, written }`：普通对象 + 两数据属性。
fn ta_read_written_result<H: VmHost>(vm: &mut H, read: usize, written: usize) -> JsValue {
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let read_si = vm.kernel_core().perm_interner().intern("read").0;
    let written_si = vm.kernel_core().perm_interner().intern("written").0;
    let obj_ref = unsafe { &mut *obj };
    vm.set_or_create_prop_value(obj_ref, read_si, JsValue::int(read as i32));
    vm.set_or_create_prop_value(obj_ref, written_si, JsValue::int(written as i32));
    JsValue::from_js_object(obj)
}

/// `%Uint8Array%.prototype.toBase64(options)`：ValidateUint8Array 先于选项
/// 副作用，按字母表与 omitPadding 编码视图字节。
pub fn uint8array_to_base64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(validate_uint8_array(vm, this_val));
    let opts = native_try!(ta_options_object(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let mut alphabet = crate::ta_codec::Base64Alphabet::Standard;
    let mut omit_padding = false;
    if let Some(opts_val) = opts {
        let obj = unsafe { &*opts_val.as_js_object_ptr() };
        // SAFETY: opts_val 来自 ToObject 成功路径，必为活动对象指针。
        alphabet = native_try!(ta_alphabet_option(vm, obj, opts_val));
        let key_si = vm.string_key_si("omitPadding");
        let v = native_try!(vm
            .ordinary_get(obj, key_si, opts_val)
            .map_err(|e| crate::iterator::engine_error(vm, &e)));
        omit_padding = ta_to_boolean(v);
    }
    let payload_ptr = native_try!(buffer_payload(vm, view.buffer));
    // SAFETY: payload_ptr 来自活动缓冲区对象（AB/SAB 双认），视图范围由构造保证界内。
    let Some(buffer) = unsafe { &*payload_ptr }.data.as_deref() else {
        // buffer detach：按规范的视图校验步抛 TypeError；入口校验先行，
        // 此臂为防御背板。
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    // 视图范围按 live 长度与当前缓冲长度双收口（缓冲可被 resize 收缩、
    // 视图可越界）。
    let start = view.byte_offset.min(buffer.len());
    let end = (view.byte_offset.saturating_add(ta_live_length(view))).min(buffer.len());
    NativeResult::Ok(vm.new_string_owned(crate::ta_codec::encode_base64(&buffer[start..end], alphabet, omit_padding)))
}

/// `%Uint8Array%.prototype.toHex()`：ValidateUint8Array 后编码小写两位 hex。
pub fn uint8array_to_hex<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(validate_uint8_array(vm, this_val));
    let payload_ptr = native_try!(buffer_payload(vm, view.buffer));
    // SAFETY: payload_ptr 来自活动缓冲区对象（AB/SAB 双认），视图范围由构造保证界内。
    let Some(buffer) = unsafe { &*payload_ptr }.data.as_deref() else {
        // buffer detach：按规范的视图校验步抛 TypeError；入口校验先行，
        // 此臂为防御背板。
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    // 视图范围按 live 长度与当前缓冲长度双收口（缓冲可被 resize 收缩、
    // 视图可越界）。
    let start = view.byte_offset.min(buffer.len());
    let end = (view.byte_offset.saturating_add(ta_live_length(view))).min(buffer.len());
    NativeResult::Ok(vm.new_string_owned(crate::ta_codec::encode_hex(&buffer[start..end])))
}

/// `%Uint8Array%.prototype.setFromBase64(string, options)`：解码写入视图，
/// 返回 `{ read, written }`；前块已写入后遇错抛 SyntaxError（前字节保留）。
pub fn uint8array_set_from_base64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(validate_uint8_array(vm, this_val));
    let string = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let options = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let payload_ptr = native_try!(buffer_payload(vm, view.buffer));
    // SAFETY: payload_ptr 来自活动缓冲区对象（AB/SAB 双认），视图范围由构造保证界内。
    let Some(buffer) = unsafe { &mut *payload_ptr }.data.as_deref_mut() else {
        // buffer detach：按规范的视图校验步抛 TypeError；入口校验先行，
        // 此臂为防御背板。
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let start = view.byte_offset;
    let (read, written) =
        native_try!(ta_decode_base64(vm, string, options, Some(ta_live_length(view)), &mut |idx, b| {
            // 缓冲可被 resize 收缩：目标字节越界时静默不写（live 视图越界语义）。
            if start + idx < buffer.len() {
                buffer[start + idx] = b;
            }
        }));
    NativeResult::Ok(ta_read_written_result(vm, read, written))
}

/// `%Uint8Array%.prototype.setFromHex(string)`：无选项；奇数长度最前判定
/// （零写入）抛 SyntaxError；坏字符保留前字节写入后抛。
pub fn uint8array_set_from_hex<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(validate_uint8_array(vm, this_val));
    let string = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !string.is_string() {
        return NativeResult::Err(type_error(vm, "string argument required"));
    }
    let payload_ptr = native_try!(buffer_payload(vm, view.buffer));
    // SAFETY: payload_ptr 来自活动缓冲区对象（AB/SAB 双认），视图范围由构造保证界内。
    let Some(buffer) = unsafe { &mut *payload_ptr }.data.as_deref_mut() else {
        // buffer detach：按规范的视图校验步抛 TypeError；入口校验先行，
        // 此臂为防御背板。
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let start = view.byte_offset;
    let units = vm.string_units(string).into_owned();
    let (read, written) = match crate::ta_codec::decode_hex(&units, Some(ta_live_length(view)), &mut |idx, b| {
        // 缓冲可被 resize 收缩：目标字节越界时静默不写（live 视图越界语义）。
        if start + idx < buffer.len() {
            buffer[start + idx] = b;
        }
    }) {
        Ok(r) => r,
        Err(_) => return NativeResult::Err(crate::error::create_syntax_error(vm, "invalid hex input")),
    };
    NativeResult::Ok(ta_read_written_result(vm, read, written))
}

/// `Uint8Array.fromBase64(string, options)`：不读 this，结果 proto 恒为
/// %Uint8Array%.prototype（不经 species/构造器）。
pub fn uint8array_from_base64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let string = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let options = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let mut bytes = Vec::new();
    native_try!(ta_decode_base64(vm, string, options, None, &mut |_, b| bytes.push(b)));
    let len = bytes.len();
    let buffer = JsValue::from_js_object(new_array_buffer(vm, bytes, 0, default_array_buffer_proto(vm)));
    NativeResult::Ok(JsValue::from_js_object(create_typed_array(
        vm,
        TypedArrayKind::Uint8,
        buffer,
        0,
        len,
        false,
    )))
}

/// `Uint8Array.fromHex(string)`：不读 this；奇数长度最前判定抛 SyntaxError。
pub fn uint8array_from_hex<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let string = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !string.is_string() {
        return NativeResult::Err(type_error(vm, "string argument required"));
    }
    let units = vm.string_units(string).into_owned();
    let mut bytes = Vec::new();
    match crate::ta_codec::decode_hex(&units, None, &mut |_, b| bytes.push(b)) {
        Ok(_) => {}
        Err(_) => return NativeResult::Err(crate::error::create_syntax_error(vm, "invalid hex input")),
    }
    let len = bytes.len();
    let buffer = JsValue::from_js_object(new_array_buffer(vm, bytes, 0, default_array_buffer_proto(vm)));
    NativeResult::Ok(JsValue::from_js_object(create_typed_array(
        vm,
        TypedArrayKind::Uint8,
        buffer,
        0,
        len,
        false,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array_buffer::ArrayBufferPayload;

    fn gate(text: &str, length: usize) -> TaIndexGate {
        ta_index_gate_from_text(text, length)
    }

    /// 手工构造带载荷盒的 ArrayBuffer 对象（不经 Vm，只验 live 长度纯核）。
    fn ab_object_with_data(data: Option<Vec<u8>>) -> JsObject {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
        let payload = ArrayBufferPayload {
            data,
            max_byte_length: 0,
            immutable: false,
        };
        let payload_ptr = Box::into_raw(Box::new(payload));
        // SAFETY: 载荷盒形态与 new_array_buffer 的 native_fn 槽存储一致，测试结束前恰好释放一次。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        obj
    }

    /// 手工构造带载荷盒的 SharedArrayBuffer 对象（不经 Vm，只验 live 长度纯核；
    /// 载荷盒形态与 ArrayBuffer 臂同构）。
    fn sab_object_with_data(data: Option<Vec<u8>>) -> JsObject {
        let mut obj = ab_object_with_data(data);
        obj.type_tag = JsObject::OBJ_TYPE_SHARED_ARRAY_BUFFER;
        obj
    }

    fn ta_view(
        buffer: &JsObject, kind: TypedArrayKind, byte_offset: usize, length: usize, auto: bool,
    ) -> TypedArrayData {
        TypedArrayData {
            kind,
            buffer: JsValue::from_js_object(buffer as *const JsObject as *mut JsObject),
            byte_offset,
            length,
            auto_length: auto,
        }
    }

    // J auto 视图 live 长度随 buffer 伸缩（floor 折算、offset 裁剪）。
    #[test]
    fn live_length_auto_tracks_buffer() {
        let full = ab_object_with_data(Some(vec![0; 8]));
        let v = ta_view(&full, TypedArrayKind::Uint8, 0, 8, true);
        assert_eq!(ta_live_length(v), 8);
        assert!(!ta_is_oob(v));
        assert_eq!(ta_spec_length(v), 8);

        let shrunk = ab_object_with_data(Some(vec![0; 6]));
        let v = ta_view(&shrunk, TypedArrayKind::Int32, 0, 8, true);
        assert_eq!(ta_live_length(v), 1, "6 字节缓冲按 4 字节元素 floor 折算为 1");

        let past = ab_object_with_data(Some(vec![0; 2]));
        let v = ta_view(&past, TypedArrayKind::Uint8, 4, 8, true);
        assert!(ta_is_oob(v), "auto offset 超 buffer 即越界");
        assert_eq!(ta_live_length(v), 0);
    }

    // K 定长视图：界内取静态长度；窗口被 buffer 收缩裁掉即越界 0，
    // 规范长度（TypedArrayLength）仍取静态值。
    #[test]
    fn live_length_fixed_static_then_oob() {
        let full = ab_object_with_data(Some(vec![0; 8]));
        let v = ta_view(&full, TypedArrayKind::Uint8, 0, 4, false);
        assert!(!ta_is_oob(v));
        assert_eq!(ta_live_length(v), 4);
        assert_eq!(ta_spec_length(v), 4);

        let shrunk = ab_object_with_data(Some(vec![0; 6]));
        let v = ta_view(&shrunk, TypedArrayKind::Uint16, 0, 4, false);
        assert!(ta_is_oob(v), "窗口尾 8 字节超 6 字节缓冲");
        assert_eq!(ta_live_length(v), 0);
        assert_eq!(ta_spec_length(v), 4, "规范长度对定长视图恒取静态值");
    }

    // L detach：auto/定长同口径越界 0。
    #[test]
    fn live_length_detached_is_zero() {
        let detached = ab_object_with_data(None);
        for auto in [true, false] {
            let v = ta_view(&detached, TypedArrayKind::Uint8, 0, 4, auto);
            assert!(ta_is_oob(v));
            assert_eq!(ta_live_length(v), 0);
            assert_eq!(ta_spec_length(v), 0);
        }
    }

    // M 见证夹取边界：±Infinity 归端点，负值从尾部折算（max(len+k, 0)），
    // 有限值截断后夹到长度。
    #[test]
    fn clamp_index_edges() {
        assert_eq!(clamp_index_to_len(f64::INFINITY, 4), 4);
        assert_eq!(clamp_index_to_len(f64::NEG_INFINITY, 4), 0);
        assert_eq!(clamp_index_to_len(-2.0, 4), 2);
        assert_eq!(clamp_index_to_len(-9.0, 4), 0);
        assert_eq!(clamp_index_to_len(9.0, 4), 4);
        assert_eq!(clamp_index_to_len(2.7, 4), 2);
        assert_eq!(clamp_index_to_len(0.0, 0), 0);
    }

    // A 整数键（length 除注明外取 2）。
    #[test]
    fn gate_text_integer_keys() {
        assert_eq!(gate("0", 2), TaIndexGate::NumericValid(0));
        assert_eq!(gate("1", 2), TaIndexGate::NumericValid(1));
        assert_eq!(gate("0", 1), TaIndexGate::NumericValid(0));
        assert_eq!(gate("2", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("4294967295", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("4294967296", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("007", 2), TaIndexGate::Ordinary);
    }

    // B 小数键：round-trip 成立的非整数归数字无效，round-trip 失败归 Ordinary。
    #[test]
    fn gate_text_fractional_keys() {
        assert_eq!(gate("1.1", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("0.0001", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("0.1", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("0.000001", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("0.30000000000000004", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("1e-7", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("1e-20", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("1e-21", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("-9007199254740992", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("1.0", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("+1", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("-1.0", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("0.0000001", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("1000000000000000000000", 2), TaIndexGate::Ordinary);
        assert_eq!(gate(" 1", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("1 ", 2), TaIndexGate::Ordinary);
        assert_eq!(gate(".5", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("5.", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("1e2", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("1e20", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("1.7976931348623157e308", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("1.7976931348623159e308", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("1e400", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("2e308", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("9007199254740993", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("123456789012345678901234567890", 2), TaIndexGate::Ordinary);
    }

    // C 负数键：round-trip 成立的负数归数字无效，ToNumber=NaN 归 Ordinary。
    #[test]
    fn gate_text_negative_keys() {
        assert_eq!(gate("-1", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("-1", 0), TaIndexGate::NumericInvalid);
        assert_eq!(gate("-0.5", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("--1", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("-", 2), TaIndexGate::Ordinary);
    }

    // D 十六/八/二进制与下划线键：数字对象 ToString 永不输出 0x 前缀，
    // 全部不 round-trip，归 Ordinary。
    #[test]
    fn gate_text_radix_and_underscore_keys() {
        assert_eq!(gate("0x1", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("0X1", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("0o17", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("0b1", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("1_000", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("0x1.8p1", 2), TaIndexGate::Ordinary);
    }

    // E "-0" 特例：字符串相归数字无效（不得 Valid(0)），round-trip 失败的
    // 变体归 Ordinary。
    #[test]
    fn gate_text_minus_zero_special() {
        assert_eq!(gate("-0", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("-0", 0), TaIndexGate::NumericInvalid);
        assert_eq!(gate("-0.0", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("- 0", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("+0", 2), TaIndexGate::Ordinary);
    }

    // F 保留字：NaN/±Infinity 走数字路径（round-trip 成立），拼写偏差归
    // Ordinary。
    #[test]
    fn gate_text_reserved_words() {
        assert_eq!(gate("NaN", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("Infinity", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("-Infinity", 2), TaIndexGate::NumericInvalid);
        assert_eq!(gate("inf", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("INFINITY", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("NaNx", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("Infinity1", 2), TaIndexGate::Ordinary);
    }

    // G 空串与怪文本：ToNumber(+0) 的 round-trip "0" 与原文不等，归 Ordinary。
    #[test]
    fn gate_text_empty_and_odd() {
        assert_eq!(gate("", 2), TaIndexGate::Ordinary);
        assert_eq!(gate(" ", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("\u{3000}", 2), TaIndexGate::Ordinary);
    }

    // H 2^32 溢出与最大长度边界（最大长度取 1 字节 TA 上界 2^30）。
    #[test]
    fn gate_text_2pow32_and_max_length() {
        assert_eq!(gate("4294967295", 1073741824), TaIndexGate::NumericInvalid);
        assert_eq!(gate("1073741823", 1073741824), TaIndexGate::NumericValid(1073741823));
        assert_eq!(gate("1073741824", 1073741824), TaIndexGate::NumericInvalid);
        // 2^64 的规范 ToString 是最短回环式 "18446744073709552000"（V8 实测），
        // 与原文不等，round-trip 不成立归 Ordinary（建真实属性）。
        assert_eq!(gate("18446744073709551616", 2), TaIndexGate::Ordinary);
        assert_eq!(gate("99999999999999999999999", 2), TaIndexGate::Ordinary);
    }

    // I int-key 臂（纯核）。
    #[test]
    fn gate_int_arm() {
        assert_eq!(ta_index_gate_int(0, 2), TaIndexGate::NumericValid(0));
        assert_eq!(ta_index_gate_int(2, 2), TaIndexGate::NumericInvalid);
    }

    // M SAB 双认：live 长度/越界判定/规范长度对 SAB 载荷与 AB 同口径
    // （SAB 无 detach 产出路径，载荷恒在）。
    #[test]
    fn live_length_sab_dual_arm() {
        let sab = sab_object_with_data(Some(vec![0; 8]));
        let v = ta_view(&sab, TypedArrayKind::Int32, 4, 1, false);
        assert!(!ta_is_oob(v));
        assert_eq!(ta_live_length(v), 1);
        assert_eq!(ta_spec_length(v), 1);

        let v = ta_view(&sab, TypedArrayKind::Uint8, 0, 8, true);
        assert_eq!(ta_live_length(v), 8);
        assert!(!ta_is_oob(v));

        // 界内 auto 视图：live 长按 buffer 当前字节折算裁剪。
        let v = ta_view(&sab, TypedArrayKind::Uint8, 4, 8, true);
        assert!(!ta_is_oob(v));
        assert_eq!(ta_live_length(v), 4);

        // offset 超 SAB 缓冲即越界 0（与 AB 臂同口径）。
        let v = ta_view(&sab, TypedArrayKind::Uint8, 12, 8, true);
        assert!(ta_is_oob(v));
        assert_eq!(ta_live_length(v), 0);
    }

    // N 双认标签之外的对象：同 AB 口径越界 0。
    #[test]
    fn live_length_non_buffer_is_oob() {
        let plain = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        let v = ta_view(&plain, TypedArrayKind::Uint8, 0, 4, true);
        assert!(ta_is_oob(v));
        assert_eq!(ta_live_length(v), 0);
    }
}
