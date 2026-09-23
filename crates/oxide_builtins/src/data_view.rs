use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use num_bigint::BigInt;
use num_traits::ToPrimitive;

use crate::array_buffer::{array_buffer_payload, ArrayBufferPayload};

use oxide_runtime_api::{NativeResult, VmHost};

/// DataView 视图状态盒。`byte_length` 语义随 `length_is_auto` 分流：
/// 定长视图存静态长；auto 视图（构造时 byteLength 缺省且构造期缓冲可 resize）
/// 存构造期 live − offset 初值，活读一律现算 live − offset。
#[derive(Clone, Copy)]
pub(crate) struct DataViewData {
    buffer: JsValue,
    byte_offset: usize,
    byte_length: usize,
    length_is_auto: bool,
}

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

/// ToIndex（ECMA-262 §7.1.19）：ToNumber → NaN 归 0 → 非有限抛 RangeError → trunc →
/// 负值抛 RangeError。
///
/// # 边界与前提
/// - 序位按规范步序：trunc 先于判负（-0.99999 归 0，不抛）。
/// - 强转失败（对象 valueOf/toString 抛错、Symbol、BigInt）以 `Err` 传播原异常值，
///   调用方不得吞成 NaN。
fn to_index<H: VmHost>(vm: &mut H, value: JsValue, msg: &str) -> Result<usize, JsValue> {
    let n = vm
        .coerce_number_bounded(value)
        .map_err(|e| crate::iterator::engine_error(vm, &e))?;
    if n.is_nan() {
        return Ok(0);
    }
    if !n.is_finite() {
        return Err(crate::error::create_range_error(vm, msg));
    }
    let truncated = n.trunc();
    if truncated < 0.0 {
        return Err(crate::error::create_range_error(vm, msg));
    }
    Ok(truncated as usize)
}

fn is_little_endian<H: VmHost>(vm: &mut H, args: &[u8], idx: usize) -> bool {
    args.get(idx)
        .map(|reg| oxide_runtime_api::to_boolean(vm.reg(*reg)))
        .unwrap_or(false)
}

/// 可选 byteOffset 实参的 ToIndex（缺省 → 0），强转失败传播。
fn to_index_arg<H: VmHost>(vm: &mut H, args: &[u8], idx: usize) -> Result<usize, JsValue> {
    if args.len() > idx {
        to_index(vm, vm.reg(args[idx]), "DataView offset out of bounds")
    } else {
        Ok(0)
    }
}

/// value 实参的 ToNumber（缺省 → undefined → NaN），强转失败传播。
fn coerce_value_arg<H: VmHost>(vm: &mut H, args: &[u8], idx: usize) -> Result<f64, JsValue> {
    let value = if args.len() > idx { vm.reg(args[idx]) } else { JsValue::undefined() };
    vm.coerce_number_bounded(value)
        .map_err(|e| crate::iterator::engine_error(vm, &e))
}

/// ToInt32 回卷：0/±Infinity 归 0，trunc 后对 2^32 取模再按符号解释（位模式口径）。
fn to_int32(n: f64) -> i32 {
    if n == 0.0 || !n.is_finite() {
        return 0;
    }
    let int = n.trunc().rem_euclid(4_294_967_296.0) as u32;
    if int > i32::MAX as u32 {
        (int as i64 - 4_294_967_296i64) as i32
    } else {
        int as i32
    }
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

/// 视图界判核（规范 IsViewOutOfBounds 与 GetViewByteLength 合一）：缓冲已
/// detach（载荷 `data` 为 `None`）、auto 视图 offset 超 live、定长视图
/// offset + 静态长超 live（含溢出）→ TypeError；通过时返回视图长
/// （auto 视图现算 live − offset，定长视图返静态长）。
fn dv_view_bounds<H: VmHost>(
    vm: &mut H, payload_ptr: *mut ArrayBufferPayload, view: DataViewData,
) -> Result<usize, JsValue> {
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let Some(data) = unsafe { &*payload_ptr }.data.as_deref() else {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let live = data.len();
    if view.length_is_auto {
        if view.byte_offset > live {
            return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
        }
        return Ok(live - view.byte_offset);
    }
    match view.byte_offset.checked_add(view.byte_length) {
        Some(end) if end <= live => Ok(view.byte_length),
        _ => Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid")),
    }
}

/// set 面入口写守卫：immutable 缓冲 → TypeError（先于一切实参强转）。
fn dv_buffer_writable<H: VmHost>(vm: &mut H, view: DataViewData) -> Result<(), JsValue> {
    let payload_ptr = array_buffer_payload(vm, view.buffer)?;
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    if unsafe { &*payload_ptr }.immutable {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer is immutable"));
    }
    Ok(())
}

/// JS 调用点后的缓冲重取纪律：重读寄存器再重取载荷（epoch 晋升改写
/// 寄存器，旧 JsValue 滞留 epoch 克隆旧盒）；detached → TypeError。
/// 返回（寄存器重读值、live 长、存储上限）。
fn revalidate_buffer<H: VmHost>(vm: &mut H, buf_reg: u8) -> Result<(JsValue, usize, usize), JsValue> {
    let buffer = vm.reg(buf_reg);
    let payload_ptr = array_buffer_payload(vm, buffer)?;
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let payload = unsafe { &*payload_ptr };
    let Some(data) = payload.data.as_deref() else {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    Ok((buffer, data.len(), payload.max_byte_length))
}

/// GetViewValue 后半：按已校验的视图与偏移读 N 字节。界判核先
/// （detach/OOB → TypeError，live 长口径），后视图相对越界
/// （→ RangeError，判活视图长）。
fn read_bytes_at<const N: usize, H: VmHost>(vm: &mut H, view: DataViewData, offset: usize) -> Result<[u8; N], JsValue> {
    let payload_ptr = array_buffer_payload(vm, view.buffer)?;
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let Some(buffer) = unsafe { &*payload_ptr }.data.as_deref() else {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let view_len = dv_view_bounds(vm, payload_ptr, view)?;
    match offset.checked_add(N) {
        Some(end) if end <= view_len => {}
        _ => return Err(crate::error::create_range_error(vm, "DataView offset out of bounds")),
    }
    let abs = view.byte_offset + offset;
    let mut out = [0u8; N];
    out.copy_from_slice(&buffer[abs..abs + N]);
    Ok(out)
}

/// SetViewValue 后半：按已校验的视图与偏移写 N 字节（读路径同序）。
fn write_bytes<const N: usize, H: VmHost>(
    vm: &mut H, view: DataViewData, offset: usize, bytes: [u8; N],
) -> Result<(), JsValue> {
    let payload_ptr = array_buffer_payload(vm, view.buffer)?;
    // 界判核共享借用先于可变借用建立，避免活别名。
    let view_len = dv_view_bounds(vm, payload_ptr, view)?;
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷；
    // 界判核已证 data 在场，下方守卫为结构性兜底。
    let Some(buffer) = unsafe { &mut *payload_ptr }.data.as_deref_mut() else {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    match offset.checked_add(N) {
        Some(end) if end <= view_len => {}
        _ => return Err(crate::error::create_range_error(vm, "DataView offset out of bounds")),
    }
    let abs = view.byte_offset + offset;
    buffer[abs..abs + N].copy_from_slice(&bytes);
    Ok(())
}

/// 校验 receiver 原型链含 `%DataView.prototype%`（`new DataView` 构造出的实例
/// 特征，普通调用面 this = globalThis 不满足）。
fn has_data_view_proto<H: VmHost>(vm: &mut H, this_val: JsValue) -> bool {
    if !this_val.is_object() {
        return false;
    }
    let target = vm.session().builtin_world().data_view_proto.as_ptr() as *mut JsObject;
    let mut cursor = this_val;
    for _ in 0..1024 {
        if !cursor.is_object() {
            return false;
        }
        let ptr = cursor.as_js_object_ptr();
        if ptr.is_null() {
            return false;
        }
        if std::ptr::eq(ptr, target) {
            return true;
        }
        cursor = unsafe { &*ptr }.proto();
    }
    false
}

/// `DataView(buffer, byteOffset, byteLength)` 构造逻辑：在 ArrayBuffer 上建立定点字节视图。
///
/// # 步骤
/// 1. receiver 原型链不含 %DataView.prototype% → TypeError（普通调用面：
///    this = globalThis 不满足；构造路径恒传 DataView 原型链上的 receiver）。
/// 2. buffer 品牌校验（ArrayBufferData）；首个载荷取用后即弃指针，
///    不持借用跨 JS 调用。
/// 3. offset = ToIndex(byteOffset) 传播式；其后重取（重读寄存器 + 载荷重取）：
///    detached 缓冲 → TypeError（在偏移强转之后）；offset > live → RangeError。
/// 4. byteLength 缺省/undefined → 缺省长 live − offset，auto 位按构造期缓冲
///    resizable 定；否则 ToIndex 传播式，其后重取按新 live 复验
///    offset + byteLength → RangeError。
/// 5. GetPrototypeFromConstructor：经 `ordinary_get` 读 NewTarget（regs[255]）
///    的 `prototype`（触发访问器，读期异常原值传播）；非对象回落 %DataView.prototype%。
/// 6. 终校验（proto 读后再重取一次）：detached → TypeError；offset > live' →
///    RangeError；显式长 offset + byteLength > live' → RangeError；auto 视图
///    存储长按 live' − offset 重算。
/// 7. 分配 OBJ_TYPE_DATA_VIEW 对象并挂 DataViewData 状态盒（终校验先于
///    分配，raise 路径无半成品对象）。
///
/// # 边界与前提
/// - 原型读取在全部初校验之后：越界 RangeError 先于抛错的 prototype getter。
/// - NewTarget 非对象（直调/缺省面）时直接回落默认原型，不抛错。
/// - 每个 JS 调用点后重取缓冲（重读寄存器 + 载荷重取）：晋升改写寄存器，
///   旧值滞留 epoch 克隆旧盒。
pub fn data_view_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !has_data_view_proto(vm, this_val) {
        return NativeResult::Err(crate::error::create_type_error(vm, "DataView must be called with new"));
    }
    let new_target = vm.reg(255);
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "DataView requires an ArrayBuffer"));
    }
    let buffer = vm.reg(args[1]);
    // 首个载荷取用只做品牌校验，指针即弃，不持借用跨 JS 调用。
    native_try!(array_buffer_payload(vm, buffer));

    let byte_offset = if args.len() > 2 {
        native_try!(to_index(vm, vm.reg(args[2]), "DataView byteOffset out of bounds"))
    } else {
        0
    };

    // 偏移强转后的重取：detached 缓冲构造抛 TypeError，且偏移的 ToNumber
    // 已恰好发生一次（规范序）。
    let (_, live, max_byte_length) = native_try!(revalidate_buffer(vm, args[1]));
    if byte_offset > live {
        return NativeResult::Err(crate::error::create_range_error(vm, "DataView byteOffset out of bounds"));
    }

    // byteLength 显式 undefined 视同缺省（ToIndex(undefined) = 0 会吞掉视图长度）；
    // 缺省臂 auto 位按构造期缓冲 resizable 判定。
    let explicit = args.len() > 3 && !vm.reg(args[3]).is_undefined();
    let length_is_auto = !explicit && max_byte_length != 0;
    let mut byte_length = live - byte_offset;
    if explicit {
        byte_length = native_try!(to_index(vm, vm.reg(args[3]), "DataView byteLength out of bounds"));
        // byteLength 强转后的重取，按新 live 复验。
        let (_, live, _) = native_try!(revalidate_buffer(vm, args[1]));
        if byte_offset + byte_length > live {
            return NativeResult::Err(crate::error::create_range_error(vm, "DataView byteLength out of bounds"));
        }
    }

    // GetPrototypeFromConstructor：普通读触发原型链上的访问器 getter，
    // 读期用户异常经原值槽恢复后重抛。NewTarget 非对象（直调/缺省面）时
    // 回落默认原型，不抛错。
    let default_proto = JsValue::from_js_object(vm.session().builtin_world().data_view_proto.as_ptr() as *mut JsObject);
    let proto = if new_target.is_object() {
        let nt_ptr = new_target.as_js_object_ptr();
        let nt_obj = unsafe { &*nt_ptr };
        let proto_si = vm.kernel_core().perm_interner().intern("prototype").0;
        let proto_val = match vm.ordinary_get(nt_obj, proto_si, new_target) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
        if proto_val.is_object() {
            proto_val
        } else {
            default_proto
        }
    } else {
        default_proto
    };

    // 终校验：proto getter 是用户代码窗（detach/resize 可发生），再重取一次，
    // 全部判定用 live'；auto 视图存储长按 live' − offset 重算。
    let (buffer, live, _) = native_try!(revalidate_buffer(vm, args[1]));
    if byte_offset > live {
        return NativeResult::Err(crate::error::create_range_error(vm, "DataView byteOffset out of bounds"));
    }
    if !length_is_auto && byte_offset + byte_length > live {
        return NativeResult::Err(crate::error::create_range_error(vm, "DataView byteLength out of bounds"));
    }
    let byte_length = if length_is_auto { live - byte_offset } else { byte_length };

    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_DATA_VIEW;
    let data = Box::into_raw(Box::new(DataViewData {
        buffer,
        byte_offset,
        byte_length,
        length_is_auto,
    }));
    // SAFETY: DataView 对象不可调用，native_fn 存不透明 `Box<DataViewData>` 指针，
    // 与 ArrayBuffer / RegExp 的类型化存储模式一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(data as *const ()) }));
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
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<1, H>(vm, view, offset));
    NativeResult::Ok(JsValue::int(i8::from_ne_bytes(bytes) as i32))
}

/// `DataView.prototype.getUint8(byteOffset)`：读取 1 字节无符号整数。
pub fn data_view_get_uint8<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<1, H>(vm, view, offset));
    NativeResult::Ok(JsValue::int(bytes[0] as i32))
}

/// `DataView.prototype.getInt16(byteOffset, littleEndian)`：读取 2 字节有符号整数。
pub fn data_view_get_int16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<2, H>(vm, view, offset));
    let n = if is_little_endian(vm, args, 2) {
        i16::from_le_bytes(bytes)
    } else {
        i16::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n as i32))
}

/// `DataView.prototype.getUint16(byteOffset, littleEndian)`：读取 2 字节无符号整数。
pub fn data_view_get_uint16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<2, H>(vm, view, offset));
    let n = if is_little_endian(vm, args, 2) {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n as i32))
}

/// `DataView.prototype.getInt32(byteOffset, littleEndian)`：读取 4 字节有符号整数。
pub fn data_view_get_int32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<4, H>(vm, view, offset));
    let n = if is_little_endian(vm, args, 2) {
        i32::from_le_bytes(bytes)
    } else {
        i32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::int(n))
}

/// `DataView.prototype.getUint32(byteOffset, littleEndian)`：读取 4 字节无符号整数。
pub fn data_view_get_uint32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<4, H>(vm, view, offset));
    let n = if is_little_endian(vm, args, 2) {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n as f64))
}

/// `DataView.prototype.getFloat32(byteOffset, littleEndian)`：读取 4 字节单精度浮点。
pub fn data_view_get_float32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<4, H>(vm, view, offset));
    let n = if is_little_endian(vm, args, 2) {
        f32::from_le_bytes(bytes)
    } else {
        f32::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(n as f64))
}

/// `DataView.prototype.getFloat16(byteOffset, littleEndian)`：读取 2 字节半精度浮点。
pub fn data_view_get_float16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<2, H>(vm, view, offset));
    let bits = if is_little_endian(vm, args, 2) {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    };
    NativeResult::Ok(JsValue::float(crate::math::f16_bits_to_f64(bits)))
}

/// `DataView.prototype.getFloat64(byteOffset, littleEndian)`：读取 8 字节双精度浮点。
pub fn data_view_get_float64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<8, H>(vm, view, offset));
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
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<8, H>(vm, view, offset));
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
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let bytes = native_try!(read_bytes_at::<8, H>(vm, view, offset));
    let n = if is_little_endian(vm, args, 2) {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    };
    NativeResult::Ok(vm.new_bigint(BigInt::from(n)))
}

/// `DataView.prototype.setInt8(byteOffset, value)`：写入 1 字节有符号整数。
///
/// 步序：品牌 → immutable 写守卫 → ToIndex(byteOffset) → ToInt32(value)
/// 回卷 → 越界 RangeError → 写。
pub fn data_view_set_int8<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let n = to_int32(native_try!(coerce_value_arg(vm, args, 2)));
    native_try!(write_bytes::<1, H>(vm, view, offset, [n as u8]));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setUint8(byteOffset, value)`：写入 1 字节无符号整数。
pub fn data_view_set_uint8<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let n = to_int32(native_try!(coerce_value_arg(vm, args, 2)));
    native_try!(write_bytes::<1, H>(vm, view, offset, [n as u8]));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setInt16(byteOffset, value, littleEndian)`：写入 2 字节有符号整数。
pub fn data_view_set_int16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let n = to_int32(native_try!(coerce_value_arg(vm, args, 2)));
    let bytes = if is_little_endian(vm, args, 3) {
        (n as u16).to_le_bytes()
    } else {
        (n as u16).to_be_bytes()
    };
    native_try!(write_bytes::<2, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setUint16(byteOffset, value, littleEndian)`：写入 2 字节无符号整数。
pub fn data_view_set_uint16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let n = to_int32(native_try!(coerce_value_arg(vm, args, 2)));
    let bytes = if is_little_endian(vm, args, 3) {
        (n as u16).to_le_bytes()
    } else {
        (n as u16).to_be_bytes()
    };
    native_try!(write_bytes::<2, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setInt32(byteOffset, value, littleEndian)`：写入 4 字节有符号整数。
pub fn data_view_set_int32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let n = to_int32(native_try!(coerce_value_arg(vm, args, 2)));
    let bytes = if is_little_endian(vm, args, 3) { n.to_le_bytes() } else { n.to_be_bytes() };
    native_try!(write_bytes::<4, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setUint32(byteOffset, value, littleEndian)`：写入 4 字节无符号整数。
pub fn data_view_set_uint32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let n = to_int32(native_try!(coerce_value_arg(vm, args, 2)));
    let bytes = if is_little_endian(vm, args, 3) {
        (n as u32).to_le_bytes()
    } else {
        (n as u32).to_be_bytes()
    };
    native_try!(write_bytes::<4, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setFloat32(byteOffset, value, littleEndian)`：写入单精度浮点。
pub fn data_view_set_float32<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let value = native_try!(coerce_value_arg(vm, args, 2)) as f32;
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes::<4, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setFloat16(byteOffset, value, littleEndian)`：按 IEEE 754
/// binary16 舍入写入 2 字节半精度浮点（NaN 规范化由 `f64_to_f16_bits` 处理）。
pub fn data_view_set_float16<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let value = native_try!(coerce_value_arg(vm, args, 2));
    let bits = crate::math::f64_to_f16_bits(value);
    let bytes = if is_little_endian(vm, args, 3) {
        bits.to_le_bytes()
    } else {
        bits.to_be_bytes()
    };
    native_try!(write_bytes::<2, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setFloat64(byteOffset, value, littleEndian)`：写入双精度浮点。
pub fn data_view_set_float64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let value = native_try!(coerce_value_arg(vm, args, 2));
    let bytes = if is_little_endian(vm, args, 3) {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    native_try!(write_bytes::<8, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setBigInt64(byteOffset, value, littleEndian)`：按 ToBigInt
/// 语义接收值（Number 入参抛 TypeError），取低 64 位位模式写入 8 字节有符号整数。
///
/// 步序：品牌 → immutable 写守卫 → ToIndex(byteOffset) → ToBigInt(value)
/// → 越界 RangeError → 写。
pub fn data_view_set_big_int64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let b =
        native_try!(oxide_runtime_api::to_bigint_full(value, vm).map_err(|e| crate::iterator::engine_error(vm, &e)));
    let low = low64(vm, b) as i64;
    let bytes = if is_little_endian(vm, args, 3) {
        low.to_le_bytes()
    } else {
        low.to_be_bytes()
    };
    native_try!(write_bytes::<8, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// `DataView.prototype.setBigUint64(byteOffset, value, littleEndian)`：按 ToBigInt
/// 语义接收值（Number 入参抛 TypeError），取低 64 位位模式写入 8 字节无符号整数。
pub fn data_view_set_big_uint64<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    native_try!(dv_buffer_writable(vm, view));
    let offset = native_try!(to_index_arg(vm, args, 1));
    let value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let b =
        native_try!(oxide_runtime_api::to_bigint_full(value, vm).map_err(|e| crate::iterator::engine_error(vm, &e)));
    let low = low64(vm, b);
    let bytes = if is_little_endian(vm, args, 3) {
        low.to_le_bytes()
    } else {
        low.to_be_bytes()
    };
    native_try!(write_bytes::<8, H>(vm, view, offset, bytes));
    NativeResult::Ok(JsValue::undefined())
}

/// 取 BigInt 值低 64 位位模式：与 2^64-1 掩码后恒非负且可转 u64。
fn low64<H: VmHost>(vm: &mut H, val: JsValue) -> u64 {
    let v = vm.bigint_value(val);
    (v & (BigInt::from(u64::MAX))).to_u64().unwrap_or(u64::MAX)
}

/// `DataView.prototype.toString`：校验 receiver 后返回 `[object DataView]`。
pub fn data_view_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    native_try!(get_data_view_data(vm, this_val));
    NativeResult::Ok(vm.new_string("[object DataView]"))
}

/// `DataView.prototype.buffer` getter：返回该视图所锚定的 ArrayBuffer 对象。
pub fn data_view_buffer_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    NativeResult::Ok(view.buffer)
}

/// `DataView.prototype.byteOffset` getter：返回视图起始字节在 buffer 内的偏移。
/// 缓冲已 detach 或视图越界（live 长口径）→ TypeError。
pub fn data_view_byte_offset_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let payload_ptr = native_try!(array_buffer_payload(vm, view.buffer));
    native_try!(dv_view_bounds(vm, payload_ptr, view));
    NativeResult::Ok(JsValue::float(view.byte_offset as f64))
}

/// `DataView.prototype.byteLength` getter：返回视图覆盖的字节长度。
/// 缓冲已 detach 或视图越界 → TypeError；auto 视图返 live − offset 活读
/// （存储值仅构造期语义）。
pub fn data_view_byte_length_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let view = native_try!(get_data_view_data(vm, this_val));
    let payload_ptr = native_try!(array_buffer_payload(vm, view.buffer));
    let view_len = native_try!(dv_view_bounds(vm, payload_ptr, view));
    NativeResult::Ok(JsValue::float(view_len as f64))
}
