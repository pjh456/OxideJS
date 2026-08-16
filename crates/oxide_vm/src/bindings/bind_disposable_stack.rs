use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_global_value, bind_well_known_data_property,
    bind_well_known_method_alias, configure_native_constructor,
};

/// 在 `DisposableStack.prototype` 上安装方法/别名/访问器/@@toStringTag。
///
/// 幂等：proto 上已有 `use` 方法（全新 proto 或已完整绑定）时整组跳过，
/// 保留原型经 dirty reset 重复经过时安全。`@@dispose` 别名须在 `dispose`
/// 绑定之后调用（读源槽 lookup_position，顺序敏感）。
pub fn bind_disposable_stack_protos(core: &Arc<KernelCore>, session: &KernelSession) {
    let world = session.builtin_world();
    let proto_ptr = world.disposable_stack_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    let si_use = core.perm_interner().intern("use").0;
    if core.shape_forge().lookup_position(proto.shape_id(), si_use).is_some() {
        return;
    }

    apply_binding_table(
        world,
        proto,
        core,
        &[
            (
                "use",
                oxide_builtins::disposable_stack::disposable_stack_use::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "adopt",
                oxide_builtins::disposable_stack::disposable_stack_adopt::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "defer",
                oxide_builtins::disposable_stack::disposable_stack_defer::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "dispose",
                oxide_builtins::disposable_stack::disposable_stack_dispose::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "move",
                oxide_builtins::disposable_stack::disposable_stack_move::<crate::vm::Vm> as *const (),
                0,
            ),
        ],
    );
    bind_accessor_getter(
        core,
        session,
        proto,
        "disposed",
        oxide_builtins::disposable_stack::disposable_stack_disposed_getter::<crate::vm::Vm> as *const (),
    );
    // @@dispose 与 dispose 为同一函数对象（先绑 dispose 再取源槽）。
    bind_well_known_method_alias(core, proto, "dispose", 12);
    let sf = core.perm_interner().as_ref();
    let tag = JsValue::perm_string(sf.string_ptr(sf.intern("DisposableStack").0));
    bind_well_known_data_property(core, proto, 9, tag, PropAttributes::new(false, false, true));
}

/// 在 proto 上写 `constructor` 数据属性（非枚举，描述符 {t,f,t}）。
pub(crate) fn write_proto_constructor(core: &Arc<KernelCore>, proto: &mut JsObject, ctor_val: JsValue) {
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();
    let si_constructor = sf.intern("constructor").0;
    if sh.lookup_position(proto.shape_id(), si_constructor).is_some() {
        return;
    }
    let new_shape = sh.make_shape(proto.shape_id(), si_constructor);
    proto.set_shape_id(new_shape);
    let pos = proto.push_prop(ctor_val);
    proto.set_data_meta(pos, PropAttributes::new(true, false, true));
    proto.bump_generation();
}

/// 把 `DisposableStack` 构造器绑定到 global（init 与 full_reset global 重建共用）。
///
/// proto.constructor 已有则复用（dirty reset 保留路径）；否则 Box 自建构造器
/// （proto=Function.prototype，prototype 槽=DisposableStack.prototype，描述符按规范）。
/// 幂等：protos 安装与 global 槽写入均自带重复跳过。
pub fn bind_disposable_stack(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let world = session.builtin_world();
    let proto_ptr = world.disposable_stack_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    let si_constructor = core.perm_interner().intern("constructor").0;
    let ctor_val = match core.shape_forge().lookup_position(proto.shape_id(), si_constructor) {
        Some(pos) => {
            let existing = proto.get_prop_at(pos);
            if existing.is_object() {
                existing
            } else {
                JsValue::undefined()
            }
        }
        None => JsValue::undefined(),
    };
    let ctor_val = if ctor_val.is_object() {
        ctor_val
    } else {
        // Box 自建：[[Prototype]] 指向 Function.prototype，prototype/name 描述符按规范。
        let function_proto = world.function_proto.as_ptr() as *mut JsObject;
        let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto)));
        ctor.set_function(true);
        configure_native_constructor(
            &mut ctor,
            oxide_builtins::disposable_stack::disposable_stack_constructor::<crate::vm::Vm> as *const (),
            0,
        );
        let sf = core.perm_interner().as_ref();
        let sh = core.shape_forge().as_ref();
        let si_prototype = sf.intern("prototype").0;
        let si_name = sf.intern("name").0;
        let si_length = sf.intern("length").0;
        let ctor_shape1 = sh.make_shape(EMPTY_SHAPE_ID, si_prototype);
        let ctor_shape2 = sh.make_shape(ctor_shape1, si_name);
        let ctor_shape3 = sh.make_shape(ctor_shape2, si_length);
        ctor.set_shape_id(ctor_shape3);
        ctor.ensure_hash_props()
            .push(JsValue::from_js_object(world.disposable_stack_proto.as_ptr() as *mut JsObject));
        ctor.ensure_hash_props()
            .push(JsValue::perm_string(sf.string_ptr(sf.intern("DisposableStack").0)));
        ctor.ensure_hash_props().push(JsValue::int(0));
        ctor.set_data_meta(0u32, PropAttributes::new(false, false, false));
        ctor.set_data_meta(1u32, PropAttributes::new(false, false, true));
        ctor.set_data_meta(2u32, PropAttributes::new(false, false, true));
        JsValue::from_js_object(Box::into_raw(ctor))
    };

    write_proto_constructor(core, proto, ctor_val);
    bind_disposable_stack_protos(core, session);
    bind_global_value(core, global, "DisposableStack", ctor_val);
}

/// 对齐保留 global 上 `DisposableStack` 函数对象的 `prototype` 属性：object 家族
/// 重建会产生新的 `DisposableStack.prototype`，保留的构造器须指向新原型，
/// 否则 reset 后 `instanceof` 沿旧原型链恒 false。
///
/// global 同时重建（global dirty）或初始化时无 `DisposableStack` 属性，直接跳过
/// ——彼时由 `bind_disposable_stack` 以新原型创建函数对象。
pub fn sync_disposable_stack_ctor(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let si_stack = core.perm_interner().intern("DisposableStack").0;
    let Some(pos) = core.shape_forge().lookup_position(global.shape_id(), si_stack) else {
        return;
    };
    let stack_val = global.get_prop_at(pos);
    if !stack_val.is_object() {
        return;
    }
    let ctor = unsafe { &mut *stack_val.as_js_object_ptr() };
    let si_prototype = core.perm_interner().intern("prototype").0;
    let Some(proto_pos) = core.shape_forge().lookup_position(ctor.shape_id(), si_prototype) else {
        return;
    };
    let new_proto = JsValue::from_js_object(session.builtin_world().disposable_stack_proto.as_ptr() as *mut JsObject);
    ctor.set_prop_at(proto_pos, new_proto);

    let proto_ptr = session.builtin_world().disposable_stack_proto.as_ptr() as *mut JsObject;
    write_proto_constructor(core, unsafe { &mut *proto_ptr }, stack_val);
}
