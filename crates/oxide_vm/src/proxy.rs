//! Proxy 状态盒的 session GC 三类接线：值边、字节核算与释放。
//!
//! 状态盒本体（`ProxyState`）定义在 `oxide_builtins::proxy`（构造器侧分配），
//! 本模块只读其布局做 GC 接线；撤销函数（独立 tag）的边函数在
//! `oxide_builtins::proxy::proxy_revoker_native_edges`。
//!
//! trap 派发框架：`call_trap` 从 handler 读 trap 值并同步调用，各入口
//! （get / set / has / defineProperty）的 is_proxy 守卫路由到本助手；逐 trap
//! 完整语义（不变量检查、receiver 语义、target 自身属性描述符）由后续卡补全。

use oxide_builtins::proxy::ProxyState;
use oxide_runtime_api::VmHost;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use crate::vm::Vm;

/// Proxy 状态盒内全部值边（GC mark 边）：target 与 handler。
pub(crate) fn proxy_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = oxide_builtins::proxy::proxy_state_ref(obj) else {
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

/// trap 调用助手：从 handler 读 trap 值并同步调用。
///
/// # 步骤
/// 1. 取状态盒并检 revoked 位：位真抛 TypeError（"Proxy is revoked"）。
/// 2. 经全 get 路径从 handler 读 trap 值（receiver 为 handler——handler 自身
///    是代理时其 get trap 依规范触发）。
/// 3. trap 值 undefined 返 None（落穿普通行为）。
/// 4. 经 `call_function_sync` 同步调用 trap 值（this 为 handler）。
///
/// # 边界与前提
/// - `proxy` 须为带状态盒的 Proxy 对象（守卫以 `is_proxy_obj` 保证入口）。
/// - 代理对象须为 GC 根（活栈上），handler 与 target 经家族表边标活，
///   trap 调用期间状态盒不被回收（重入 GC 安全点）。
///
/// # 副作用
/// - trap 调用执行用户代码（含分配与 GC）；trap 返回值原样透传。
///
/// # 注意事项
/// - 逐 trap 返回值校验（布尔投影、不变量）由各卡守卫补全，本助手只给通用形态。
pub(crate) fn call_trap(
    vm: &mut Vm, proxy: &JsObject, trap_name_si: u32, args: &[JsValue],
) -> Result<Option<JsValue>, String> {
    let state = oxide_builtins::proxy::proxy_state_ref(proxy).expect("proxy state box must exist");
    if state.revoked {
        return Err(vm.error_message_text("TypeError", "Proxy is revoked"));
    }
    let handler = state.handler;
    // SAFETY: handler 由构造器校验为非空对象指针。
    let handler_obj = unsafe { &*handler.as_js_object_ptr() };
    let trap_val = vm.ordinary_get(handler_obj, trap_name_si, handler)?;
    if trap_val.is_undefined() {
        return Ok(None);
    }
    let result = vm.call_function_sync(trap_val, handler, args)?;
    Ok(Some(result))
}

/// 布尔 trap 返回值校验：trap 返回值投影为布尔，非布尔返回抛 TypeError。
///
/// 错误文本为 `TypeError: ...` 前缀形态，调用边界经 `create_from_text`
/// 恢复为 TypeError 异常对象。
pub(crate) fn trap_bool_result(result: JsValue, trap_name: &str) -> Result<bool, String> {
    if result.is_bool() {
        return Ok(result.as_bool());
    }
    Err(oxide_runtime_api::format_error_message(
        "TypeError",
        &format!("Proxy {trap_name} trap must return a boolean"),
    ))
}

/// 构造 defineProperty trap 的属性描述符对象（value 加三标志位）。
///
/// 描述符属性均为不可写、不可枚举、不可配置（与内建对象属性口径一致）；
/// 对象原型链落 Object.prototype。
pub(crate) fn define_property_descriptor(vm: &mut Vm, val: JsValue, attributes: PropAttributes) -> JsValue {
    let proto = vm.builtin_proto(oxide_runtime_api::ProtoKind::ObjectProto);
    let obj = vm.alloc_object(JsObject::new_empty(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
    ));
    let attrs = PropAttributes::new(false, false, true);
    let obj_ref = unsafe { &mut *obj };
    let _ = vm.define_data_property(obj_ref, vm.perm_intern("value"), val, attrs);
    let _ = vm.define_data_property(obj_ref, vm.perm_intern("writable"), JsValue::bool(attributes.writable()), attrs);
    let _ =
        vm.define_data_property(obj_ref, vm.perm_intern("enumerable"), JsValue::bool(attributes.enumerable()), attrs);
    let _ = vm.define_data_property(
        obj_ref,
        vm.perm_intern("configurable"),
        JsValue::bool(attributes.configurable()),
        attrs,
    );
    JsValue::from_js_object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::NativeFn;
    use crate::vm::Vm;
    use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
    use oxide_runtime_api::{NativeResult, ProtoKind};
    use oxide_types::object::NativeFnPtr;

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

    /// get trap 原生函数体：逼一次完整收集并返回固定值。
    fn gc_forcing_get_trap(vm: &mut Vm, _args: &[u8]) -> NativeResult {
        vm.collect_session_gc();
        NativeResult::Ok(JsValue::int(42))
    }

    /// 计数 trap 原生函数体：receiver（handler）的计数属性加一，返回 true。
    fn counting_trap(vm: &mut Vm, _args: &[u8]) -> NativeResult {
        let receiver = vm.reg(253);
        // SAFETY: receiver 由 trap 调用约定保证为 handler 对象值。
        let handler = unsafe { &mut *receiver.as_js_object_ptr() };
        let hits_si = vm.perm_intern("hits");
        let cur = vm.resolve_property(handler, hits_si).unwrap_or(JsValue::int(0));
        vm.set_or_create_prop_value(handler, hits_si, JsValue::int(cur.as_int() + 1));
        NativeResult::Ok(JsValue::bool(true))
    }

    /// 建带原生函数项的函数对象（session 堆分配并登记）。
    fn native_fn_obj(vm: &mut Vm, func: NativeFn) -> *mut JsObject {
        let proto = vm.builtin_proto(ProtoKind::FunctionProto);
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        obj.set_function(true);
        // SAFETY: func 是合法 NativeFn 函数项。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(func as *const ()) }));
        obj.set_native_arg_count(0);
        vm.alloc_object(obj)
    }

    /// 建带状态盒的 Proxy 对象（target、handler、代理均登记 session 堆）。
    fn proxy_with_state(vm: &mut Vm, target: JsValue, handler: JsValue) -> *mut JsObject {
        let proto = vm.builtin_proto(ProtoKind::ObjectProto);
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto));
        obj.type_tag = JsObject::OBJ_TYPE_PROXY;
        let state = Box::new(ProxyState {
            target,
            handler,
            revoked: false,
        });
        obj.set_native_data(Box::into_raw(state) as *mut u8);
        vm.alloc_object(obj)
    }

    /// 重入 GC 安全点：trap 调用期间逼完整收集，handler 与 target 指针身份
    /// 不变（经状态盒边标活），且收集计数推进（收集确在 trap 调用期间发生）。
    #[test]
    fn trap_call_gc_safety_point_keeps_handler_and_target_alive() {
        let mut vm = Vm::new();
        let target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let trap_fn = native_fn_obj(&mut vm, gc_forcing_get_trap);
        let get_si = vm.perm_intern("get");
        vm.set_or_create_prop_value(unsafe { &mut *handler }, get_si, JsValue::from_js_object(trap_fn));
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        // 代理置于活栈寄存器（Regs 根组，避开实参打包区），与生产 trap 调用路径同形。
        vm.regs[4] = JsValue::from_js_object(proxy);
        let before = vm.session_gc_stats().total_collections;
        let key_si = vm.perm_intern("k");
        let result = vm
            .ordinary_get(unsafe { &*proxy }, key_si, JsValue::from_js_object(proxy))
            .expect("get 应成功");
        assert_eq!(result, JsValue::int(42), "get trap 返回值须原样返回");
        assert!(vm.session_gc_stats().total_collections > before, "trap 调用期间应发生收集");
        let state = oxide_builtins::proxy::proxy_state_ref(unsafe { &*proxy }).expect("状态盒");
        assert!(
            std::ptr::eq(state.target.as_js_object_ptr(), target),
            "target 不得被 trap 调用期间的收集回收"
        );
        assert!(
            std::ptr::eq(state.handler.as_js_object_ptr(), handler),
            "handler 不得被 trap 调用期间的收集回收"
        );
    }

    /// 已撤销代理：全部 trap 派发抛 TypeError。
    #[test]
    fn call_trap_revoked_throws_type_error() {
        let mut vm = Vm::new();
        let target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        {
            let state = oxide_builtins::proxy::proxy_state_mut(unsafe { &mut *proxy }).expect("状态盒");
            state.revoked = true;
        }
        let get_si = vm.perm_intern("get");
        let err = call_trap(&mut vm, unsafe { &*proxy }, get_si, &[]).expect_err("已撤销代理须抛错");
        assert!(err.contains("TypeError"), "已撤销代理须抛 TypeError：{err}");
        assert!(err.contains("Proxy is revoked"), "错误文本须带撤销标记：{err}");
    }

    /// 守卫路由：get / set / has trap 对代理命中，resolve_property 对代理保守返 None。
    #[test]
    fn guard_routing_for_proxy_get_set_has_and_resolve() {
        let mut vm = Vm::new();
        let target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let trap_fn = native_fn_obj(&mut vm, counting_trap);
        let handler_val = JsValue::from_js_object(handler);
        for name in ["get", "set", "has"] {
            vm.set_or_create_prop_value(
                unsafe { &mut *handler },
                vm.perm_intern(name),
                JsValue::from_js_object(trap_fn),
            );
        }
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), handler_val);
        let key_si = vm.perm_intern("k");
        let hits_si = vm.perm_intern("hits");

        let result = vm.ordinary_get(unsafe { &*proxy }, key_si, handler_val).expect("get 应成功");
        assert_eq!(result, JsValue::bool(true), "get trap 返回值须原样返回");
        assert_eq!(
            vm.resolve_property(unsafe { &*handler }, hits_si).unwrap(),
            JsValue::int(1),
            "get trap 须命中"
        );

        vm.ordinary_set(unsafe { &mut *proxy }, key_si, JsValue::int(7), handler_val, true)
            .expect("set 应成功");
        assert_eq!(
            vm.resolve_property(unsafe { &*handler }, hits_si).unwrap(),
            JsValue::int(2),
            "set trap 须命中"
        );

        let exists = vm.has_property(unsafe { &*proxy }, key_si).expect("has 应成功");
        assert!(exists, "has trap 须返回 true");
        assert_eq!(
            vm.resolve_property(unsafe { &*handler }, hits_si).unwrap(),
            JsValue::int(3),
            "has trap 须命中"
        );

        assert!(
            vm.resolve_property(unsafe { &*proxy }, key_si).is_none(),
            "resolve_property 对代理须保守返 None"
        );
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
        let state = oxide_builtins::proxy::proxy_state_ref(&obj).expect("状态盒");
        assert!(!state.revoked);
        let state = oxide_builtins::proxy::proxy_state_mut(&mut obj).expect("状态盒");
        state.revoked = true;
        assert!(oxide_builtins::proxy::proxy_state_ref(&obj).expect("状态盒").revoked);
    }
}
