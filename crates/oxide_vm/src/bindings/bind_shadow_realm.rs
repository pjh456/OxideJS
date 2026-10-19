//! ShadowRealm 构造器绑定：Box 分配构造器（不占 `BuiltinWorld` P 字段、不新增
//! `BuiltinId`、不占 `BuiltinDirtySet` 脏位）加 prototype 与全局槽，全槽
//! `lookup_position` 幂等守卫。
//!
//! 与 `bind_worker` 同型：构造器经 `Box::into_raw` 登记 world 释放
//! 登记表（session 收尾统一释放），全局 `ShadowRealm` 槽经 `bind_constructor!`
//! 原位安装（既有槽更新槽值、无槽新开）。

use crate::bind_constructor;
use crate::bindings::{
    apply_binding_table, bind_well_known_data_property, configure_native_constructor, BindEnv,
};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 ShadowRealm 构造器（Box 分配）与 prototype 方法绑定到 global。
///
/// # 幂等
/// 本函数从三条路径到达：`init_kernel_builtins`（初始）、`rebind_dirty_builtins`
/// 的无条件分支（构造器不在脏体系内）、global-dirty 路径
/// （`bind_global_builtin_slots`）。构造器本体经 `lookup_position` 守卫复用（有
/// 槽即不新建）；原型槽位经 `lookup_position` 守卫（有槽即跳过）；
/// `bind_constructor!` 的全局槽臂自带既有槽原位更新。
///
/// # 副作用
/// - 构造器与原型对象经 `Box::into_raw` 登记 world 释放登记表（session 收尾统一
///   释放）；全局 `ShadowRealm` 槽经 `bind_constructor!` 原位安装。
/// - prototype 上装 `constructor` 指回构造器、`@@toStringTag` 数据属性
///   "ShadowRealm" 与 `evaluate` / `importValue` 方法（not supported 桩）。
///
/// `realm_id` 经 `@@toStringTag` 符号键编码使用（well-known 符号键按
/// （realm 编号，局部下标）编码）。
pub fn bind_shadow_realm(env: BindEnv<'_>, global: &mut JsObject) {
    let world = env.world();
    let sf = env.string_forge();
    let sh = env.shape_forge();

    // ---- 构造器本体（Box 分配，不占 P 字段）----
    // 幂等：全局 `ShadowRealm` 槽已有构造器时复用（不新建）。
    let si_shadow_realm = sf.intern("ShadowRealm").0;
    let ctor_ptr: *mut JsObject = match sh.lookup_position(global.shape_id(), si_shadow_realm) {
        Some(pos) if global.get_prop_at(pos).is_object() => global.get_prop_at(pos).as_js_object_ptr(),
        _ => {
            let function_proto = world.function_proto.as_ptr() as *mut JsObject;
            let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto));
            ctor.set_function(true);
            configure_native_constructor(
                &mut ctor,
                crate::shadow_realm::shadow_realm_constructor as *const (),
                0,
            );
            let ptr = Box::into_raw(Box::new(ctor));
            world.track_leaked_object(ptr);
            ptr
        }
    };

    // 构造器 name 槽（描述符 { f,f,t }），幂等守卫。
    let si_name = sf.intern("name").0;
    {
        let ctor = unsafe { &mut *ctor_ptr };
        if sh.lookup_position(ctor.shape_id(), si_name).is_none() {
            let shape = sh.make_shape(ctor.shape_id(), si_name);
            ctor.set_shape_id(shape);
            ctor.ensure_hash_props()
                .push(JsValue::perm_string(sf.string_ptr(sf.intern("ShadowRealm").0)));
            let pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
            ctor.set_data_meta(pos, PropAttributes::new(false, false, true));
            ctor.bump_generation();
        }
    }

    // ---- 原型占位 ----
    // 幂等：构造器 prototype 槽已有原型时复用（不新建）。
    let si_prototype = sf.intern("prototype").0;
    let proto_ptr: *mut JsObject = {
        let ctor = unsafe { &*ctor_ptr };
        match sh.lookup_position(ctor.shape_id(), si_prototype) {
            Some(pos) if ctor.get_prop_at(pos).is_object() => ctor.get_prop_at(pos).as_js_object_ptr(),
            _ => {
                // ShadowRealm.prototype 的 [[Prototype]] 指向 Object.prototype。
                let object_proto = world.object_proto.as_ptr() as *mut JsObject;
                let proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
                let ptr = Box::into_raw(Box::new(proto));
                world.track_leaked_object(ptr);
                // 回填构造器 prototype 槽（描述符 { f,f,f }）。
                let ctor = unsafe { &mut *ctor_ptr };
                let shape = sh.make_shape(ctor.shape_id(), si_prototype);
                ctor.set_shape_id(shape);
                ctor.ensure_hash_props().push(JsValue::from_js_object(ptr));
                let pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
                ctor.set_data_meta(pos, PropAttributes::new(false, false, false));
                ctor.bump_generation();
                ptr
            }
        }
    };

    {
        let proto = unsafe { &mut *proto_ptr };

        // constructor 槽 → 构造器对象（描述符 { t,f,t }），幂等守卫。
        let si_constructor = sf.intern("constructor").0;
        if sh.lookup_position(proto.shape_id(), si_constructor).is_none() {
            let shape = sh.make_shape(proto.shape_id(), si_constructor);
            proto.set_shape_id(shape);
            let pos = proto.push_prop(JsValue::from_js_object(ctor_ptr));
            proto.set_data_meta(pos, PropAttributes::new(true, false, true));
            proto.bump_generation();
        }

        // @@toStringTag 数据属性 "ShadowRealm"（描述符 { f,f,t }），幂等守卫。
        let tag_key = oxide_types::private_key::encode_symbol_key(
            env.realm_id(),
            oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG,
        );
        if sh.lookup_position(proto.shape_id(), tag_key).is_none() {
            let tag_val = JsValue::perm_string(sf.string_ptr(sf.intern("ShadowRealm").0));
            bind_well_known_data_property(
                env.core,
                proto,
                oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG,
                tag_val,
                PropAttributes::new(false, false, true),
                env.realm_id(),
            );
        }

        // evaluate / importValue 方法（not supported 桩，幂等守卫）。
        let si_evaluate = sf.intern("evaluate").0;
        if sh.lookup_position(proto.shape_id(), si_evaluate).is_none() {
            apply_binding_table(
                env,
                proto,
                &[(
                    "evaluate",
                    crate::shadow_realm::evaluate_stub as *const (),
                    1,
                )],
            );
        }
        let si_import_value = sf.intern("importValue").0;
        if sh.lookup_position(proto.shape_id(), si_import_value).is_none() {
            apply_binding_table(
                env,
                proto,
                &[(
                    "importValue",
                    crate::shadow_realm::import_value_stub as *const (),
                    2,
                )],
            );
        }
    }

    // 全局 `ShadowRealm` 槽（bind_constructor! 自带既有槽原位更新、无槽新开）。
    bind_constructor!(
        env,
        global,
        "ShadowRealm",
        ctor_ptr,
        crate::shadow_realm::shadow_realm_constructor,
        0,
        hash: true
    );
}
