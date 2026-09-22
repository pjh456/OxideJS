use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

pub(crate) const MAX_ARRAY_BUFFER_LENGTH: usize = 1 << 30;

/// ArrayBuffer 载荷：字节缓冲与状态位。`data` 为 `None` 即缓冲区已 detach；
/// `max_byte_length` 为 0 即定长缓冲；`immutable` 为字节缓冲写守卫标志。
/// 载荷经 `Box::into_raw` 存于对象 `native_fn` 槽，GC 三自由函数
/// （clone/size/drop）对整结构体操作，克隆须连标志位一并拷贝。
#[derive(Clone)]
pub(crate) struct ArrayBufferPayload {
    pub(crate) data: Option<Vec<u8>>,
    // 状态位：resizable 上限与 immutable 写守卫，当前读路径尚未消费。
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "状态位随构造器写入与克隆拷贝，消费方在 options/resize/detach 语义路径"
        )
    )]
    pub(crate) max_byte_length: usize,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "状态位随构造器写入与克隆拷贝，消费方在 options/resize/detach 语义路径"
        )
    )]
    pub(crate) immutable: bool,
}

impl ArrayBufferPayload {
    /// 构造定长已附着载荷（构造器/切片路径的默认形态）。
    fn attached(data: Vec<u8>) -> Self {
        ArrayBufferPayload {
            data: Some(data),
            max_byte_length: 0,
            immutable: false,
        }
    }
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
    // ToIntegerOrInfinity：±Infinity 不截断，负无穷归 0、正无穷归 len，
    // 避免饱和成 isize::MIN 后取负溢出。
    if n.is_infinite() {
        return if n < 0.0 { 0 } else { len };
    }
    let int = n.trunc() as isize;
    if int < 0 {
        len.saturating_sub(int.unsigned_abs())
    } else {
        (int as usize).min(len)
    }
}

pub(crate) fn new_array_buffer<H: VmHost>(vm: &mut H, data: Vec<u8>) -> *mut JsObject {
    let proto = vm.session().builtin_world().array_buffer_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
    obj.type_tag = JsObject::OBJ_TYPE_ARRAY_BUFFER;
    // byteLength 经原型访问器读（构造器不写 own 数据属性，实例零命名属性）。
    let payload_ptr = Box::into_raw(Box::new(ArrayBufferPayload::attached(data)));
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
    let payload_ptr = native_try!(array_buffer_payload(vm, this_val));
    // SAFETY: payload_ptr 经 array_buffer_payload 校验为合法 ArrayBuffer 载荷。
    let Some(data) = unsafe { &*payload_ptr }.data.as_ref() else {
        return NativeResult::Err(crate::error::create_type_error(vm, "ArrayBuffer internal state invalid"));
    };
    NativeResult::Ok(JsValue::int(data.len() as i32))
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
        let mut obj = ab_object_with_payload(ArrayBufferPayload::attached(vec![0u8; 8]));
        let bytes = (std::mem::size_of::<ArrayBufferPayload>() + 8) as u64;
        assert_eq!(array_buffer_native_size(&obj), bytes);
        assert_eq!(drop_array_buffer_native(&mut obj), bytes);
        assert_eq!(drop_array_buffer_native(&mut obj), 0);
    }

    #[test]
    fn payload_ptr_brand_guard() {
        // 非 ArrayBuffer 类型标签（PLAIN）对象取载荷指针恒 None。
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        let payload_ptr = Box::into_raw(Box::new(ArrayBufferPayload::attached(vec![0u8; 4])));
        // SAFETY: 同 ab_object_with_payload 的载荷盒形态。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        assert!(array_buffer_payload_ptr(&obj).is_none());
        // SAFETY: 测试构造的盒，恰好释放一次。
        unsafe { drop(Box::from_raw(payload_ptr)) };
    }
}
