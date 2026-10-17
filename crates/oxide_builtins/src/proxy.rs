//! %Proxy% 构造器与 revocable：Proxy 状态盒（target、handler、revoked 位）、
//! 参数与不变量校验、函数代理 is_function 位、Proxy.revocable 的撤销函数。
//!
//! 状态盒经 `Box::into_raw` 挂 `JsObject.native_data`，GC 边函数族（mark 边）
//! 由 `oxide_vm` 的家族表消费；撤销函数以独立 tag 存被撤销代理指针（GC 边），
//! 保证仅持撤销函数时代理不被回收。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{NativeResult, ProtoKind, VmHost};
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

/// Proxy 状态盒：存于 Proxy 对象 `native_data`，target 与 handler 是 GC 边，
/// revoked 位标记已撤销状态（撤销后全部 trap 抛 TypeError）。
///
/// 生命周期与 Proxy 对象一致：构造期经 `Box::into_raw` 分配，GC drop 链释放，
/// 此后不替换。
pub struct ProxyState {
    /// 目标对象（GC 边）。
    pub target: JsValue,
    /// 处理器对象（GC 边）。
    pub handler: JsValue,
    /// 撤销位：置位后所有 trap 派发抛 TypeError。
    pub revoked: bool,
}

/// 读 Proxy 状态盒（盒未建立时 None）。
pub fn proxy_state_ref(obj: &JsObject) -> Option<&ProxyState> {
    let ptr = obj.native_data() as *const ProxyState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 由 Box::into_raw 分配，生命周期与 Proxy 对象一致。
        Some(unsafe { &*ptr })
    }
}

/// 可变访问 Proxy 状态盒（写 revoked 位）。
pub fn proxy_state_mut(obj: &mut JsObject) -> Option<&mut ProxyState> {
    let ptr = obj.native_data() as *mut ProxyState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 由 Box::into_raw 分配，生命周期与 Proxy 对象一致。
        Some(unsafe { &mut *ptr })
    }
}

/// 撤销函数的对象边（GC mark 强边）：`native_data` 存的被撤销代理指针。
pub fn proxy_revoker_native_edges(obj: &JsObject) -> Vec<JsValue> {
    if !obj.is_proxy_revoker_obj() {
        return Vec::new();
    }
    let ptr = obj.native_data() as *mut JsObject;
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: native_data 存存活代理指针（GC 边），mark 期对象存活。
    vec![JsValue::from_js_object(ptr)]
}

/// 参数校验：target 与 handler 均须对象，否则抛 TypeError（规范 10.2.4 步骤 1-2）。
fn check_target_and_handler(target: JsValue, handler: JsValue) -> Result<(), &'static str> {
    if !target.is_object() || target.as_js_object_ptr().is_null() {
        return Err("Proxy target must be an object");
    }
    if !handler.is_object() || handler.as_js_object_ptr().is_null() {
        return Err("Proxy handler must be an object");
    }
    Ok(())
}

/// 不变量检查（规范 10.2.4 步骤 4-5）：handler 的 isExtensible 与 setPrototypeOf
/// trap 若存在，其布尔值须与 target 实际状态一致，否则抛 TypeError。
///
/// # 边界与前提
/// - 仅当 handler 自有对应 trap 属性时检查（继承属性不触发）。
/// - trap 读为传播读，getter 抛错原值上抛。
fn proxy_create_invariants<H: VmHost>(
    vm: &mut H, target_obj: &JsObject, handler_obj: &JsObject, handler: JsValue,
) -> Result<(), JsValue> {
    // isExtensible trap：handler 报 true 时 target 须可扩展，报 false 时须不可扩展。
    let si_is_extensible = vm.perm_intern("isExtensible");
    if vm.get_own_property_slot(handler_obj, si_is_extensible).is_some() {
        let trap_val = vm
            .ordinary_get(handler_obj, si_is_extensible, handler)
            .map_err(|e| crate::iterator::engine_error(vm, &e))?;
        let target_extensible = target_obj.is_extensible();
        let trap_is_true = trap_val.is_bool() && trap_val.as_bool();
        let trap_is_false = trap_val.is_bool() && !trap_val.as_bool();
        if (trap_is_true && !target_extensible) || (trap_is_false && target_extensible) {
            return Err(crate::error::create_type_error(
                vm,
                "Proxy invariant: isExtensible trap inconsistent with target",
            ));
        }
    }
    // setPrototypeOf trap：handler 报 true 时 target 原型须为对象或 null，
    // 报 false 时须为其它类型。
    let si_set_proto = vm.perm_intern("setPrototypeOf");
    if vm.get_own_property_slot(handler_obj, si_set_proto).is_some() {
        let trap_val = vm
            .ordinary_get(handler_obj, si_set_proto, handler)
            .map_err(|e| crate::iterator::engine_error(vm, &e))?;
        let target_proto = target_obj.proto();
        let target_proto_is_obj_or_null = target_proto.is_object() || target_proto.is_null();
        let trap_is_true = trap_val.is_bool() && trap_val.as_bool();
        let trap_is_false = trap_val.is_bool() && !trap_val.as_bool();
        if (trap_is_true && !target_proto_is_obj_or_null) || (trap_is_false && target_proto_is_obj_or_null) {
            return Err(crate::error::create_type_error(
                vm,
                "Proxy invariant: setPrototypeOf trap inconsistent with target",
            ));
        }
    }
    Ok(())
}

/// 创建代理对象本体：tag 39 + ProxyState 载荷盒 + Object.prototype 原型链；
/// target 可调用时置 is_function 位（typeof 经该位返回 function）。
fn create_proxy_object<H: VmHost>(vm: &mut H, target: JsValue, handler: JsValue) -> JsValue {
    let object_proto = vm.builtin_proto(ProtoKind::ObjectProto);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
    obj.type_tag = JsObject::OBJ_TYPE_PROXY;
    let state = Box::new(ProxyState {
        target,
        handler,
        revoked: false,
    });
    obj.set_native_data(Box::into_raw(state) as *mut u8);
    if crate::iterator::is_callable(target) {
        obj.set_function(true);
    }
    let proxy_obj = vm.alloc_object(obj);
    JsValue::from_js_object(proxy_obj)
}

/// `Proxy ( target, handler )` 构造器：NewTarget 校验（须经 new 调用）后按
/// 规范 10.2.4 建代理对象并返回。
///
/// # 步骤
/// 1. NewTarget 校验：非构造形态调用抛 TypeError。
/// 2. 参数校验：target 与 handler 均须对象。
/// 3. 不变量检查：handler 的 isExtensible / setPrototypeOf trap 与 target 一致性。
/// 4. 创建代理对象（tag 39 + 状态盒 + Object.prototype 原型链）。
///
/// # 边界与前提
/// - %Proxy% 无 prototype 属性（规范 25.1），返回的代理对象原型链落
///   Object.prototype。
/// - target 可调用时代理置 is_function 位（typeof 返 function）。
pub fn proxy_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if !vm.constructing_native() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Proxy constructor requires 'new'"));
    }
    let target = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let handler = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    if let Err(msg) = check_target_and_handler(target, handler) {
        return NativeResult::Err(crate::error::create_type_error(vm, msg));
    }
    // SAFETY: check_target_and_handler 已校验非空对象指针。
    let target_obj = unsafe { &*target.as_js_object_ptr() };
    let handler_obj = unsafe { &*handler.as_js_object_ptr() };
    if let Err(exc) = proxy_create_invariants(vm, target_obj, handler_obj, handler) {
        return NativeResult::Err(exc);
    }
    NativeResult::Ok(create_proxy_object(vm, target, handler))
}

/// `Proxy.revocable ( target, handler )`：建代理对象后返回 `{proxy, revoke}`
/// 结果对象；revoke 为撤销函数（调用后代理全部 trap 抛 TypeError）。
///
/// # 步骤
/// 1. 参数校验与不变量检查（同构造器）。
/// 2. 创建代理对象。
/// 3. 创建撤销函数（length 0、name 空串、非构造器，native_data 存代理指针）。
/// 4. 建结果对象并装 `proxy` / `revoke` 两数据属性。
///
/// # 边界与前提
/// - 撤销函数经独立 tag 存代理指针，GC 边保活代理（仅持撤销函数时代理不回收）。
/// - 撤销函数可重复调用，幂等置 revoked 位。
pub fn proxy_revocable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let handler = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    if let Err(msg) = check_target_and_handler(target, handler) {
        return NativeResult::Err(crate::error::create_type_error(vm, msg));
    }
    // SAFETY: check_target_and_handler 已校验非空对象指针。
    let target_obj = unsafe { &*target.as_js_object_ptr() };
    let handler_obj = unsafe { &*handler.as_js_object_ptr() };
    if let Err(exc) = proxy_create_invariants(vm, target_obj, handler_obj, handler) {
        return NativeResult::Err(exc);
    }
    let proxy_val = create_proxy_object(vm, target, handler);

    // 撤销函数：Function.prototype 原型链 + native 函数项 + length 0 / name 空串。
    let function_proto = vm.builtin_proto(ProtoKind::FunctionProto);
    let mut revoker = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto));
    revoker.set_function(true);
    revoker.type_tag = JsObject::OBJ_TYPE_PROXY_REVOKER;
    // SAFETY: proxy_revoker_dispatcher 是合法 NativeFn 函数项。
    revoker.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(proxy_revoker_dispatcher::<H> as *const ()) }));
    revoker.set_native_arg_count(0);
    let revoker_obj = vm.alloc_object(revoker);
    let revoker_ref = unsafe { &mut *revoker_obj };
    let length_si = vm.perm_intern("length");
    if let Err(e) =
        vm.define_data_property(revoker_ref, length_si, JsValue::int(0), PropAttributes::new(false, false, true))
    {
        return NativeResult::Err(crate::iterator::engine_error(vm, &e));
    }
    let name_si = vm.perm_intern("name");
    let empty_name = vm.new_string("");
    if let Err(e) = vm.define_data_property(revoker_ref, name_si, empty_name, PropAttributes::new(false, false, true)) {
        return NativeResult::Err(crate::iterator::engine_error(vm, &e));
    }
    // 代理指针存进撤销函数 native_data（GC 边，mark 期经家族表传播）。
    unsafe {
        (*revoker_obj).set_native_data(proxy_val.as_js_object_ptr() as *mut u8);
    }
    let revoker_val = JsValue::from_js_object(revoker_obj);

    // 结果对象 {proxy, revoke}：两数据属性（可写、可枚举、可配置）。
    let object_proto = vm.builtin_proto(ProtoKind::ObjectProto);
    let result_obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let result_ref = unsafe { &mut *result_obj };
    let proxy_si = vm.perm_intern("proxy");
    let revoke_si = vm.perm_intern("revoke");
    vm.set_or_create_prop_value(result_ref, proxy_si, proxy_val);
    vm.set_or_create_prop_value(result_ref, revoke_si, revoker_val);
    NativeResult::Ok(JsValue::from_js_object(result_obj))
}

/// 撤销函数调用体：置被撤销代理的 revoked 位，幂等返回 undefined。
///
/// # 步骤
/// 1. 经 callee 槽（reg 254）读撤销函数本体。
/// 2. 读其 native_data 的被撤销代理指针。
/// 3. 置代理状态盒 revoked 位。
///
/// # 边界与前提
/// - 撤销函数本体缺失或代理指针为空时静默返 undefined（防御性）。
fn proxy_revoker_dispatcher<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let revoker_val = vm.reg(254);
    let revoker_ptr = revoker_val.as_js_object_ptr();
    if revoker_ptr.is_null() {
        return NativeResult::Ok(JsValue::undefined());
    }
    // SAFETY: revoker 为存活函数对象。
    let revoker = unsafe { &*revoker_ptr };
    let proxy_ptr = revoker.native_data() as *mut JsObject;
    if proxy_ptr.is_null() {
        return NativeResult::Ok(JsValue::undefined());
    }
    // SAFETY: proxy_ptr 为存活代理对象（经撤销函数 GC 边保活）。
    let proxy = unsafe { &mut *proxy_ptr };
    let Some(state) = proxy_state_mut(proxy) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    state.revoked = true;
    NativeResult::Ok(JsValue::undefined())
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
    fn proxy_state_ref_reads_target_and_handler() {
        let target = plain_obj_value();
        let handler = plain_obj_value();
        let obj = proxy_obj(target, handler);
        let state = proxy_state_ref(&obj).expect("状态盒");
        assert_eq!(state.target, target);
        assert_eq!(state.handler, handler);
        assert!(!state.revoked);
    }

    #[test]
    fn proxy_state_mut_toggles_revoked() {
        let mut obj = proxy_obj(plain_obj_value(), plain_obj_value());
        let state = proxy_state_mut(&mut obj).expect("状态盒");
        state.revoked = true;
        assert!(proxy_state_ref(&obj).expect("状态盒").revoked);
    }

    #[test]
    fn proxy_revoker_edges_return_proxy_pointer() {
        let proxy = plain_obj_value();
        let mut revoker = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        revoker.type_tag = JsObject::OBJ_TYPE_PROXY_REVOKER;
        revoker.set_native_data(proxy.as_js_object_ptr() as *mut u8);
        assert_eq!(proxy_revoker_native_edges(&revoker), vec![proxy]);
    }

    #[test]
    fn proxy_revoker_edges_empty_for_non_revoker() {
        let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        assert!(proxy_revoker_native_edges(&obj).is_empty());
    }
}
