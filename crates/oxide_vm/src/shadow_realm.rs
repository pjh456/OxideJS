//! ShadowRealm native 实现：构造器（建影子 realm 与实例对象）与 `evaluate` /
//! `importValue` 的 not supported 桩。
//!
//! 关键约定：
//! - 函数是 concrete `&mut Vm` 形态（同 `worker_constructor`），不挂 `VmHost`
//!   trait：构造器要新建 Vm（影子 realm），`VmHost` 接口面表达不了。
//! - 实例对象是 PLAIN 对象（`[[Prototype]]` → ShadowRealm.prototype），主 Vm
//!   侧表 `shadow_realm_objects` 是 brand 校验依据（this 指针在侧表），不占
//!   OBJ_TYPE 槽位。
//! - `evaluate` / `importValue` 现为 not supported 桩（抛含 `not supported`
//!   子串的 TypeError），真实现是后续任务。

use oxide_builtins::error;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{NativeResult, ProtoKind, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::vm::Vm;

/// `new ShadowRealm()`：建影子 realm（独立 Vm）与实例对象。
///
/// # 步骤
/// 1. 构造形态校验（规范无 `[[Call]]`：非构造调用抛 TypeError）。
/// 2. 建影子 Vm：复用主 Vm 的 `KernelCore`（共享 intern/shape/code 缓存）并
///    注入同一动态编译服务。
/// 3. 建实例对象：PLAIN 对象，`[[Prototype]]` → ShadowRealm.prototype。
/// 4. 登记主 Vm 侧表：键 = 影子 realm 编号，值 = （实例对象，影子 Vm）。
/// 5. 返回实例对象。
///
/// # 边界与前提
/// - 影子 Vm 存主 Vm 侧表、不跨线程：`full_reset` 与 `Drop` 统一清理。
/// - 实例对象是 GC 根（侧表登记）；影子 Vm 堆独立，主 Vm 的 GC 不触达。
///
/// # 副作用
/// - 新建一个影子 Vm（新 session 加全量 builtin world）；登记一条侧表条目
///   （GC 根）。
pub(crate) fn shadow_realm_constructor(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    // 构造形态校验（规范只有 [[Construct]]）。
    if !vm.constructing_native {
        return NativeResult::Err(error::create_type_error(vm, "ShadowRealm constructor requires new"));
    }

    // 建影子 Vm：共享主 Vm 的 KernelCore 与动态编译服务。
    let mut shadow_vm = Vm::with_kernel_core(vm.kernel_core.clone());
    shadow_vm.set_compiler_service(vm.compiler.clone());

    // 建实例对象：PLAIN 对象，[[Prototype]] → ShadowRealm.prototype。
    let proto_val = shadow_realm_prototype_value(vm);
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
    let ptr = vm.alloc_object(obj);

    // 登记主 Vm 侧表（键 = 影子 realm 编号，值 = 实例对象加影子 Vm）。
    let realm_id = shadow_vm.realm_id() as u64;
    vm.shadow_realm_objects
        .insert(realm_id, (JsValue::from_js_object(ptr), shadow_vm));

    NativeResult::Ok(JsValue::from_js_object(ptr))
}

/// `ShadowRealm.prototype.evaluate` 桩：按规范前置步骤校验后抛 not supported。
///
/// # 步骤
/// 1. brand 校验（`this` 须是 ShadowRealm 实例）。
/// 2. 实参 `source` 非字符串抛 TypeError（规范只接受 String，不做 ToString）。
/// 3. 编译 `source`（编译期 SyntaxError 原样透传；语法判定与 realm 无关，
///    经调用方编译服务完成）。
/// 4. 抛 not supported。
///
/// # 边界
/// - 桩消息含 `not supported` 子串（test262 判定器据此判能力缺失）。
pub(crate) fn evaluate_stub(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if let Err(e) = shadow_realm_brand_check(vm, this_val) {
        return NativeResult::Err(e);
    }

    // 实参 source（args[1]）：非字符串抛 TypeError。
    let source_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let source = match vm.lookup_str(source_val) {
        Some(s) => s,
        None => {
            return NativeResult::Err(error::create_type_error(
                vm,
                "ShadowRealm.prototype.evaluate source must be a string",
            ))
        }
    };

    // 编译期语法错误原样透传（SyntaxError）。
    if let Err(msg) = vm.compiler.compile_script(&source) {
        return NativeResult::Err(error::create_syntax_error(vm, &msg));
    }

    NativeResult::Err(error::create_type_error(vm, "ShadowRealm.prototype.evaluate is not supported"))
}

/// `ShadowRealm.prototype.importValue` 桩：按规范前置步骤校验后抛 not supported。
///
/// # 步骤
/// 1. brand 校验（`this` 须是 ShadowRealm 实例）。
/// 2. `ToString(specifier)`（abrupt completion 原值传播）。
/// 3. 实参 `exportName` 非字符串抛 TypeError（规范只接受 String，不做 ToString）。
/// 4. 抛 not supported。
///
/// # 边界
/// - 桩消息含 `not supported` 子串（test262 判定器据此判能力缺失）。
pub(crate) fn import_value_stub(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if let Err(e) = shadow_realm_brand_check(vm, this_val) {
        return NativeResult::Err(e);
    }

    // ToString(specifier)：abrupt completion 传播原异常（native 重入点原值已物化
    // 入异常通道）。
    let specifier_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if let Err(e) = oxide_runtime_api::to_string_full(specifier_val, vm) {
        let exc = vm.exception_value.take().unwrap_or_else(|| error::create_from_text(vm, &e));
        return NativeResult::Err(exc);
    }

    // 实参 exportName（args[2]）：非字符串抛 TypeError。
    let export_name_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    if !export_name_val.is_string() {
        return NativeResult::Err(error::create_type_error(
            vm,
            "ShadowRealm.prototype.importValue exportName must be a string",
        ));
    }

    NativeResult::Err(error::create_type_error(vm, "ShadowRealm.prototype.importValue is not supported"))
}

/// brand 校验：`this` 须是对象且指针在主 Vm 侧表内（实例对象注册表）。
fn shadow_realm_brand_check(vm: &mut Vm, this_val: JsValue) -> Result<(), JsValue> {
    if !this_val.is_object() {
        return Err(error::create_type_error(vm, "ShadowRealm method called on non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    let in_table = vm
        .shadow_realm_objects
        .values()
        .any(|(obj, _)| obj.as_js_object_ptr() == this_ptr);
    if !in_table {
        return Err(error::create_type_error(vm, "not a ShadowRealm instance"));
    }
    Ok(())
}

/// 读全局 `ShadowRealm` 构造器的 `prototype` 属性（ShadowRealm.prototype）。
///
/// # 边界
/// - 全局 `ShadowRealm` 槽缺失或构造器无 `prototype` 属性时回退到
///   `%Object.prototype%`（防御性兜底，正常路径 `bind_shadow_realm` 已安装）。
fn shadow_realm_prototype_value(vm: &mut Vm) -> JsValue {
    let object_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::ObjectProto));
    let global = vm.global_object();
    let global_ptr = global.as_ptr() as *mut JsObject;
    // SAFETY: global_ptr 是当前 session 的 global 对象，存活。
    let global_obj = unsafe { &*global_ptr };
    let si_shadow_realm = vm.perm_intern("ShadowRealm");
    let Some(store) = vm.get_own_property_slot(global_obj, si_shadow_realm) else {
        return object_proto;
    };
    let ctor_val = global_obj.get_prop_at(store);
    if !ctor_val.is_object() {
        return object_proto;
    }
    let ctor_ptr = ctor_val.as_js_object_ptr();
    // SAFETY: ctor_ptr 是 ShadowRealm 构造器对象，存活。
    let ctor_obj = unsafe { &*ctor_ptr };
    let si_prototype = vm.perm_intern("prototype");
    let Some(proto_store) = vm.get_own_property_slot(ctor_obj, si_prototype) else {
        return object_proto;
    };
    let proto_val = ctor_obj.get_prop_at(proto_store);
    if proto_val.is_object() {
        proto_val
    } else {
        object_proto
    }
}
