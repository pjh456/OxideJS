//! Proxy 状态盒：Proxy 对象 `native_data` 槽的载荷盒（target、handler、revoked
//! 位）与 session GC 三类接线——值边、字节核算与释放。

use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// Proxy 状态盒：存于 Proxy 对象 `native_data`，target 与 handler 是 GC 边，
/// revoked 位标记已撤销状态（撤销后全部 trap 抛 TypeError）。
///
/// 生命周期与 Proxy 对象一致：构造期经 `Box::into_raw` 分配，GC drop 链释放，
/// 此后不替换。
pub(crate) struct ProxyState {
    /// 目标对象（GC 边）。
    pub(crate) target: JsValue,
    /// 处理器对象（GC 边）。
    pub(crate) handler: JsValue,
    /// 撤销位：置位后所有 trap 派发抛 TypeError。
    #[allow(dead_code)] // 读方为 trap 派发路径，随该路径接线启用
    pub(crate) revoked: bool,
}

/// 读 Proxy 状态盒（盒未建立时 None）。
pub(crate) fn proxy_state_ref(obj: &JsObject) -> Option<&ProxyState> {
    let ptr = obj.native_data() as *const ProxyState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 由 Box::into_raw 分配，生命周期与 Proxy 对象一致。
        Some(unsafe { &*ptr })
    }
}

/// 可变访问 Proxy 状态盒（写 revoked 位）。
#[allow(dead_code)] // 写方为构造器与 revocable 路径，随该路径接线启用
pub(crate) fn proxy_state_mut(obj: &mut JsObject) -> Option<&mut ProxyState> {
    let ptr = obj.native_data() as *mut ProxyState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 由 Box::into_raw 分配，生命周期与 Proxy 对象一致。
        Some(unsafe { &mut *ptr })
    }
}

/// Proxy 状态盒内全部值边（GC mark 边）：target 与 handler。
pub(crate) fn proxy_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = proxy_state_ref(obj) else {
        return Vec::new();
    };
    vec![state.target, state.handler]
}

/// 只读核算 Proxy 状态盒字节（不释放），供 GC 账目核算。
pub(crate) fn proxy_native_size(obj: &JsObject) -> u64 {
    if !obj.is_proxy_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut ProxyState;
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: 非 Proxy 与空指针已早退，ptr 非空且指向存活 Box<ProxyState>，此处只读不释放。
    std::mem::size_of::<ProxyState>() as u64
}

/// 释放 Proxy 状态盒（对象被回收时），返回释放字节数。
pub(crate) fn drop_proxy_native(obj: &JsObject) -> u64 {
    let bytes = proxy_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let ptr = obj.native_data() as *mut ProxyState;
    // SAFETY: ptr 非空（proxy_native_size 已验证），Box::from_raw 恰好释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    drop(state);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_types::shape::EMPTY_SHAPE_ID;

    /// 建一个带状态盒的 Proxy 对象（测试构造路径，不经 Vm 分配）。
    fn proxy_obj(target: JsValue, handler: JsValue) -> JsObject {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.type_tag = JsObject::OBJ_TYPE_PROXY;
        let state = Box::new(ProxyState {
            target,
            handler,
            revoked: false,
        });
        obj.set_native_data(Box::into_raw(state) as *mut u8);
        obj
    }

    /// 堆分配一个普通对象值，作 GC 边占位。
    fn plain_obj_value() -> JsValue {
        let obj = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        JsValue::from_js_object(Box::into_raw(obj))
    }

    #[test]
    fn proxy_native_edges_return_target_and_handler() {
        let target = plain_obj_value();
        let handler = plain_obj_value();
        let obj = proxy_obj(target, handler);
        assert_eq!(proxy_native_edges(&obj), vec![target, handler]);
    }

    #[test]
    fn proxy_native_size_and_drop_account_box_bytes() {
        let obj = proxy_obj(plain_obj_value(), plain_obj_value());
        let bytes = proxy_native_size(&obj);
        assert_eq!(bytes, std::mem::size_of::<ProxyState>() as u64);
        assert_eq!(drop_proxy_native(&obj), bytes);
    }

    #[test]
    fn proxy_native_size_is_zero_for_non_proxy() {
        let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        assert_eq!(proxy_native_size(&obj), 0);
        assert_eq!(drop_proxy_native(&obj), 0);
    }

    #[test]
    fn proxy_state_revoked_bit_roundtrip() {
        let mut obj = proxy_obj(plain_obj_value(), plain_obj_value());
        let state = proxy_state_ref(&obj).expect("状态盒");
        assert!(!state.revoked);
        let state = proxy_state_mut(&mut obj).expect("状态盒");
        state.revoked = true;
        assert!(proxy_state_ref(&obj).expect("状态盒").revoked);
    }
}
