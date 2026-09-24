//! Weak 族绑定：WeakMap 构造器 + 原型四方法（set/get/has/delete）安装。
//!
//! 弱族不占 BuiltinWorld 槽：构造器经 `P` 留存 `stub_objects`（global
//! 重建路径原位重绑，索引 4 紧随 stub 族四条目），原型对象经
//! `Box::into_raw` 登记 world 释放登记表（属性装完再登记，登记点世代即
//! 洁净基线），恒存活至 session 收尾统一释放；选择性重建经
//! `inherit_leaked_objects` 迁移登记表、`retire_replaced` 重指原型链，
//! 重绑时全局槽原位更新不追加。

use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::configure_native_constructor;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::P;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 在原型上安装一条 native 方法，站点标签 = 族名 perm 键：非 P 原型目标
/// 全部落 family-0，无标签时 WeakMap 与后续弱族同名方法槽（has/delete）
/// 在 FnWrapperKey 复用键上撞键，重绑会迁移到错误家族的 wrapper。
fn bind_labeled_method(
    world: &oxide_kernel::builtin::BuiltinWorld, proto: &mut JsObject, core: &Arc<KernelCore>, label: u32, name: &str,
    func: *const (), nargs: u8,
) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    // SAFETY: func 是转成 *const () 的 NativeFn 函数项指针。
    let fn_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(func) };
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method_labeled_static(
        proto,
        shape_forge,
        string_forge,
        name,
        fn_ptr,
        nargs,
        world,
        label,
    );
}

/// 把 WeakMap 构造器与原型四方法绑定到 global。
///
/// # 副作用
/// - 构造器经 `P` 留存 `stub_objects`（索引 4，紧随 stub 族四条目）；
///   原型对象经释放登记表登记，恒存活至 session 收尾；全局 `WeakMap` 槽
///   经 `bind_constructor!` 原位安装（既有槽更新槽值，不追加新槽）。
/// - 方法 wrapper 带族名站点标签登记复用键，选择性重建时按键迁移，
///   登记表跨重建不增长。
pub fn bind_weak_map(core: &Arc<KernelCore>, session: &mut KernelSession, global: &mut JsObject) {
    // world 唯一属主期：全程走可变引用（stub_objects 留存 + 释放登记同表）。
    let world = Arc::get_mut(&mut session.builtin_world)
        .expect("BuiltinWorld must be uniquely owned during init_kernel_builtins");
    let object_proto_ptr = world.object_proto.as_ptr() as *mut JsObject;
    let function_proto_ptr = world.function_proto.as_ptr() as *mut JsObject;
    let label = core.perm_interner().intern("WeakMap").0;
    let si_name = core.perm_interner().intern("name").0;

    // 构造器对象：Function 原型链 + native 函数项；name 由本函数安装
    // （bind_constructor! 只补 length），描述符 { f,f,t }。
    let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto_ptr));
    ctor.set_function(true);
    configure_native_constructor(
        &mut ctor,
        oxide_builtins::weak_map::weak_map_constructor::<crate::vm::Vm> as *const (),
        0,
    );
    let name_shape = core.shape_forge().make_shape(ctor.shape_id(), si_name);
    ctor.set_shape_id(name_shape);
    ctor.ensure_hash_props().push(JsValue::perm_string(
        core.perm_interner().string_ptr(core.perm_interner().intern("WeakMap").0),
    ));
    let name_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(name_pos, PropAttributes::new(false, false, true));

    let stub = P::new(ctor);
    let ctor_ptr = stub.as_ptr() as *mut JsObject;

    // 原型对象：constructor 回指（{ w,f,t }）+ @@toStringTag（{ f,f,t }），
    // 四方法 { w,f,t } 经站点标签绑定；属性装完再登记释放登记表。
    let mut proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto_ptr));
    let si_constructor = core.perm_interner().intern("constructor").0;
    let ctor_shape = core.shape_forge().make_shape(proto.shape_id(), si_constructor);
    proto.set_shape_id(ctor_shape);
    proto.ensure_hash_props().push(JsValue::from_js_object(ctor_ptr));
    let ctor_pos = proto.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    proto.set_data_meta(ctor_pos, PropAttributes::new(true, false, true));
    let tag_key =
        oxide_types::private_key::make_well_known_symbol_key(oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG);
    let tag_shape = core.shape_forge().make_shape(proto.shape_id(), tag_key);
    proto.set_shape_id(tag_shape);
    proto.ensure_hash_props().push(JsValue::perm_string(
        core.perm_interner().string_ptr(core.perm_interner().intern("WeakMap").0),
    ));
    let tag_pos = proto.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    proto.set_data_meta(tag_pos, PropAttributes::new(false, false, true));
    bind_labeled_method(
        world,
        &mut proto,
        core,
        label,
        "set",
        oxide_builtins::weak_map::weak_map_set::<crate::vm::Vm> as *const (),
        2,
    );
    bind_labeled_method(
        world,
        &mut proto,
        core,
        label,
        "get",
        oxide_builtins::weak_map::weak_map_get::<crate::vm::Vm> as *const (),
        1,
    );
    bind_labeled_method(
        world,
        &mut proto,
        core,
        label,
        "has",
        oxide_builtins::weak_map::weak_map_has::<crate::vm::Vm> as *const (),
        1,
    );
    bind_labeled_method(
        world,
        &mut proto,
        core,
        label,
        "delete",
        oxide_builtins::weak_map::weak_map_delete::<crate::vm::Vm> as *const (),
        1,
    );
    let proto_ptr = Box::into_raw(Box::new(proto));
    world.track_leaked_object(proto_ptr);

    // 构造器 prototype 槽（{ f,f,f }）：P 留存 stub_objects（global 重建
    // 原位重绑依赖索引 4）后，经 bind_constructor! 安装全局槽。
    // SAFETY: ctor 尚未发布到 global，局部 P 为唯一持有者，可变访问无读者冲突。
    let ctor = unsafe { &mut *ctor_ptr };
    let si_prototype = core.perm_interner().intern("prototype").0;
    let proto_shape = core.shape_forge().make_shape(ctor.shape_id(), si_prototype);
    ctor.set_shape_id(proto_shape);
    ctor.ensure_hash_props().push(JsValue::from_js_object(proto_ptr));
    let proto_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(proto_pos, PropAttributes::new(false, false, false));
    world.stub_objects.push(stub);

    bind_constructor!(
        core,
        global,
        "WeakMap",
        ctor_ptr,
        oxide_builtins::weak_map::weak_map_constructor::<crate::vm::Vm>,
        0,
        hash: true
    );
}
