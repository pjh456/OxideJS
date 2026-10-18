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
/// 3. trap 值 undefined 或 null 返 None（GetMethod 形态：落穿普通行为）。
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
    if trap_val.is_undefined() || trap_val.is_null() {
        return Ok(None);
    }
    // trap 值须可调用（规范 Call 语义）：非可调用抛 TypeError 而非落入
    // call_function_sync 的通用「accessor is not callable」文本（kind 丢失）。
    if !trap_val.is_object() {
        return Err(oxide_runtime_api::format_error_message("TypeError", "Proxy trap is not callable"));
    }
    if !unsafe { &*trap_val.as_js_object_ptr() }.is_function() {
        return Err(oxide_runtime_api::format_error_message("TypeError", "Proxy trap is not callable"));
    }
    let result = vm.call_function_sync(trap_val, handler, args)?;
    Ok(Some(result))
}

/// 布尔 trap 返回值投影（规范 ToBoolean 形态）：trap 返回值经 ToBoolean
/// 投影为布尔，任意值均可投影（0/null/undefined/空串等 falsy 值为 false）。
pub(crate) fn trap_bool_result(result: JsValue, _trap_name: &str) -> Result<bool, String> {
    Ok(oxide_runtime_api::to_boolean(result))
}

/// 把实参切片物化为 JS 数组（CreateArrayFromList 形态：Array.prototype
/// 原型链，元素区逐值写入）。
pub(crate) fn materialize_args_array(vm: &mut Vm, args: &[JsValue]) -> JsValue {
    let array_proto = vm.builtin_proto(oxide_runtime_api::ProtoKind::ArrayProto);
    let arr = vm.alloc_object(JsObject::new_array(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        args.len().min(oxide_types::object::MAX_DENSE_PROPS),
    ));
    for (i, item) in args.iter().enumerate() {
        // SAFETY: arr 是刚经统一入口分配的数组对象，元素区已就绪。
        unsafe { (*arr).set_prop_at(i, *item) };
    }
    // SAFETY: 与 new_array 同 epoch，写入后按实际元素数设 prop_count。
    unsafe { (*arr).set_prop_count(args.len().min(oxide_types::object::MAX_DENSE_PROPS)) };
    JsValue::from_js_object(arr)
}

/// apply trap 调用（[[Call]] 路由）：trap 权威，缺失落穿 target。
///
/// # 步骤
/// 1. 取状态盒（revoked 检查在 `call_trap` 内）。
/// 2. 从 handler 读 apply trap 值（GetMethod 形态：undefined 与 null 均视为缺失）。
/// 3. trap 缺失：落穿 target 的 [[Call]]（同 this 同实参的同步调用）。
/// 4. trap 存在：以 (target, thisArgument, argumentsList) 调用，返回值原样透传。
///
/// # 边界与前提
/// - `proxy` 须为带状态盒的 Proxy 对象（守卫以 `is_proxy_obj` 保证入口）。
/// - 函数代理的 target 可调用（构造期置 is_function 位的前提），落穿
///   `call_function_sync` 依赖该前提。
pub(crate) fn proxy_apply_call(
    vm: &mut Vm, proxy: &JsObject, this_value: JsValue, args: &[JsValue],
) -> Result<JsValue, String> {
    let state = oxide_builtins::proxy::proxy_state_ref(proxy).expect("状态盒");
    let target = state.target;
    let args_array = materialize_args_array(vm, args);
    match call_trap(vm, proxy, vm.perm_intern("apply"), &[target, this_value, args_array])? {
        Some(result) => Ok(result),
        None => vm.call_function_sync(target, this_value, args),
    }
}

/// construct trap 调用（[[Construct]] 路由）：trap 权威，缺失落穿 target。
///
/// # 步骤
/// 1. 取状态盒（revoked 检查在 `call_trap` 内）。
/// 2. 从 handler 读 construct trap 值（GetMethod 形态）。
/// 3. trap 缺失：返 None，调用方落穿 target 的普通 [[Construct]]。
/// 4. trap 存在：以 (target, argumentsList, newTarget) 调用；返回值须为对象，
///    否则抛 TypeError。
///
/// # 边界与前提
/// - `proxy` 须为带状态盒的 Proxy 对象（守卫以 `is_proxy_obj` 保证入口）。
/// - `new_target` 为调用点 newTarget（代理自身），落穿时原样透传给 target。
pub(crate) fn proxy_construct_trap(
    vm: &mut Vm, proxy: &JsObject, args: &[JsValue], new_target: JsValue,
) -> Result<Option<JsValue>, String> {
    let state = oxide_builtins::proxy::proxy_state_ref(proxy).expect("状态盒");
    let target = state.target;
    let args_array = materialize_args_array(vm, args);
    let result = call_trap(vm, proxy, vm.perm_intern("construct"), &[target, args_array, new_target])?;
    match result {
        Some(v) if v.is_object() => Ok(Some(v)),
        Some(_) => Err(oxide_runtime_api::format_error_message(
            "TypeError",
            "a [[Construct]] trap returned a non-object",
        )),
        None => Ok(None),
    }
}

/// 构造 `[[Set]]` 落 receiver 的 defineProperty trap 的部分描述符对象
/// （`{ [[Value]]: V }` 形态）：仅携 value 字段，writable/enumerable/
/// configurable 由 trap 侧（或其委托的 target 定义）按 targetDesc 补全。
/// 描述符属性均为可写、可枚举、可配置；对象原型链落 Object.prototype。
pub(crate) fn set_partial_trap_desc(vm: &mut Vm, val: JsValue) -> JsValue {
    let proto = vm.builtin_proto(oxide_runtime_api::ProtoKind::ObjectProto);
    let obj = vm.alloc_object(JsObject::new_empty(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
    ));
    let attrs = PropAttributes::DEFAULT_DATA;
    let obj_ref = unsafe { &mut *obj };
    let _ = vm.define_data_property(obj_ref, vm.perm_intern("value"), val, attrs);
    JsValue::from_js_object(obj)
}

/// 构造 defineProperty trap 的属性描述符对象（FromPropertyDescriptor 形态）：
/// 数据形态为 value 加 writable/enumerable/configurable，访问器形态为 get/set 加
/// enumerable/configurable；描述符属性均为可写、可枚举、可配置（trap 侧以
/// Object.keys 枚举字段数）。对象原型链落 Object.prototype。
pub(crate) fn define_trap_desc(
    vm: &mut Vm, data: Option<(JsValue, bool)>, get: JsValue, set: JsValue, attributes: PropAttributes,
) -> JsValue {
    let proto = vm.builtin_proto(oxide_runtime_api::ProtoKind::ObjectProto);
    let obj = vm.alloc_object(JsObject::new_empty(
        oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
    ));
    let attrs = PropAttributes::DEFAULT_DATA;
    let obj_ref = unsafe { &mut *obj };
    match data {
        Some((val, writable)) => {
            let _ = vm.define_data_property(obj_ref, vm.perm_intern("value"), val, attrs);
            let _ = vm.define_data_property(obj_ref, vm.perm_intern("writable"), JsValue::bool(writable), attrs);
        }
        None => {
            let _ = vm.define_data_property(obj_ref, vm.perm_intern("get"), get, attrs);
            let _ = vm.define_data_property(obj_ref, vm.perm_intern("set"), set, attrs);
        }
    }
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
    use oxide_runtime_api::{NativeResult, ProtoKind, VmHost};
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

    /// handler 计数属性加一（receiver 由 trap 调用约定保证为 handler）。
    fn bump_hits(vm: &mut Vm) {
        let receiver = vm.reg(253);
        // SAFETY: receiver 由 trap 调用约定保证为 handler 对象值。
        let handler = unsafe { &mut *receiver.as_js_object_ptr() };
        let hits_si = vm.perm_intern("hits");
        let cur = vm.resolve_property(handler, hits_si).unwrap_or(JsValue::int(0));
        vm.set_or_create_prop_value(handler, hits_si, JsValue::int(cur.as_int() + 1));
    }

    /// getPrototypeOf trap 原生函数体：计数加一，返回一个新对象。
    fn counting_get_prototype_trap(vm: &mut Vm, _args: &[u8]) -> NativeResult {
        bump_hits(vm);
        let result = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        NativeResult::Ok(JsValue::from_js_object(result))
    }

    /// ownKeys trap 原生函数体：计数加一，返回单元素数组 ["k"]。
    fn counting_own_keys_trap(vm: &mut Vm, _args: &[u8]) -> NativeResult {
        bump_hits(vm);
        let array_proto = vm.builtin_proto(ProtoKind::ArrayProto);
        let arr = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(array_proto), 1));
        unsafe { (*arr).set_prop_at(0, vm.new_string("k")) };
        unsafe { (*arr).set_prop_count(1) };
        NativeResult::Ok(JsValue::from_js_object(arr))
    }

    /// getOwnPropertyDescriptor trap 原生函数体：计数加一，返回数据描述符
    /// 对象 {value: 7, writable/enumerable/configurable 全真}。
    fn counting_gopd_trap(vm: &mut Vm, _args: &[u8]) -> NativeResult {
        bump_hits(vm);
        let proto = vm.builtin_proto(ProtoKind::ObjectProto);
        let desc = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto)));
        let attrs = PropAttributes::DEFAULT_DATA;
        let d = unsafe { &mut *desc };
        let _ = vm.define_data_property(d, vm.perm_intern("value"), JsValue::int(7), attrs);
        let _ = vm.define_data_property(d, vm.perm_intern("writable"), JsValue::bool(true), attrs);
        let _ = vm.define_data_property(d, vm.perm_intern("enumerable"), JsValue::bool(true), attrs);
        let _ = vm.define_data_property(d, vm.perm_intern("configurable"), JsValue::bool(true), attrs);
        NativeResult::Ok(JsValue::from_js_object(desc))
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

    /// 函数代理不变量：target 可调用且不可扩展时构造器抛 TypeError。
    #[test]
    fn proxy_constructor_rejects_callable_non_extensible_target() {
        let mut vm = Vm::new();
        let target = native_fn_obj(&mut vm, gc_forcing_get_trap);
        unsafe { (*target).set_extensible(false) };
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        // native 调用协议：253/254 为 receiver/callee 槽，实参经寄存器下标传递。
        vm.regs[1] = JsValue::from_js_object(target);
        vm.regs[2] = JsValue::from_js_object(handler);
        vm.constructing_native = true;
        let result = oxide_builtins::proxy::proxy_constructor(&mut vm, &[253, 254, 1, 2]);
        let exc = match result {
            NativeResult::Err(exc) => exc,
            _ => panic!("可调用且不可扩展的 target 须抛 TypeError"),
        };
        let exc_ref = unsafe { &*exc.as_js_object_ptr() };
        let name_val = vm.resolve_property(exc_ref, vm.perm_intern("name")).unwrap();
        // SAFETY: name_val 为字符串值（异常对象 name 属性）。
        let name = unsafe { (*name_val.as_string_ptr()).as_lossy_str() };
        assert_eq!(name, "TypeError", "须抛 TypeError");
    }

    /// apply trap 体：把三实参（target、thisArg、argumentsList）身份记入 handler，
    /// 返回固定值。
    fn apply_trap_capture(vm: &mut Vm, args: &[u8]) -> NativeResult {
        let handler = vm.reg(253);
        let handler_ref = unsafe { &mut *handler.as_js_object_ptr() };
        vm.set_or_create_prop_value(handler_ref, vm.perm_intern("target"), vm.reg(args[1]));
        vm.set_or_create_prop_value(handler_ref, vm.perm_intern("this"), vm.reg(args[2]));
        vm.set_or_create_prop_value(handler_ref, vm.perm_intern("list"), vm.reg(args[3]));
        NativeResult::Ok(JsValue::int(99))
    }

    /// apply trap 路由：trap 以 (target, thisArg, argumentsList) 调用，返回值原样透传。
    #[test]
    fn proxy_apply_call_routes_apply_trap_with_args() {
        let mut vm = Vm::new();
        let target = native_fn_obj(&mut vm, counting_trap);
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        let trap_fn = native_fn_obj(&mut vm, apply_trap_capture);
        let this_value = plain_obj_value();
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("apply"),
            JsValue::from_js_object(trap_fn),
        );
        let result = proxy_apply_call(&mut vm, unsafe { &*proxy }, this_value, &[JsValue::int(1), JsValue::int(2)])
            .expect("apply 应成功");
        assert_eq!(result, JsValue::int(99), "trap 返回值须原样透传");
        let handler_ref = unsafe { &*handler };
        assert_eq!(
            vm.resolve_property(handler_ref, vm.perm_intern("target")).unwrap(),
            JsValue::from_js_object(target),
            "第一实参须为 target"
        );
        assert_eq!(
            vm.resolve_property(handler_ref, vm.perm_intern("this")).unwrap(),
            this_value,
            "第二实参须为 thisArg"
        );
        let list = vm.resolve_property(handler_ref, vm.perm_intern("list")).unwrap();
        assert!(list.is_object(), "第三实参须为数组对象");
        let list_ref = unsafe { &*list.as_js_object_ptr() };
        assert!(list_ref.is_array(), "argumentsList 须为数组");
    }

    /// apply trap 缺失：落穿 target 的普通调用路径。
    #[test]
    fn proxy_apply_call_falls_through_to_target() {
        let mut vm = Vm::new();
        // target 不读 receiver（落穿时 this 为 undefined），仅返回固定值。
        fn target_return_7(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
            NativeResult::Ok(JsValue::int(7))
        }
        let target = native_fn_obj(&mut vm, target_return_7);
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        let result = proxy_apply_call(&mut vm, unsafe { &*proxy }, JsValue::undefined(), &[JsValue::int(1)])
            .expect("落穿 target 应成功");
        assert_eq!(result, JsValue::int(7), "target 调用结果须原样返回");
    }

    /// construct trap 返回值须为对象，非对象返回抛 TypeError。
    #[test]
    fn proxy_construct_trap_non_object_result_throws() {
        let mut vm = Vm::new();
        let target = native_fn_obj(&mut vm, counting_trap);
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        // construct trap 体：返回非对象值。
        fn construct_trap_non_object(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
            NativeResult::Ok(JsValue::int(42))
        }
        let trap_fn = native_fn_obj(&mut vm, construct_trap_non_object);
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("construct"),
            JsValue::from_js_object(trap_fn),
        );
        let err = proxy_construct_trap(&mut vm, unsafe { &*proxy }, &[], JsValue::from_js_object(proxy))
            .expect_err("非对象返回值须抛 TypeError");
        assert!(err.contains("TypeError"), "须抛 TypeError：{err}");
        assert!(err.contains("non-object"), "错误文本须带非对象标记：{err}");
    }

    /// construct trap 路由：trap 以 (target, argumentsList, newTarget) 调用，
    /// 对象返回值原样透传。
    #[test]
    fn proxy_construct_trap_routes_with_new_target() {
        let mut vm = Vm::new();
        let target = native_fn_obj(&mut vm, counting_trap);
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        // construct trap 体：记录三实参身份，返回固定对象。
        fn construct_trap_capture(vm: &mut Vm, args: &[u8]) -> NativeResult {
            let handler = vm.reg(253);
            let handler_ref = unsafe { &mut *handler.as_js_object_ptr() };
            vm.set_or_create_prop_value(handler_ref, vm.perm_intern("target"), vm.reg(args[1]));
            vm.set_or_create_prop_value(handler_ref, vm.perm_intern("list"), vm.reg(args[2]));
            vm.set_or_create_prop_value(handler_ref, vm.perm_intern("newtarget"), vm.reg(args[3]));
            let result = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
            NativeResult::Ok(JsValue::from_js_object(result))
        }
        let trap_fn = native_fn_obj(&mut vm, construct_trap_capture);
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("construct"),
            JsValue::from_js_object(trap_fn),
        );
        let new_target = plain_obj_value();
        let result = proxy_construct_trap(&mut vm, unsafe { &*proxy }, &[JsValue::int(3)], new_target)
            .expect("construct 应成功")
            .expect("trap 存在须有返回值");
        assert!(result.is_object(), "trap 对象返回值须原样透传");
        let handler_ref = unsafe { &*handler };
        assert_eq!(
            vm.resolve_property(handler_ref, vm.perm_intern("target")).unwrap(),
            JsValue::from_js_object(target),
            "第一实参须为 target"
        );
        let list = vm.resolve_property(handler_ref, vm.perm_intern("list")).unwrap();
        assert!(list.is_object() && unsafe { &*list.as_js_object_ptr() }.is_array(), "第二实参须为数组");
        assert_eq!(
            vm.resolve_property(handler_ref, vm.perm_intern("newtarget")).unwrap(),
            new_target,
            "第三实参须为 newTarget"
        );
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

    /// 7 个 trait 方法：非代理路径零行为变化（返回值与固有方法直读一致）。
    #[test]
    fn proto_trait_methods_plain_object_paths() {
        let mut vm = Vm::new();
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let key_si = vm.perm_intern("k");
        vm.set_or_create_prop_value(unsafe { &mut *obj }, key_si, JsValue::int(1));
        let obj_ref = unsafe { &*obj };

        // get_prototype_of：与直读一致。
        assert_eq!(
            vm.get_prototype_of(obj_ref).expect("get_prototype_of 应成功"),
            obj_ref.proto(),
            "非代理路径须与直读一致"
        );

        // set_prototype_of：正常设置成功；新旧 SameValue 直接成功。
        let new_proto = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        assert!(
            vm.set_prototype_of(unsafe { &mut *obj }, JsValue::from_js_object(new_proto))
                .expect("set_prototype_of 应成功"),
            "正常设置须成功"
        );
        assert_eq!(unsafe { &*obj }.proto(), JsValue::from_js_object(new_proto));
        assert!(
            vm.set_prototype_of(unsafe { &mut *obj }, JsValue::from_js_object(new_proto))
                .expect("set_prototype_of 应成功"),
            "新旧 SameValue 须直接成功"
        );

        // is_extensible：与直读一致。
        assert_eq!(
            vm.is_extensible(obj_ref).expect("is_extensible 应成功"),
            obj_ref.is_extensible(),
            "非代理路径须与直读一致"
        );

        // prevent_extensions：置不可扩展。
        vm.prevent_extensions(unsafe { &mut *obj }).expect("prevent_extensions 应成功");
        assert!(!unsafe { &*obj }.is_extensible(), "prevent_extensions 后须不可扩展");

        // own_keys：与 walk_own_keys 一致（仅字符串键 "k"）。
        let keys = vm.own_keys(obj_ref).expect("own_keys 应成功");
        assert_eq!(keys.len(), 1, "自身键须恰有一个");
        assert_eq!(vm.lookup_str(keys[0]).as_deref(), Some("k"), "键值须为字符串 k");

        // get_own_property_desc：存在属性返描述符，缺失返 None。
        assert!(
            vm.get_own_property_desc(obj_ref, key_si)
                .expect("get_own_property_desc 应成功")
                .is_some(),
            "存在属性须有描述符"
        );
        assert!(
            vm.get_own_property_desc(obj_ref, vm.perm_intern("missing"))
                .expect("get_own_property_desc 应成功")
                .is_none(),
            "缺失属性须返 None"
        );

        // delete_own：删除成功返 true；缺失键删除亦返 true。
        assert!(vm.delete_own(unsafe { &mut *obj }, key_si).expect("delete_own 应成功"), "删除须成功");
        assert!(
            vm.delete_own(unsafe { &mut *obj }, key_si).expect("delete_own 应成功"),
            "缺失键删除须成功"
        );
    }

    /// 7 个 trait 方法：代理路径经 trap 路由（trap 命中、返值原样透传）。
    #[test]
    fn proto_trait_methods_proxy_paths_route_traps() {
        let mut vm = Vm::new();
        let target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        // target 原型须为对象或 null（setPrototypeOf trap 报 true 的不变量检查）。
        unsafe { (*target).set_proto(JsValue::null()).expect("set_proto 应成功") };
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let handler_val = JsValue::from_js_object(handler);
        let get_proto_fn = native_fn_obj(&mut vm, counting_get_prototype_trap);
        let counting_fn = native_fn_obj(&mut vm, counting_trap);
        let own_keys_fn = native_fn_obj(&mut vm, counting_own_keys_trap);
        let gopd_fn = native_fn_obj(&mut vm, counting_gopd_trap);
        for (name, trap_fn) in [
            ("getPrototypeOf", get_proto_fn),
            ("setPrototypeOf", counting_fn),
            ("isExtensible", counting_fn),
            ("preventExtensions", counting_fn),
            ("ownKeys", own_keys_fn),
            ("getOwnPropertyDescriptor", gopd_fn),
            ("deleteProperty", counting_fn),
        ] {
            vm.set_or_create_prop_value(
                unsafe { &mut *handler },
                vm.perm_intern(name),
                JsValue::from_js_object(trap_fn),
            );
        }
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), handler_val);
        let proxy_ref = unsafe { &*proxy };
        let handler_ref = unsafe { &*handler };
        let hits_si = vm.perm_intern("hits");
        let key_si = vm.perm_intern("k");

        // getPrototypeOf：trap 返对象，原样透传。
        let proto = vm.get_prototype_of(proxy_ref).expect("get_prototype_of 应成功");
        assert!(proto.is_object(), "getPrototypeOf trap 返值须为对象");
        assert_eq!(
            vm.resolve_property(handler_ref, hits_si).unwrap(),
            JsValue::int(1),
            "getPrototypeOf trap 须命中"
        );

        // isExtensible：trap 报 true，布尔投影为 true。
        assert!(
            vm.is_extensible(proxy_ref).expect("is_extensible 应成功"),
            "isExtensible trap 报 true 须投影为 true"
        );
        assert_eq!(
            vm.resolve_property(handler_ref, hits_si).unwrap(),
            JsValue::int(2),
            "isExtensible trap 须命中"
        );

        // ownKeys：trap 返数组，原样透传。
        let keys = vm.own_keys(proxy_ref).expect("own_keys 应成功");
        assert_eq!(keys.len(), 1, "ownKeys trap 返值须为单元素数组");
        assert_eq!(vm.lookup_str(keys[0]).as_deref(), Some("k"), "ownKeys trap 返值须原样透传");
        assert_eq!(vm.resolve_property(handler_ref, hits_si).unwrap(), JsValue::int(3), "ownKeys trap 须命中");

        // getOwnPropertyDescriptor：trap 返描述符对象，原样透传。
        let desc = vm
            .get_own_property_desc(proxy_ref, key_si)
            .expect("get_own_property_desc 应成功");
        assert!(desc.is_some(), "getOwnPropertyDescriptor trap 须返描述符");
        assert_eq!(
            vm.resolve_property(handler_ref, hits_si).unwrap(),
            JsValue::int(4),
            "getOwnPropertyDescriptor trap 须命中"
        );

        // deleteProperty：trap 报 true，布尔投影为 true。
        assert!(
            vm.delete_own(unsafe { &mut *proxy }, key_si).expect("delete_own 应成功"),
            "deleteProperty trap 报 true 须投影为 true"
        );
        assert_eq!(
            vm.resolve_property(handler_ref, hits_si).unwrap(),
            JsValue::int(5),
            "deleteProperty trap 须命中"
        );

        // preventExtensions：trap 报 true（与 target 不可扩展状态一致）。
        unsafe { (*target).set_extensible(false) };
        vm.prevent_extensions(unsafe { &mut *proxy })
            .expect("prevent_extensions 应成功");
        assert_eq!(
            vm.resolve_property(handler_ref, hits_si).unwrap(),
            JsValue::int(6),
            "preventExtensions trap 须命中"
        );

        // setPrototypeOf：trap 报 true，不变量检查通过（target 原型为 null）。
        assert!(
            vm.set_prototype_of(unsafe { &mut *proxy }, JsValue::null())
                .expect("set_prototype_of 应成功"),
            "setPrototypeOf trap 报 true 须投影为 true"
        );
        assert_eq!(
            vm.resolve_property(handler_ref, hits_si).unwrap(),
            JsValue::int(7),
            "setPrototypeOf trap 须命中"
        );
    }

    /// 7 个 trait 方法：代理路径不变量检查（getPrototypeOf 返非对象非 null
    /// 抛 TypeError、setPrototypeOf 报 false 返 false、ownKeys 返非数组抛
    /// TypeError）。
    #[test]
    fn proto_trait_methods_proxy_invariants() {
        let mut vm = Vm::new();
        let target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        unsafe { (*target).set_proto(JsValue::null()).expect("set_proto 应成功") };
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        // getPrototypeOf trap 返非对象非 null 值。
        fn bad_get_prototype_trap(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
            NativeResult::Ok(JsValue::int(42))
        }
        // setPrototypeOf trap 报 false。
        fn false_trap(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
            NativeResult::Ok(JsValue::bool(false))
        }
        // ownKeys trap 返非数组值。
        fn bad_own_keys_trap(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
            NativeResult::Ok(JsValue::int(42))
        }
        let get_proto_fn = native_fn_obj(&mut vm, bad_get_prototype_trap);
        let set_proto_fn = native_fn_obj(&mut vm, false_trap);
        let own_keys_fn = native_fn_obj(&mut vm, bad_own_keys_trap);
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("getPrototypeOf"),
            JsValue::from_js_object(get_proto_fn),
        );
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("setPrototypeOf"),
            JsValue::from_js_object(set_proto_fn),
        );
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("ownKeys"),
            JsValue::from_js_object(own_keys_fn),
        );
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        let proxy_ref = unsafe { &*proxy };

        // getPrototypeOf trap 返非对象非 null 抛 TypeError。
        let err = vm.get_prototype_of(proxy_ref).expect_err("须抛 TypeError");
        assert!(err.contains("TypeError"), "须抛 TypeError：{err}");

        // setPrototypeOf trap 报 false 投影为 false（非错误）。
        assert!(
            !vm.set_prototype_of(unsafe { &mut *proxy }, JsValue::null())
                .expect("set_prototype_of 应成功"),
            "trap 报 false 须投影为 false"
        );

        // ownKeys trap 返非数组抛 TypeError。
        let err = vm.own_keys(proxy_ref).expect_err("须抛 TypeError");
        assert!(err.contains("TypeError"), "须抛 TypeError：{err}");
    }

    /// setPrototypeOf 不变量（规范 10.5.2 步 10-12）：trap 报 true 且 target
    /// 不可扩展时，新 proto 与 target 原型非 SameValue 抛 TypeError；target
    /// 可扩展时直接成功（不读 target 原型）。
    #[test]
    fn set_prototype_of_invariant_by_extensibility() {
        let mut vm = Vm::new();
        let target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        unsafe { (*target).set_proto(JsValue::null()).expect("set_proto 应成功") };
        unsafe { (*target).set_extensible(false) };
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let trap_fn = native_fn_obj(&mut vm, counting_trap);
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("setPrototypeOf"),
            JsValue::from_js_object(trap_fn),
        );
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        // 异 proto：不可扩展 target 下 trap 报 true 须抛 TypeError。
        let other = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let err = vm
            .set_prototype_of(unsafe { &mut *proxy }, JsValue::from_js_object(other))
            .expect_err("须抛 TypeError");
        assert!(err.contains("TypeError"), "须抛 TypeError：{err}");
        // 同 proto（null）：不变量通过，返 true。
        assert!(
            vm.set_prototype_of(unsafe { &mut *proxy }, JsValue::null())
                .expect("set_prototype_of 应成功"),
            "同 proto 须返 true"
        );
    }

    /// setPrototypeOf 可扩展早返（规范 10.5.2 步 10-11）：target 可扩展时
    /// trap 报 true 直接成功，不触 target 的 [[GetPrototypeOf]]（其 trap 抛
    /// 异常值验证未调用）。
    #[test]
    fn set_prototype_of_extensible_target_skips_target_proto_read() {
        let mut vm = Vm::new();
        // target 为代理且可扩展：其 getPrototypeOf trap 抛错。
        fn throwing_get_prototype_trap(vm: &mut Vm, _args: &[u8]) -> NativeResult {
            let exc = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
            NativeResult::Err(JsValue::from_js_object(exc))
        }
        let inner_target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let inner_handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let throw_fn = native_fn_obj(&mut vm, throwing_get_prototype_trap);
        vm.set_or_create_prop_value(
            unsafe { &mut *inner_handler },
            vm.perm_intern("getPrototypeOf"),
            JsValue::from_js_object(throw_fn),
        );
        let target =
            proxy_with_state(&mut vm, JsValue::from_js_object(inner_target), JsValue::from_js_object(inner_handler));
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let trap_fn = native_fn_obj(&mut vm, counting_trap);
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("setPrototypeOf"),
            JsValue::from_js_object(trap_fn),
        );
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        let new_proto = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        assert!(
            vm.set_prototype_of(unsafe { &mut *proxy }, JsValue::from_js_object(new_proto))
                .expect("可扩展 target 下 trap 报 true 须成功"),
            "须返 true"
        );
    }

    /// preventExtensions 不变量（规范 10.5.4 步 9）：trap 报 true 而 target
    /// 可扩展抛 TypeError；target 不可扩展时返 true。
    #[test]
    fn prevent_extensions_invariant_by_extensibility() {
        let mut vm = Vm::new();
        let target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let trap_fn = native_fn_obj(&mut vm, counting_trap);
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("preventExtensions"),
            JsValue::from_js_object(trap_fn),
        );
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        // 可扩展 target + trap 报 true：抛 TypeError。
        let err = vm.prevent_extensions(unsafe { &mut *proxy }).expect_err("须抛 TypeError");
        assert!(err.contains("TypeError"), "须抛 TypeError：{err}");
        // 不可扩展 target + trap 报 true：返 true。
        unsafe { (*target).set_extensible(false) };
        assert!(
            vm.prevent_extensions(unsafe { &mut *proxy })
                .expect("prevent_extensions 应成功"),
            "target 不可扩展时 trap 报 true 须返 true"
        );
    }

    /// getPrototypeOf 不变量（规范 10.5.1 步 9-12）：target 不可扩展且 trap
    /// 结果与 target 原型非 SameValue 抛 TypeError。
    #[test]
    fn get_prototype_of_invariant_on_non_extensible_target() {
        let mut vm = Vm::new();
        let target = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        unsafe { (*target).set_proto(JsValue::null()).expect("set_proto 应成功") };
        unsafe { (*target).set_extensible(false) };
        let handler = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined()));
        let trap_fn = native_fn_obj(&mut vm, counting_get_prototype_trap);
        vm.set_or_create_prop_value(
            unsafe { &mut *handler },
            vm.perm_intern("getPrototypeOf"),
            JsValue::from_js_object(trap_fn),
        );
        let proxy = proxy_with_state(&mut vm, JsValue::from_js_object(target), JsValue::from_js_object(handler));
        let err = vm.get_prototype_of(unsafe { &*proxy }).expect_err("须抛 TypeError");
        assert!(err.contains("TypeError"), "须抛 TypeError：{err}");
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
