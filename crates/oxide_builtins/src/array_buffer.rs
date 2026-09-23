use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::private_key::{make_well_known_symbol_key, WELL_KNOWN_SYMBOL_SPECIES};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::array::to_integer_or_infinity_bounded;

pub(crate) const MAX_ARRAY_BUFFER_LENGTH: usize = 1 << 30;

/// ArrayBuffer 载荷：字节缓冲与状态位。`data` 为 `None` 即缓冲区已 detach；
/// `max_byte_length` 为存储态上限：0 即定长缓冲（resizable 判据），非 0 存
/// 请求上限 + 1（上限 0 与定长必须可分）；`immutable` 为字节缓冲写守卫标志。
/// 载荷经 `Box::into_raw` 存于对象 `native_fn` 槽，GC 三自由函数
/// （clone/size/drop）对整结构体操作，克隆须连标志位一并拷贝。
#[derive(Clone)]
pub(crate) struct ArrayBufferPayload {
    pub(crate) data: Option<Vec<u8>>,
    pub(crate) max_byte_length: usize,
    pub(crate) immutable: bool,
}

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

/// ToIndex：undefined → 0；ToInteger 传播式（NaN → +0、trunc、±Infinity
/// 保留）；负值与 ToLength 夹取后不可同值表示（≥ 2^53，含 +Infinity）→
/// RangeError。分配期上界（引擎上限）不在本阶段校验。
fn to_index<H: VmHost>(vm: &mut H, value: JsValue) -> Result<usize, JsValue> {
    if value.is_undefined() {
        return Ok(0);
    }
    let n = match vm.coerce_number_bounded(value) {
        Ok(n) => n,
        Err(err) => return Err(crate::iterator::engine_error(vm, &err)),
    };
    // ToInteger：NaN → +0，±Infinity 原样，有限值截断。
    let integer = if n.is_nan() { 0.0 } else { n.trunc() };
    if integer < 0.0 {
        return Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    // ToLength 夹取到 2^53-1：≥ 2^53 的整数与夹取值不可同值，拒绝。
    if integer >= 9_007_199_254_740_992.0_f64 {
        return Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    Ok(integer as usize)
}

/// ToIntegerOrInfinity：ToNumber 传播式（用户转换异常原值恢复），NaN → +0、
/// trunc，±Infinity 保留——界判由调用方上界比较收口。
fn to_integer_or_infinity<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    let n = match vm.coerce_number_bounded(value) {
        Ok(n) => n,
        Err(err) => return Err(crate::iterator::engine_error(vm, &err)),
    };
    Ok(if n.is_nan() { 0.0 } else { n.trunc() })
}

/// ToIntegerOrInfinity：ToNumber 传播式（BigInt/Symbol/转换异常向上传播），
/// NaN → 0、trunc、±Infinity 保留——负值归 0、正值归 len 由调用侧折叠。
fn normalize_index<H: VmHost>(vm: &mut H, value: JsValue, len: usize) -> Result<usize, JsValue> {
    let n = to_integer_or_infinity_bounded(vm, value)?;
    // ±Infinity 不截断，负无穷归 0、正无穷归 len，
    // 避免饱和成 isize::MIN 后取负溢出。
    if n.is_infinite() {
        return Ok(if n < 0.0 { 0 } else { len });
    }
    let int = n.trunc() as isize;
    if int < 0 {
        Ok(len.saturating_sub(int.unsigned_abs()))
    } else {
        Ok((int as usize).min(len))
    }
}

/// 默认缓冲区原型 %ArrayBuffer.prototype%（构造器回落路径与各定长建点共用）。
pub(crate) fn default_array_buffer_proto<H: VmHost>(vm: &H) -> JsValue {
    JsValue::from_js_object(vm.session().builtin_world().array_buffer_proto.as_ptr() as *mut JsObject)
}

/// 分配携给定 proto 与载荷形态（`max_byte_length` 为存储态：0 定长，非 0 可
/// resize）的 ArrayBuffer 对象；byteLength 经原型访问器读（不写 own 数据属性）。
pub(crate) fn new_array_buffer<H: VmHost>(
    vm: &mut H, data: Vec<u8>, max_byte_length: usize, proto: JsValue,
) -> *mut JsObject {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
    let payload = ArrayBufferPayload {
        data: Some(data),
        max_byte_length,
        immutable: false,
    };
    let payload_ptr = Box::into_raw(Box::new(payload));
    // SAFETY: ArrayBuffer 对象不可调用，故 native_fn 槽复用作不透明载荷盒
    // 指针，与既有 RegExp 存储模式一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
    vm.alloc_object(obj)
}

pub(crate) fn array_buffer_payload_ptr(obj: &JsObject) -> Option<*mut ArrayBufferPayload> {
    if !obj.is_array_buffer_obj() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut ArrayBufferPayload)
}

/// 克隆 ArrayBuffer 载荷（字节缓冲 + 状态位）到新对象（跨 epoch 克隆流程用）。
pub fn clone_array_buffer_native(old_obj: &JsObject, new_obj: &mut JsObject) {
    let Some(ptr) = array_buffer_payload_ptr(old_obj) else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    // SAFETY: ptr 非空且指向存活载荷盒（克隆/晋升臂由 GC 持有源对象）。
    let cloned = unsafe { &*ptr }.clone();
    let cloned_ptr = Box::into_raw(Box::new(cloned));
    new_obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(cloned_ptr as *const ()) }));
}

/// 只读核算 ArrayBuffer 载荷字节（不释放）：结构体尺寸 + 附着缓冲容量，
/// detached 载荷仅计结构体尺寸。
pub fn array_buffer_native_size(obj: &JsObject) -> u64 {
    let Some(payload_ptr) = array_buffer_payload_ptr(obj) else {
        return 0;
    };
    if payload_ptr.is_null() {
        return 0;
    }
    // SAFETY: payload_ptr 非空且指向存活载荷盒。
    let payload = unsafe { &*payload_ptr };
    (std::mem::size_of::<ArrayBufferPayload>() + payload.data.as_ref().map_or(0, |d| d.capacity())) as u64
}

/// 释放 ArrayBuffer 载荷盒（native_fn 槽），返回字节数；槽置空后重复调用零释放。
pub fn drop_array_buffer_native(obj: &mut JsObject) -> u64 {
    let bytes = array_buffer_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(payload_ptr) = array_buffer_payload_ptr(obj) else {
        return 0;
    };
    // SAFETY: payload_ptr 非空（array_buffer_native_size 已验证），Box::from_raw 恰好释放一次。
    let payload = unsafe { Box::from_raw(payload_ptr) };
    drop(payload);
    obj.set_native_fn(None);
    bytes
}

/// 分配定长 SharedArrayBuffer 对象（零填充字节缓冲，`max_byte_length` 存 0）。
pub(crate) fn new_shared_array_buffer<H: VmHost>(vm: &mut H, data: Vec<u8>) -> *mut JsObject {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    obj.type_tag = JsObject::OBJ_TYPE_SHARED_ARRAY_BUFFER;
    let payload = ArrayBufferPayload {
        data: Some(data),
        max_byte_length: 0,
        immutable: false,
    };
    let payload_ptr = Box::into_raw(Box::new(payload));
    // SAFETY: SharedArrayBuffer 对象不可调用，native_fn 槽复用为不透明载荷盒
    // 指针，与 ArrayBuffer 存储模式一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
    vm.alloc_object(obj)
}

pub(crate) fn shared_array_buffer_payload_ptr(obj: &JsObject) -> Option<*mut ArrayBufferPayload> {
    if !obj.is_shared_array_buffer_obj() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut ArrayBufferPayload)
}

/// SharedArrayBuffer 载荷盒字节数（`native_fn` 槽）；非 SAB 或已释放 → 0。
pub fn shared_array_buffer_native_size(obj: &JsObject) -> u64 {
    let payload_ptr = match shared_array_buffer_payload_ptr(obj) {
        Some(p) => p,
        None => return 0,
    };
    if payload_ptr.is_null() {
        return 0;
    }
    // SAFETY: payload_ptr 非空且指向存活载荷盒。
    let payload = unsafe { &*payload_ptr };
    (std::mem::size_of::<ArrayBufferPayload>() + payload.data.as_ref().map_or(0, |d| d.capacity())) as u64
}

/// 释放 SharedArrayBuffer 载荷盒（native_fn 槽），返回字节数；槽置空后重复调用零释放。
pub fn drop_shared_array_buffer_native(obj: &mut JsObject) -> u64 {
    let bytes = shared_array_buffer_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(payload_ptr) = shared_array_buffer_payload_ptr(obj) else {
        return 0;
    };
    // SAFETY: payload_ptr 非空（size 已验证），Box::from_raw 恰好释放一次。
    let payload = unsafe { Box::from_raw(payload_ptr) };
    drop(payload);
    obj.set_native_fn(None);
    bytes
}

/// 克隆 SharedArrayBuffer 载荷（字节缓冲）到新对象（跨 epoch 克隆流程用）。
pub fn clone_shared_array_buffer_native(old_obj: &JsObject, new_obj: &mut JsObject) {
    let Some(ptr) = shared_array_buffer_payload_ptr(old_obj) else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    // SAFETY: ptr 非空且指向存活载荷盒（克隆/晋升臂由 GC 持有源对象）。
    let cloned = unsafe { &*ptr }.clone();
    let cloned_ptr = Box::into_raw(Box::new(cloned));
    new_obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(cloned_ptr as *const ()) }));
}

/// `SharedArrayBuffer(length)`：构造语义仅 `new`；length 缺省 0，经 ToIndex
/// 传播式（强转副作用原值上抛），超引擎上界 → RangeError。最小实现：字节
/// 缓冲零填充定长，无 resizable 选项面与 `[[ArrayBufferMaxByteLength]]`。
pub fn shared_array_buffer_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if !vm.constructing_native() {
        return NativeResult::Err(crate::error::create_type_error(vm, "SharedArrayBuffer must be called with new"));
    }
    let length = if args.len() > 1 { native_try!(to_index(vm, vm.reg(args[1]))) } else { 0 };
    if length > MAX_ARRAY_BUFFER_LENGTH {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid SharedArrayBuffer length"));
    }
    let obj_ptr = new_shared_array_buffer(vm, vec![0; length]);
    NativeResult::Ok(JsValue::from_js_object(obj_ptr))
}

/// 读入口：校验 receiver 为 ArrayBuffer 并取载荷指针。detach（载荷 `data` 为
/// `None`）不在此分叉，由消费方现读 `data` 时按既有错误形态处理。
pub(crate) fn array_buffer_payload<H: VmHost>(
    vm: &mut H, this_val: JsValue,
) -> Result<*mut ArrayBufferPayload, JsValue> {
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
    let Some(payload_ptr) = obj.native_fn() else {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let payload_ptr = payload_ptr.as_ptr() as *mut ArrayBufferPayload;
    if payload_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    }
    Ok(payload_ptr)
}

/// `ArrayBuffer(length [ , options ])` 构造逻辑。
///
/// # 步骤
/// 1. 非构造形态（普通调用，new.target 缺失）→ TypeError。
/// 2. length = ToIndex(length) 传播式；undefined/缺省 → 0。
/// 3. GetArrayBufferMaxByteLengthOption：options 非对象 → 0；Get
///    "maxByteLength" 传播式；undefined → 0；否则 ToIndex 传播式。
/// 4. length > max → RangeError（先于原型读与对象创建）。
/// 5. GetPrototypeFromConstructor：经 ordinary_get 读 new.target 的
///    "prototype"（访问器触发，异常原值传播）；非对象结果回落
///    %ArrayBuffer.prototype%。
/// 6. 对象创建（定长/可 resize 由选项是否在场决定）；分配期上界校验
///    （length 与 max 各一条，超引擎上限 → RangeError）。
/// 7. 选项在场 → own maxByteLength 数据属性 {w:0, e:0, c:0}（值为 ToIndex
///    后整数）。
pub fn array_buffer_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if !vm.constructing_native() {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer must be called with new"));
    }
    let length = if args.len() > 1 { native_try!(to_index(vm, vm.reg(args[1]))) } else { 0 };
    // GetArrayBufferMaxByteLengthOption：None = 空（定长），Some(v) = 请求的
    // 上限（v 可为 0，与"空"语义相异：length > v 的界判与 own 属性均须生效）。
    let max_opt = if args.len() > 2 {
        let options = vm.reg(args[2]);
        if !options.is_object() {
            None
        } else {
            let options_ptr = options.as_js_object_ptr();
            // SAFETY: is_object 保证指针非空且对象本 session 存活。
            let options_obj = unsafe { &*options_ptr };
            let si = vm.kernel_core().perm_interner().intern("maxByteLength").0;
            let max_val = match vm.ordinary_get(options_obj, si, options) {
                Ok(v) => v,
                Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
            };
            if max_val.is_undefined() {
                None
            } else {
                Some(native_try!(to_index(vm, max_val)))
            }
        }
    } else {
        None
    };
    if let Some(max) = max_opt {
        if length > max {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
        }
    }
    // GetPrototypeFromConstructor：prototype getter 可抛错，异常原值传播；
    // 结果非对象时回落默认原型。
    let default_proto = default_array_buffer_proto(vm);
    let new_target = vm.reg(255);
    let proto = if new_target.is_object() {
        let nt_ptr = new_target.as_js_object_ptr();
        // SAFETY: is_object 保证指针非空且对象本 session 存活。
        let nt_obj = unsafe { &*nt_ptr };
        let proto_si = vm.kernel_core().perm_interner().intern("prototype").0;
        let proto_val = match vm.ordinary_get(nt_obj, proto_si, new_target) {
            Ok(v) => v,
            Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
        };
        if proto_val.is_object() {
            proto_val
        } else {
            default_proto
        }
    } else {
        default_proto
    };
    // 分配期上界：length 与 max 超引擎上限一律拒绝（ToIndex 阶段不校验）。
    if length > MAX_ARRAY_BUFFER_LENGTH || max_opt.is_some_and(|m| m > MAX_ARRAY_BUFFER_LENGTH) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    // 存储态上限 = 请求上限 + 1（上限 0 与定长可分；定长存 0）。
    let obj_ptr = new_array_buffer(vm, vec![0; length], max_opt.map_or(0, |m| m + 1), proto);
    // SAFETY: obj_ptr 为 alloc_object 新建对象，无别名。
    let obj = unsafe { &mut *obj_ptr };
    if let Some(max) = max_opt {
        let si = vm.kernel_core().perm_interner().intern("maxByteLength").0;
        if let Err(err) = vm.define_data_property(
            obj,
            si,
            JsValue::int(max as i32),
            oxide_types::object::PropAttributes::new(false, false, false),
        ) {
            return NativeResult::Err(crate::iterator::engine_error(vm, &err));
        }
    }
    NativeResult::Ok(JsValue::from_js_object(obj_ptr))
}

/// `ArrayBuffer.prototype.byteLength` getter：返回缓冲区字节数；
/// detached 缓冲返回 0。
pub fn array_buffer_byte_length<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let len = unsafe { &*payload_ptr }.data.as_ref().map_or(0, |d| d.len());
    NativeResult::Ok(JsValue::int(len as i32))
}

/// `ArrayBuffer.prototype.resizable` getter：返回缓冲区是否可 resize
/// （载荷存储态上限非 0：0 = 定长，非 0 = 请求上限 + 1）。
pub fn array_buffer_resizable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let resizable = unsafe { &*payload_ptr }.max_byte_length != 0;
    NativeResult::Ok(JsValue::bool(resizable))
}

/// `ArrayBuffer.prototype.maxByteLength` getter：detached → +0；定长 →
/// 当前字节数；resizable → 真实上限（存储态 − 1 解码）。
pub fn array_buffer_max_byte_length<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let payload = unsafe { &*payload_ptr };
    if payload.data.is_none() {
        return NativeResult::Ok(JsValue::int(0));
    }
    let value = if payload.max_byte_length == 0 {
        payload.data.as_ref().map_or(0, |d| d.len())
    } else {
        payload.max_byte_length - 1
    };
    NativeResult::Ok(JsValue::int(value as i32))
}

/// `ArrayBuffer.prototype.immutable` getter：返回载荷 immutable 标志。
pub fn array_buffer_immutable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    NativeResult::Ok(JsValue::bool(unsafe { &*payload_ptr }.immutable))
}

/// `ArrayBuffer.prototype.detached` getter：detached 判据即载荷 `data` 为
/// `None`。
pub fn array_buffer_detached<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    NativeResult::Ok(JsValue::bool(unsafe { &*payload_ptr }.data.is_none()))
}

/// `ArrayBuffer.prototype.markImmutable()`：定长附着缓冲区置 immutable 标志
/// 并返回接收者；已置位再调为幂等 no-op。
///
/// # 步骤
/// 1. this 品牌校验（非对象/非 ArrayBuffer → TypeError）。
/// 2. detached → TypeError。
/// 3. resizable（存储态上限非 0）→ TypeError。
/// 4. 置 immutable 标志并返回 O。
pub fn array_buffer_mark_immutable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let payload = unsafe { &mut *payload_ptr };
    if payload.data.is_none() {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    }
    if payload.max_byte_length != 0 {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer is not resizable"));
    }
    payload.immutable = true;
    NativeResult::Ok(this_val)
}

/// ArrayBuffer detach 生产路径：品牌守卫后载荷 `data → None`（字节缓冲随
/// `Option` 置空释放，载荷盒本体存活至对象 drop）。返回 undefined。
pub fn detach_array_buffer_native<H: VmHost>(vm: &mut H, this_val: JsValue) -> NativeResult {
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    unsafe { (*payload_ptr).data = None };
    NativeResult::Ok(JsValue::undefined())
}

/// `ArrayBuffer.prototype.resize(newLength)`：可 resize 缓冲区原地调整长度，
/// 前缀保留、增补零填充、缩小截断。
///
/// # 步骤
/// 1. this 品牌校验（非对象/非 ArrayBuffer → TypeError）。
/// 2. immutable 状态位先于任何 JS 调用拷出并守卫（该守卫在 newLength
///    读取之前：强转副作用不先于 immutable 观测）。
/// 3. newByteLength = ToIntegerOrInfinity(newLength) 传播式（NaN → 0，
///    ±Infinity 保留）。
/// 4. 重取载荷指针（强转调用可触发 epoch 晋升，旧指针悬垂）；detached
///    守卫在求值之后：强转内 detach 与本步前已 detach 同形 TypeError。
/// 5. 定长（重读存储态上限 = 0）→ TypeError，先于界判。
/// 6. newByteLength < 0 或 > 真实上限（重读存储态 − 1）→ RangeError。
/// 7. `Vec::resize` 一次调用收口目标长度。
/// 8. 返回 undefined。
pub fn array_buffer_resize<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_reg = if args.is_empty() { 0 } else { args[0] };
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷；
    // 状态位拷出后本次借用即结束，不跨 JS 调用点。
    let immutable = unsafe { &*payload_ptr }.immutable;
    if immutable {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer is immutable"));
    }
    let new_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let new_length = native_try!(to_integer_or_infinity(vm, new_val));
    // 强转调用可能已把本缓冲区晋升进 session 或 detach：接收者寄存器与
    // 状态位均须重取重检，不得消费求值前快照。
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: 重取的 payload_ptr 指向存活载荷盒。
    let payload = unsafe { &mut *payload_ptr };
    let max = payload.max_byte_length;
    let Some(data) = payload.data.as_mut() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    if max == 0 {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer is not resizable"));
    }
    // 存储态上限 +1 编码：真实上限 = max - 1（上限 0 的缓冲区只许 resize(0)）。
    let real_max = max - 1;
    if new_length < 0.0 || new_length > real_max as f64 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    data.resize(new_length as usize, 0);
    NativeResult::Ok(JsValue::undefined())
}

/// ResolveBounds(len, start, end) 共享核：start 为 undefined（或缺省）归 0、
/// end 为 undefined（或缺省）归 len，在场臂 ToIntegerOrInfinity 上下夹取
/// （NaN → 0、±Infinity 饱和 0/len、负值按 len 偏移）。slice 与
/// sliceToImmutable 同源共用。
fn ab_resolve_bounds<H: VmHost>(
    vm: &mut H, len: usize, start: Option<JsValue>, end: Option<JsValue>,
) -> Result<(usize, usize), JsValue> {
    let start_val = start.unwrap_or(JsValue::undefined());
    let first = if start_val.is_undefined() { 0 } else { normalize_index(vm, start_val, len)? };
    let end_val = end.unwrap_or(JsValue::undefined());
    let final_ = if end_val.is_undefined() { len } else { normalize_index(vm, end_val, len)? };
    Ok((first, final_))
}

/// SpeciesConstructor(O, %ArrayBuffer%)：完整 Get O.constructor（访问器/异常
/// 传播）；undefined 回落 %ArrayBuffer%，其余非对象抛 TypeError；完整 Get
/// C[Symbol.species]（null 归一为 undefined）；undefined 回落 %ArrayBuffer%，
/// 非构造器抛 TypeError。
fn ab_species_constructor<H: VmHost>(vm: &mut H, o_val: JsValue) -> Result<JsValue, JsValue> {
    let o_obj = unsafe { &*o_val.as_js_object_ptr() };
    let ctor_key = vm.kernel_core().perm_interner().intern("constructor").0;
    let c = match vm.ordinary_get(o_obj, ctor_key, o_val) {
        Ok(v) => v,
        Err(msg) => return Err(crate::iterator::engine_error(vm, &msg)),
    };
    let default_ctor =
        JsValue::from_js_object(vm.session().builtin_world().array_buffer_constructor.as_ptr() as *mut JsObject);
    if c.is_undefined() {
        return Ok(default_ctor);
    }
    if !c.is_object() {
        return Err(crate::error::create_type_error(vm, "ArrayBuffer constructor is not an object"));
    }
    let species_key = make_well_known_symbol_key(WELL_KNOWN_SYMBOL_SPECIES);
    let s = match vm.ordinary_get(unsafe { &*c.as_js_object_ptr() }, species_key, c) {
        Ok(v) => v,
        Err(msg) => return Err(crate::iterator::engine_error(vm, &msg)),
    };
    let s = if s.is_null() { JsValue::undefined() } else { s };
    if s.is_undefined() {
        Ok(default_ctor)
    } else if !crate::array::is_constructor_value(s) {
        Err(crate::error::create_type_error(vm, "ArrayBuffer species is not a constructor"))
    } else {
        Ok(s)
    }
}

/// `ArrayBuffer.prototype.slice(start, end)`：复制字节区间生成新 ArrayBuffer。
///
/// # 步骤
/// 1. 品牌守卫与 detached 源守卫先于参数读（抛错时参数无副作用）。
/// 2. len 快照 → ResolveBounds（start undefined/缺省 → 0、end undefined/缺省
///    → len，不调强转）→ newLen = max(final - first, 0)。
/// 3. SpeciesConstructor(O, %ArrayBuffer%)（构造器/@@species 完整 Get，异常
///    传播；undefined 臂回落 %ArrayBuffer%；非对象/非构造器 TypeError）。
/// 4. Construct(ctor, «newLen») 经 vm.construct_ctor（native 值传递 / bytecode
///    压构造帧，含 derived 构造器 super() 语义；构造器抛出值原样上抛）。
/// 5. 结果五检：AB 载荷槽 → detached → immutable → SameValue(new, O) →
///    new.byteLength < newLen，均 TypeError。
/// 6. 构造窗口后重取源载荷（窗口内源可被 detach/晋升）：detached →
///    TypeError；按活 currentLen 夹拷贝（count = min(newLen, currentLen -
///    first)，first < currentLen 守卫），余位零填充。
///
/// # 边界与前提
/// - 源 immutable 是合法输入（只读拷贝）；对 new 的 immutable 检只挡物种
///   构造器返回的 immutable 产物。
/// - 源对象经寄存器根保活；每次 JS 调用窗口后重新读接收者寄存器与载荷指针。
pub fn array_buffer_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_reg = if args.is_empty() { 0 } else { args[0] };
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷；
    // 标量拷出后借用即结束，不跨 JS 调用窗口。
    let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let len = data.len();
    let start = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    let end = if args.len() > 2 { Some(vm.reg(args[2])) } else { None };
    let (first, final_) = native_try!(ab_resolve_bounds(vm, len, start, end));
    let new_len = final_.saturating_sub(first);

    // 物种读与构造各是 JS 调用窗口：getter/构造器可 detach、resize、晋升源。
    let ctor = native_try!(ab_species_constructor(vm, this_val));
    let new_val = match vm.construct_ctor(ctor, &[JsValue::int(new_len as i32)]) {
        Ok(v) => v,
        Err(err) => return NativeResult::Err(err),
    };

    // 构造窗口后重取接收者与载荷（源对象可被晋升进 session，旧指针悬垂）。
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: 重取的 payload_ptr 指向存活载荷盒；标量拷出后借用即结束。
    let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    // 结果五检：AB 载荷槽 → detached → immutable → SameValue → 长度。
    let new_ptr = new_val.as_js_object_ptr();
    if new_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "ArrayBuffer slice result is not an ArrayBuffer",
        ));
    }
    // SAFETY: new_val 为对象且指针非空，对象本 session 存活。
    let Some(new_payload) = array_buffer_payload_ptr(unsafe { &*new_ptr }) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "ArrayBuffer slice result is not an ArrayBuffer",
        ));
    };
    if new_payload.is_null() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "ArrayBuffer slice result is not an ArrayBuffer",
        ));
    }
    // SAFETY: new_payload 经 array_buffer_payload_ptr 校验为 ArrayBuffer 载荷盒。
    let new_state = unsafe { &*new_payload };
    if new_state.data.is_none() {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer slice result is detached"));
    }
    if new_state.immutable {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer slice result is immutable"));
    }
    if new_val == this_val {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "ArrayBuffer slice result must not be the source buffer",
        ));
    }
    if new_state.data.as_ref().map_or(0, |d| d.len()) < new_len {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer slice result is too small"));
    }
    // 活长度夹拷贝写入结果缓冲前区（构造器已零填充）：源中途收缩只拷现存
    // 字节，first 越界零拷。
    let current_len = data.len();
    let count = if first < current_len { new_len.min(current_len - first) } else { 0 };
    // SAFETY: new_payload 经 array_buffer_payload_ptr 校验；detached 臂已先行
    // 排除，data 必为 Some，可变借用止于本语句。
    if let Some(dest) = unsafe { (*new_payload).data.as_mut() } {
        dest[..count].copy_from_slice(&data[first..first + count]);
    }
    NativeResult::Ok(new_val)
}

/// `ArrayBuffer.prototype.sliceToImmutable(start, end)`：复制字节区间生成
/// immutable 新 ArrayBuffer，源缓冲不 detach、不改任何状态位。
///
/// # 步骤
/// 1. 品牌守卫与 detached 源守卫先于参数读（抛错时参数无副作用）。
/// 2. len 快照 → ResolveBounds（与 slice 共享助手）→ newLen。
/// 3. 强转窗口后重取源载荷重检：detached → TypeError（先于长度终检）；
///    currentLen < final → RangeError（源中途收缩到解析界之下）。
/// 4. GetPrototypeFromConstructor(%ArrayBuffer%)：完整读构造器 "prototype"
///    （访问器抛错传播，非对象回落 %ArrayBuffer.prototype%）。
/// 5. proto 读窗口后再次重取源载荷（窗口内源可 detach/收缩/晋升），
///    重检 detached 与在界后拷贝前 newLen 字节（恒在界内），零填充分配，
///    后置 immutable 标志；源不动。
///
/// # 边界与前提
/// - 源 immutable 是合法输入（只读拷贝）；产物恒 immutable、恒定长。
/// - 无物种面：构造器恒 %ArrayBuffer%。
pub fn array_buffer_slice_to_immutable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_reg = if args.is_empty() { 0 } else { args[0] };
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷；
    // 标量拷出后借用即结束，不跨 JS 调用窗口。
    let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let len = data.len();
    let start = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    let end = if args.len() > 2 { Some(vm.reg(args[2])) } else { None };
    let (first, final_) = native_try!(ab_resolve_bounds(vm, len, start, end));
    let new_len = final_.saturating_sub(first);

    // 强转窗口可 detach/晋升源：重取寄存器与载荷后重检（先于长度终检）。
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: 重取的 payload_ptr 指向存活载荷盒；标量拷出后借用即结束。
    let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    if data.len() < final_ {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    // GetPrototypeFromConstructor(%ArrayBuffer%)：prototype 读为 JS 窗口
    // （访问器可被用户重定义），抛错传播；非对象回落默认原型。
    let ctor_val =
        JsValue::from_js_object(vm.session().builtin_world().array_buffer_constructor.as_ptr() as *mut JsObject);
    let ctor_obj = unsafe { &*ctor_val.as_js_object_ptr() };
    let proto_si = vm.kernel_core().perm_interner().intern("prototype").0;
    let proto_val = match vm.ordinary_get(ctor_obj, proto_si, ctor_val) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(crate::iterator::engine_error(vm, &msg)),
    };
    let proto = if proto_val.is_object() { proto_val } else { default_array_buffer_proto(vm) };
    // proto 读窗口可 detach/收缩/晋升源：再重取载荷重检 detached 与在界，
    // 拷贝恒在界内（currentLen ≥ final ≥ first + newLen）。
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: 重取的 payload_ptr 指向存活载荷盒；字节借出止于本语句。
    let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    if data.len() < final_ {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    let out: Vec<u8> = data[first..first + new_len].to_vec();
    let dest_ptr = new_array_buffer(vm, out, 0, proto);
    // SAFETY: dest_ptr 为 alloc_object 新建对象，载荷盒由 new_array_buffer 建。
    let Some(dest_payload) = array_buffer_payload_ptr(unsafe { &*dest_ptr }) else {
        return type_error(vm, "ArrayBuffer internal state invalid");
    };
    // SAFETY: dest_payload 指向存活载荷盒；immutable 后置位与 transfer-Immutable
    // 同形，不扩 new_array_buffer 签名。
    unsafe { (*dest_payload).immutable = true };
    NativeResult::Ok(JsValue::from_js_object(dest_ptr))
}

/// transfer 族新缓冲保持性：Preserve 源存储态上限原样拷贝，Fixed 定长，
/// Immutable 定长并置 immutable 标志。
#[derive(Clone, Copy)]
enum TransferKeep {
    Preserve,
    Fixed,
    Immutable,
}

/// `ArrayBufferCopyAndDetach(O, newLength, keep)` 共享核：品牌守卫 →
/// newLength 求值 → detached/immutable 判定 → 界校验 → 前缀拷贝零填充建新
/// 缓冲 → 源 detach。
///
/// # 步骤
/// 1. this 品牌校验（非对象/非 ArrayBuffer → TypeError）。
/// 2. newLength 缺省 → 源当前字节长；在场 → ToIndex 传播式（强转副作用
///    先于一切守卫观测）。
/// 3. 重取源载荷指针（求值可已晋升/detach/改 immutable），重检状态位：
///    detached → TypeError；immutable → TypeError。
/// 4. 界校验：仅保持性原样臂——resizable 源且 newByteLength > 真实上限
///    （重读存储态 − 1）→ RangeError；newByteLength > 引擎分配上界 →
///    RangeError（全保持性）。
/// 5. 物化拷贝（载荷指针新鲜，字节借出止于本步）。
/// 6. 新建缓冲（proto 取 %ArrayBuffer.prototype%，保持性按 keep），字节
///    序列零填充至 newByteLength。
/// 7. 再重取源载荷指针 detach（`data → None`）：分配可已晋升源对象，新缓冲
///    持独立拷贝，源 detach 不影响返回值。
/// 8. 返回新缓冲。
fn array_buffer_copy_and_detach<H: VmHost>(
    vm: &mut H, this_reg: u8, new_length: Option<JsValue>, keep: TransferKeep,
) -> NativeResult {
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷；
    // 只读当前字节长，借用即结束，不跨 JS 调用点。
    let cur_len = unsafe { &*payload_ptr }.data.as_ref().map_or(0, |d| d.len());
    let new_len = match new_length {
        None => cur_len,
        Some(v) => native_try!(to_index(vm, v)),
    };
    // 求值可已把本缓冲区晋升进 session、detach 或置 immutable：接收者寄存器
    // 与状态位均须重取重检，不得消费求值前快照。
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: 重取的 payload_ptr 指向存活载荷盒；标量拷出后借用即结束。
    let (cur_len, stored_max, immutable, detached) = unsafe {
        let p = &*payload_ptr;
        (p.data.as_ref().map_or(0, |d| d.len()), p.max_byte_length, p.immutable, p.data.is_none())
    };
    if detached {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    }
    if immutable {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer is immutable"));
    }
    // 保持性原样臂：resizable 源的真实上限（存储态 − 1）须容纳新长度
    // （ttf/tti 定长分配无上限约束）。
    if matches!(keep, TransferKeep::Preserve) && stored_max != 0 && new_len > stored_max - 1 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    if new_len > MAX_ARRAY_BUFFER_LENGTH {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    // SAFETY: 重取的 payload_ptr 指向存活载荷盒；detached 已排除，data 必为
    // Some。字节借出止于本语句。
    let copy_len = new_len.min(cur_len);
    let mut new_data: Vec<u8> = unsafe { &*payload_ptr }
        .data
        .as_deref()
        .map_or_else(Vec::new, |d| d[..copy_len].to_vec());
    new_data.resize(new_len, 0);
    let (dest_max, dest_immutable) = match keep {
        TransferKeep::Preserve => (stored_max, false),
        TransferKeep::Fixed => (0, false),
        TransferKeep::Immutable => (0, true),
    };
    let proto = default_array_buffer_proto(vm);
    let dest_ptr = new_array_buffer(vm, new_data, dest_max, proto);
    // SAFETY: dest_ptr 为 alloc_object 新建对象，载荷盒由 new_array_buffer 建。
    if dest_immutable {
        let Some(p) = array_buffer_payload_ptr(unsafe { &*dest_ptr }) else {
            return type_error(vm, "ArrayBuffer internal state invalid");
        };
        unsafe { (*p).immutable = true };
    }
    // 分配可已将源晋升：detach 前再重取指针，免悬垂窗。
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: 重取的 payload_ptr 指向存活载荷盒。
    unsafe { (*payload_ptr).data = None };
    NativeResult::Ok(JsValue::from_js_object(dest_ptr))
}

/// `ArrayBuffer.prototype.transfer([newLength])`：拷贝并 detach，保持性
/// 原样（定长→定长，resizable 源→resizable 同上限）。
pub fn array_buffer_transfer<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_reg = if args.is_empty() { 0 } else { args[0] };
    let new_length = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    array_buffer_copy_and_detach(vm, this_reg, new_length, TransferKeep::Preserve)
}

/// `ArrayBuffer.prototype.transferToFixedLength([newLength])`：拷贝并
/// detach，新缓冲恒定长。
pub fn array_buffer_transfer_to_fixed_length<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_reg = if args.is_empty() { 0 } else { args[0] };
    let new_length = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    array_buffer_copy_and_detach(vm, this_reg, new_length, TransferKeep::Fixed)
}

/// `ArrayBuffer.prototype.transferToImmutable([newLength])`：拷贝并
/// detach，新缓冲恒定长且 immutable。
pub fn array_buffer_transfer_to_immutable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_reg = if args.is_empty() { 0 } else { args[0] };
    let new_length = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    array_buffer_copy_and_detach(vm, this_reg, new_length, TransferKeep::Immutable)
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
    if array_buffer_payload(vm, this_val).is_err() {
        return type_error(vm, "ArrayBuffer.prototype.toString called on incompatible receiver");
    }
    NativeResult::Ok(vm.new_string("[object ArrayBuffer]"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_vm::vm::Vm;

    /// 手工构造带载荷盒的 ArrayBuffer 对象（不经 Vm，只验 GC 三自由函数）。
    fn ab_object_with_payload(payload: ArrayBufferPayload) -> JsObject {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
        let payload_ptr = Box::into_raw(Box::new(payload));
        // SAFETY: 载荷盒形态与 new_array_buffer 的 native_fn 槽存储一致，测试结束前恰好释放一次。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        obj
    }

    /// 克隆目标对象与真实 GC 克隆体同形：AB 类型标签 + 空 native_fn 槽。
    fn ab_clone_target() -> JsObject {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
        obj
    }

    #[test]
    fn payload_clone_propagates_detached() {
        let old = ab_object_with_payload(ArrayBufferPayload {
            data: None,
            max_byte_length: 0,
            immutable: false,
        });
        let mut new_obj = ab_clone_target();
        clone_array_buffer_native(&old, &mut new_obj);
        let ptr = array_buffer_payload_ptr(&new_obj).expect("克隆体应携带载荷盒");
        // SAFETY: ptr 由 clone_array_buffer_native 新建的存活盒。
        let cloned = unsafe { &*ptr };
        assert!(cloned.data.is_none());
        unsafe { drop(Box::from_raw(ptr)) };
    }

    #[test]
    fn payload_clone_copies_flags() {
        let old = ab_object_with_payload(ArrayBufferPayload {
            data: Some(vec![1, 2, 3]),
            max_byte_length: 42,
            immutable: true,
        });
        let mut new_obj = ab_clone_target();
        clone_array_buffer_native(&old, &mut new_obj);
        let ptr = array_buffer_payload_ptr(&new_obj).expect("克隆体应携带载荷盒");
        // SAFETY: ptr 由 clone_array_buffer_native 新建的存活盒。
        let cloned = unsafe { &*ptr };
        assert_eq!(cloned.max_byte_length, 42);
        assert!(cloned.immutable);
        assert_eq!(cloned.data.as_ref().expect("附着缓冲应随克隆").as_slice(), &[1, 2, 3]);
        unsafe { drop(Box::from_raw(ptr)) };
    }

    #[test]
    fn detached_size_and_drop_idempotent() {
        let mut obj = ab_object_with_payload(ArrayBufferPayload {
            data: None,
            max_byte_length: 0,
            immutable: false,
        });
        let struct_bytes = std::mem::size_of::<ArrayBufferPayload>() as u64;
        assert_eq!(array_buffer_native_size(&obj), struct_bytes);
        // 首次 drop 释放结构体（无缓冲容量）；槽置空后二次 drop 零释放。
        assert_eq!(drop_array_buffer_native(&mut obj), struct_bytes);
        assert_eq!(drop_array_buffer_native(&mut obj), 0);
    }

    #[test]
    fn attached_size_and_drop() {
        let mut obj = ab_object_with_payload(ArrayBufferPayload {
            data: Some(vec![0u8; 8]),
            max_byte_length: 0,
            immutable: false,
        });
        let bytes = (std::mem::size_of::<ArrayBufferPayload>() + 8) as u64;
        assert_eq!(array_buffer_native_size(&obj), bytes);
        assert_eq!(drop_array_buffer_native(&mut obj), bytes);
        assert_eq!(drop_array_buffer_native(&mut obj), 0);
    }

    #[test]
    fn payload_ptr_brand_guard() {
        // 非 ArrayBuffer 类型标签（PLAIN）对象取载荷指针恒 None。
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        let payload_ptr = Box::into_raw(Box::new(ArrayBufferPayload {
            data: Some(vec![0u8; 4]),
            max_byte_length: 0,
            immutable: false,
        }));
        // SAFETY: 同 ab_object_with_payload 的载荷盒形态。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        assert!(array_buffer_payload_ptr(&obj).is_none());
        // SAFETY: 测试构造的盒，恰好释放一次。
        unsafe { drop(Box::from_raw(payload_ptr)) };
    }

    /// JS 求值辅助：单脚本编译执行，返回完成值。
    fn eval_ab(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("parse: {:?}", e[0].message))?;
        let module = oxide_compiler::compiler::Compiler::new()
            .compile(&program)
            .map_err(|e| format!("compile: {e}"))?;
        vm.run(&std::sync::Arc::new(module))
    }

    /// markImmutable 标志面钉：置位返回 O、二调幂等、resizable/detached 两
    /// TypeError 形。
    #[test]
    fn mark_immutable_flag_and_guard() {
        let mut vm = Vm::new();
        let r = eval_ab(
            &mut vm,
            "var ab = new ArrayBuffer(4); \
             var ret = ab.markImmutable(); \
             ret === ab && ab.immutable === true && ab.markImmutable() === ab \
             && ab.immutable === true && ab.byteLength === 4",
        )
        .unwrap();
        assert!(r.as_bool());
        let e = eval_ab(&mut vm, "new ArrayBuffer(4, {maxByteLength: 8}).markImmutable()").unwrap_err();
        assert!(e.contains("TypeError"), "resizable 应抛 TypeError: {e}");
        let e = eval_ab(
            &mut vm,
            "var ab = new ArrayBuffer(4); $262.detachArrayBuffer(ab); \
             try { ab.markImmutable(); 'no-throw' } catch (err) { err.name }",
        )
        .unwrap();
        assert_eq!(vm.lookup_str(e).unwrap(), "TypeError");
    }

    /// transfer 族新缓冲存储态字段表：Preserve 源上限原样（+1 编码不变）、
    /// Fixed/Immutable 定长，仅 Immutable 置位；源均 detach。
    #[test]
    fn transfer_dest_payload_field_table() {
        let mut vm = Vm::new();
        let r = eval_ab(
            &mut vm,
            "var src = new ArrayBuffer(4, {maxByteLength: 8}); \
             var v = new Uint8Array(src); v[0] = 9; \
             var dest = src.transfer(5); \
             var a = new Uint8Array(dest)[0]; \
             var src1 = new ArrayBuffer(4, {maxByteLength: 8}); \
             var d1 = src1.transferToFixedLength(5).maxByteLength; \
             var src2 = new ArrayBuffer(4, {maxByteLength: 8}); \
             var d2 = src2.transferToImmutable(5); \
             var fixed = new ArrayBuffer(3); \
             var d3 = fixed.transfer(3).resizable; \
             [a, src.detached, d1, d2.immutable, d2.resizable, fixed.detached, d3].join(',')",
        )
        .unwrap();
        assert_eq!(vm.lookup_str(r).unwrap(), "9,true,5,true,false,true,false");
        // 载荷存储态直读：transfer 产物 max = 8 + 1（+1 编码），ttf/tti 产物 = 0。
        let dest_val = eval_ab(
            &mut vm,
            "var s = new ArrayBuffer(4, {maxByteLength: 8}); \
             globalThis.__d = s.transfer(5); globalThis.__d",
        )
        .unwrap();
        // SAFETY: __d 为已晋升 session 的 ArrayBuffer 对象，借用止于断言、不跨下一次 eval。
        let dest_obj = unsafe { &*dest_val.as_js_object_ptr() };
        let p = array_buffer_payload_ptr(dest_obj).expect("产物应携带载荷盒");
        // SAFETY: p 指向存活载荷盒。
        let payload = unsafe { &*p };
        assert_eq!(payload.max_byte_length, 9);
        assert!(!payload.immutable);
        assert_eq!(payload.data.as_ref().expect("产物应附着").len(), 5);
    }

    /// resize detach 守卫序钉：守卫在 newLength 求值之后——强转内 detach
    /// 与求值前已 detach 两形均先跑完 valueOf 再按 TypeError 抛；
    /// immutable 守卫在求值之前（强转不跑）。
    #[test]
    fn resize_guard_order_pins() {
        let mut vm = Vm::new();
        let r = eval_ab(
            &mut vm,
            "(function () { var calls = 0; var ab = new ArrayBuffer(64, {maxByteLength: 1024}); \
             try { ab.resize({ valueOf() { calls++; $262.detachArrayBuffer(ab); return 0; } }); \
                   return 'no-throw' + calls; } \
             catch (err) { return (err.name === 'TypeError' ? 'T' : 'X') + calls; } })() \
             + '|' + \
             (function () { var c2 = 0; var ab2 = new ArrayBuffer(64, {maxByteLength: 1024}); \
              $262.detachArrayBuffer(ab2); \
              try { ab2.resize({ valueOf() { c2++; return 0; } }); return 'no-throw' + c2; } \
              catch (err) { return (err.name === 'TypeError' ? 'T' : 'X') + c2; } })()",
        )
        .unwrap();
        assert_eq!(vm.lookup_str(r).unwrap(), "T1|T1");
        let r = eval_ab(
            &mut vm,
            "var calls = 0; \
         var ab = new ArrayBuffer(4).transferToImmutable(); \
         try { ab.resize({ valueOf() { calls++; return 0; } }); 'no-throw' } \
         catch (err) { (err.name === 'TypeError' ? 'T' : 'X') + calls }",
        )
        .unwrap();
        assert_eq!(vm.lookup_str(r).unwrap(), "T0");
    }

    /// slice end 缺省臂钉：end 为 undefined 归 len 不经强转（6, undefined) → 2；
    /// start/end 全缺省 → 全长。
    #[test]
    fn slice_end_undefined_defaults_len() {
        let mut vm = Vm::new();
        let r = eval_ab(
            &mut vm,
            "new ArrayBuffer(8).slice(6, undefined).byteLength === 2 \
             && new ArrayBuffer(8).slice(undefined).byteLength === 8 \
             && new ArrayBuffer(8).slice(6).byteLength === 2 \
             && new ArrayBuffer(8).slice(undefined, undefined).byteLength === 8",
        )
        .unwrap();
        assert!(r.as_bool());
    }

    /// sliceToImmutable 源不 detach 钉：产物 immutable，源返回后可写/可
    /// resize/可 detach，产物内容独立于源。
    #[test]
    fn sti_source_writable_after_return() {
        let mut vm = Vm::new();
        let r = eval_ab(
            &mut vm,
            "(function () { var ab = new ArrayBuffer(8); \
             var v = new Uint8Array(ab); for (var i = 0; i < 8; i++) v[i] = i + 1; \
             var dest = ab.sliceToImmutable(); \
             var ok = dest.immutable === true && dest.resizable === false && ab.detached === false; \
             v[0] = 86; \
             var v2 = new Uint8Array(dest); \
             return ok && v2[0] === 1 && v2[7] === 8 && ab.byteLength === 8; })()",
        )
        .unwrap();
        assert!(r.as_bool());
    }

    /// slice 构造窗口源 detach 重检钉：species 构造器内 detach 源 → 构造后
    /// 重检抛 TypeError（不交付结果）。
    #[test]
    fn slice_source_detach_in_construct_throws() {
        let mut vm = Vm::new();
        let r = eval_ab(
            &mut vm,
            "(function () { var ab = new ArrayBuffer(8); \
             var c = {}; c[Symbol.species] = function (len) { \
                 $262.detachArrayBuffer(ab); return new ArrayBuffer(len); }; \
             ab.constructor = c; \
             try { ab.slice(); return 'no-throw'; } catch (err) { return err.name; } })()",
        )
        .unwrap();
        assert_eq!(vm.lookup_str(r).unwrap(), "TypeError");
    }
}
