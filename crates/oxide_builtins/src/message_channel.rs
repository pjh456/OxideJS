//! MessagePort 载荷盒：mpsc 双端与对端端口边存于对象 `native_fn` 槽，
//! 向 GC 家族表暴露 edges/size/drop 三自由函数。

use std::mem;

use oxide_kernel::message_queue;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::message_value::MessageValue;

/// MessagePort 载荷盒：存于对象 `native_fn` 槽（`Box::into_raw`，realm 局部
/// 单线程）。`peer` 是发往对端的 mpsc 发送端（无 GC 边），`rx` 是收自对端的
/// mpsc 接收端（无 GC 边），`peer_port` 是对端端口对象的裸指针（双向 GC 对象
/// 边，保对端存活），`closed` 是关闭标志。
///
/// `peer` / `rx` / `closed` 当前仅单测读取，生产读取方是端口 send/recv 方法
/// （后续任务落地）；字段被生产路径读取后须移除本标注。
#[allow(dead_code)]
struct MessagePortInner {
    peer: message_queue::Sender<MessageValue>,
    rx: message_queue::Receiver<MessageValue>,
    peer_port: *mut JsObject,
    closed: bool,
}

/// 取载荷盒指针：仅 MessagePort 标签对象返回 `Some`，其余标签返回 `None`。
fn message_port_payload_ptr(obj: &JsObject) -> Option<*mut MessagePortInner> {
    if !obj.is_message_port() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut MessagePortInner)
}

/// 收集 MessagePort 的对端端口边（GC 根边）。
///
/// `peer_port` 空窗口（构造期回填之前）返回空边集；边函数自守，语义自含。
pub fn message_port_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(ptr) = message_port_payload_ptr(obj) else {
        return Vec::new();
    };
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: ptr 指向有效载荷盒（槽位仅构造期写入、释放时置空），
    // 只读单个裸指针字段，不搬移载荷。
    let peer_port = unsafe { (*ptr).peer_port };
    if peer_port.is_null() {
        return Vec::new();
    }
    vec![JsValue::from_js_object(peer_port)]
}

/// 只读核算 MessagePort 载荷盒字节（不释放）。
///
/// mpsc 内部缓冲在 Arc 通道内、盒不可寻址，不计入（有界轻微少计，
/// 只推迟 GC 触发，不影响触发正确性）。
pub fn message_port_native_size(obj: &JsObject) -> u64 {
    let Some(ptr) = message_port_payload_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    mem::size_of::<MessagePortInner>() as u64
}

/// 释放 MessagePort 载荷盒（`Box<MessagePortInner>`），返回释放字节数。
///
/// 盒 drop 时发送端与接收端各减一次 mpsc 引用计数；`peer_port` 裸指针
/// 无 drop。槽置空保证重复调用零释放（幂等）。
pub fn drop_message_port_native(obj: &mut JsObject) -> u64 {
    let bytes = message_port_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = message_port_payload_ptr(obj) else {
        return 0;
    };
    // SAFETY: ptr 非空（message_port_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe { drop(Box::from_raw(ptr)) };
    obj.set_native_fn(None);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
    use oxide_types::object::NativeFnPtr;

    /// 构造真实通道并把双端移入载荷盒。
    fn port_payload() -> MessagePortInner {
        let (peer, rx) = message_queue::channel::<MessageValue>();
        MessagePortInner {
            peer,
            rx,
            peer_port: std::ptr::null_mut(),
            closed: false,
        }
    }

    /// 手工构造带载荷盒的 MessagePort 对象（不经 Vm，只验 GC 三自由函数）。
    fn port_object_with_payload(inner: MessagePortInner) -> JsObject {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.type_tag = JsObject::OBJ_TYPE_MESSAGE_PORT;
        let payload_ptr = Box::into_raw(Box::new(inner));
        // SAFETY: 载荷盒形态与 native_fn 槽存储约定一致，测试结束前恰好释放一次。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        obj
    }

    #[test]
    fn size_and_drop_idempotent() {
        let mut obj = port_object_with_payload(port_payload());
        let struct_bytes = mem::size_of::<MessagePortInner>() as u64;
        assert_eq!(message_port_native_size(&obj), struct_bytes);
        // 首次 drop 释放结构体；槽置空后二次 drop 零释放。
        assert_eq!(drop_message_port_native(&mut obj), struct_bytes);
        assert_eq!(drop_message_port_native(&mut obj), 0);
    }

    #[test]
    fn edges_peer_port_null_and_present() {
        let mut obj = port_object_with_payload(port_payload());
        // peer_port 空窗口（构造期回填之前）：边函数返回零条边。
        assert!(message_port_native_edges(&obj).is_empty());

        // 回填对端端口边：一条对象边。
        let peer = Box::leak(Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined())));
        peer.type_tag = JsObject::OBJ_TYPE_MESSAGE_PORT;
        let ptr = message_port_payload_ptr(&obj).unwrap();
        // SAFETY: peer 是测试局部对象，测试期间存活，边只读不搬移。
        unsafe { (*ptr).peer_port = peer as *mut JsObject };
        let edges = message_port_native_edges(&obj);
        assert_eq!(edges.len(), 1);
        assert!(edges[0].is_object());
        // 对端对象是测试局部泄漏，不随端口 drop 释放（裸指针无 drop）。
        drop_message_port_native(&mut obj);
    }

    #[test]
    fn channel_roundtrip_through_box() {
        let mut obj = port_object_with_payload(port_payload());
        let ptr = message_port_payload_ptr(&obj).unwrap();
        // SAFETY: 载荷盒测试期间存活。
        unsafe {
            assert!(!(*ptr).closed);
            (*ptr).peer.send(MessageValue::Number(1.0)).unwrap();
            assert!(matches!((*ptr).rx.try_recv().unwrap(), MessageValue::Number(n) if n == 1.0));
        }
        drop_message_port_native(&mut obj);
    }

    #[test]
    fn payload_ptr_brand_guard() {
        // 非 MessagePort 类型标签（PLAIN）对象取载荷指针恒 None。
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        let payload_ptr = Box::into_raw(Box::new(port_payload()));
        // SAFETY: 同 port_object_with_payload 的载荷盒形态。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        assert!(message_port_payload_ptr(&obj).is_none());
        // SAFETY: 测试构造的盒，恰好释放一次。
        unsafe { drop(Box::from_raw(payload_ptr)) };
    }
}
