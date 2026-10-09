//! BroadcastChannel 载荷盒：mpsc 发送/接收对与通道名存于对象 `native_fn` 槽，
//! 向 GC 家族表暴露 size/drop 两自由函数。
//!
//! 载荷盒无对象边（mpsc 非 GC 边、通道名为 Rust `String`），家族表 `ops_for`
//! 臂三边函数全 `None`，mark 链不消费本模块。

use std::mem;

use oxide_kernel::message_queue;
use oxide_types::object::JsObject;

use crate::message_value::MessageValue;

/// BroadcastChannel 载荷盒：存于对象 `native_fn` 槽（`Box::into_raw`，realm
/// 局部单线程）。`name` 是通道名（Rust `String`，注册表键），`tx` 是本通道的
/// mpsc 发送端（`Send`），`rx` 是配对的 mpsc 接收端（`!Send`，realm 局部），
/// `closed` 是关闭标志。`tx` 与 `rx` 是 `channel()` 配对，`tx.send(v)` 写入本
/// 通道 `rx`。
///
#[expect(dead_code)] // 字段由构造器与方法消费，载荷盒当前仅建盒与释放
struct BroadcastChannelInner {
    name: String,
    tx: message_queue::Sender<MessageValue>,
    rx: message_queue::Receiver<MessageValue>,
    closed: bool,
}

/// 取载荷盒指针：仅 BroadcastChannel 标签对象返回 `Some`，其余标签返回 `None`。
fn broadcast_channel_payload_ptr(obj: &JsObject) -> Option<*mut BroadcastChannelInner> {
    if !obj.is_broadcast_channel() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut BroadcastChannelInner)
}

/// 只读核算 BroadcastChannel 载荷盒字节（不释放）。
///
/// mpsc 内部缓冲在 Arc 通道内、盒不可寻址，不计入（有界轻微少计，只推迟
/// GC 触发，不影响触发正确性）。
pub fn broadcast_channel_native_size(obj: &JsObject) -> u64 {
    let Some(ptr) = broadcast_channel_payload_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    mem::size_of::<BroadcastChannelInner>() as u64
}

/// 释放 BroadcastChannel 载荷盒（`Box<BroadcastChannelInner>`），返回释放字节数。
///
/// 盒 drop 时发送端与接收端各减一次 mpsc 引用计数，`name` String 随盒释放。
/// 槽置空保证重复调用零释放（幂等）。
pub fn drop_broadcast_channel_native(obj: &mut JsObject) -> u64 {
    let bytes = broadcast_channel_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = broadcast_channel_payload_ptr(obj) else {
        return 0;
    };
    // SAFETY: ptr 非空（broadcast_channel_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe { drop(Box::from_raw(ptr)) };
    obj.set_native_fn(None);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
    use oxide_types::object::NativeFnPtr;
    use oxide_types::value::JsValue;

    /// 构造真实通道并把双端移入载荷盒。
    fn channel_payload() -> BroadcastChannelInner {
        let (tx, rx) = message_queue::channel::<MessageValue>();
        BroadcastChannelInner {
            name: "test".to_owned(),
            tx,
            rx,
            closed: false,
        }
    }

    /// 手工构造带载荷盒的 BroadcastChannel 对象（不经 Vm，只验 GC 两自由函数）。
    fn channel_object_with_payload(inner: BroadcastChannelInner) -> JsObject {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.type_tag = JsObject::OBJ_TYPE_BROADCAST_CHANNEL;
        let payload_ptr = Box::into_raw(Box::new(inner));
        // SAFETY: 载荷盒形态与 native_fn 槽存储约定一致，测试结束前恰好释放一次。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        obj
    }

    #[test]
    fn size_and_drop_idempotent() {
        let mut obj = channel_object_with_payload(channel_payload());
        let struct_bytes = mem::size_of::<BroadcastChannelInner>() as u64;
        assert_eq!(broadcast_channel_native_size(&obj), struct_bytes);
        // 首次 drop 释放结构体；槽置空后二次 drop 零释放。
        assert_eq!(drop_broadcast_channel_native(&mut obj), struct_bytes);
        assert_eq!(drop_broadcast_channel_native(&mut obj), 0);
    }

    #[test]
    fn channel_roundtrip_through_box() {
        let mut obj = channel_object_with_payload(channel_payload());
        let ptr = broadcast_channel_payload_ptr(&obj).unwrap();
        // SAFETY: 载荷盒测试期间存活。
        unsafe {
            assert!(!(*ptr).closed);
            (*ptr).tx.send(MessageValue::Number(1.0)).unwrap();
            assert!(matches!((*ptr).rx.try_recv().unwrap(), MessageValue::Number(n) if n == 1.0));
        }
        drop_broadcast_channel_native(&mut obj);
    }

    #[test]
    fn payload_ptr_brand_guard() {
        // 非 BroadcastChannel 类型标签（PLAIN）对象取载荷指针恒 None。
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        let payload_ptr = Box::into_raw(Box::new(channel_payload()));
        // SAFETY: 同 channel_object_with_payload 的载荷盒形态。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        assert!(broadcast_channel_payload_ptr(&obj).is_none());
        // SAFETY: 测试构造的盒，恰好释放一次。
        unsafe { drop(Box::from_raw(payload_ptr)) };
    }
}
