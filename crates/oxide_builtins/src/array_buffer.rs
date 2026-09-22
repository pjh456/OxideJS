use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
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

fn array_buffer_payload_ptr(obj: &JsObject) -> Option<*mut ArrayBufferPayload> {
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

/// `ArrayBuffer.prototype.byteLength` getter：返回缓冲区字节数。
pub fn array_buffer_byte_length<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    NativeResult::Ok(JsValue::int(data.len() as i32))
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

/// `ArrayBuffer.prototype.resize(newLength)`：可 resize 缓冲区原地调整长度，
/// 前缀保留、增补零填充、缩小截断。
///
/// # 步骤
/// 1. this 品牌校验（非对象/非 ArrayBuffer → TypeError）。
/// 2. 载荷三状态位（detached / immutable / max）先于任何 JS 调用拷入局部
///    标量。
/// 3. detached → TypeError（detach 路径由 transfer 族激活）。
/// 4. immutable → TypeError。
/// 5. newByteLength = ToIntegerOrInfinity(newLength) 传播式（NaN → 0，
///    ±Infinity 保留）。
/// 6. 定长（存储态上限 = 0）→ TypeError，先于界判。
/// 7. newByteLength < 0 或 > 真实上限（存储态 − 1）→ RangeError。
/// 8. 重取载荷指针：步 5 的 JS 调用可触发 epoch 晋升，旧指针悬垂。
/// 9. `Vec::resize` 一次调用收口目标长度。
/// 10. 返回 undefined。
pub fn array_buffer_resize<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_reg = if args.is_empty() { 0 } else { args[0] };
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷；
    // 三状态位拷出后本次借用即结束，不跨 JS 调用点。
    let (detached, immutable, max) = unsafe {
        let payload = &*payload_ptr;
        (payload.data.is_none(), payload.immutable, payload.max_byte_length)
    };
    if detached {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    }
    if immutable {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer is immutable"));
    }
    let new_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let new_length = native_try!(to_integer_or_infinity(vm, new_val));
    if max == 0 {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer is not resizable"));
    }
    // 存储态上限 +1 编码：真实上限 = max - 1（上限 0 的缓冲区只许 resize(0)）。
    let real_max = max - 1;
    if new_length < 0.0 || new_length > real_max as f64 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ArrayBuffer length"));
    }
    // 步 5 的 JS 调用可能已把本缓冲区晋升进 session：接收者寄存器经晋升
    // 重写，须重新读取后再取载荷指针（旧 this 值可能指向已释放 epoch 对象）。
    let this_val = vm.reg(this_reg);
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: 重取的 payload_ptr 指向存活载荷盒；detached 已在步 3 排除，
    // data 必为 Some。
    let payload = unsafe { &mut *payload_ptr };
    payload.data.as_mut().expect("detached 已排除").resize(new_length as usize, 0);
    NativeResult::Ok(JsValue::undefined())
}

/// `ArrayBuffer.prototype.slice(start, end)`：复制字节区间生成新 ArrayBuffer。
pub fn array_buffer_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    let len = data.len();
    let start = if args.len() > 1 {
        native_try!(normalize_index(vm, vm.reg(args[1]), len))
    } else {
        0
    };
    let end = if args.len() > 2 {
        native_try!(normalize_index(vm, vm.reg(args[2]), len))
    } else {
        len
    };
    let end = end.max(start);
    // 切片产物恒定长，原型取默认 %ArrayBuffer.prototype%。
    let proto = default_array_buffer_proto(vm);
    NativeResult::Ok(JsValue::from_js_object(new_array_buffer(vm, data[start..end].to_vec(), 0, proto)))
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
}
