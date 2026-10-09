use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, bind_accessor_getter, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 BroadcastChannel 构造器与原型方法绑定到 global。
///
/// 935.2.1 的两枚 P 字段（`broadcast_channel_constructor` /
/// `broadcast_channel_proto`）是空对象占位，本函数就地填充（不新建对象——
/// P 字段本身就是槽位，快照/脏检查/收尾枚举已覆盖）。
///
/// # 幂等
/// 本函数从三条路径到达：`init_kernel_builtins`（占位是空对象）、`rebind_dirty_
/// builtins` 的 `broadcast_channel` 分支（占位是重建后的空对象）、global-dirty
/// 路径（占位可能已填满）。全部槽写经 `lookup_position` 前置守卫，有槽即跳过；
/// `bind_constructor!` 的全局槽臂自带既有槽原位更新，天然幂等。
///
/// `realm_id` 未使用（BroadcastChannel 无 well-known 符号键），保留签名与其他
/// 绑定函数一致。
pub fn bind_broadcast_channel(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject, _realm_id: u32) {
    let world = session.builtin_world();
    let ctor_ptr = world.broadcast_channel_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = world.broadcast_channel_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();

    // ---- 填构造器占位 ----
    ctor.set_function(true);

    // prototype 槽 → 原型对象（描述符 {f,f,f}）。
    let si_prototype = sf.intern("prototype").0;
    if sh.lookup_position(ctor.shape_id(), si_prototype).is_none() {
        let shape = sh.make_shape(ctor.shape_id(), si_prototype);
        ctor.set_shape_id(shape);
        let pos = ctor.push_prop(JsValue::from_js_object(proto_ptr));
        ctor.set_data_meta(pos, PropAttributes::new(false, false, false));
        ctor.bump_generation();
    }

    // name 槽 → "BroadcastChannel"（描述符 {f,f,t}）。
    let si_name = sf.intern("name").0;
    if sh.lookup_position(ctor.shape_id(), si_name).is_none() {
        let shape = sh.make_shape(ctor.shape_id(), si_name);
        ctor.set_shape_id(shape);
        ctor.ensure_hash_props()
            .push(JsValue::perm_string(sf.string_ptr(sf.intern("BroadcastChannel").0)));
        let pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        ctor.set_data_meta(pos, PropAttributes::new(false, false, true));
        ctor.bump_generation();
    }

    // [[Prototype]] → Function.prototype（幂等，裸写槽）。
    let fn_proto_val = JsValue::from_js_object(world.function_proto.as_ptr() as *mut JsObject);
    let _ = ctor.set_proto(fn_proto_val);

    configure_native_constructor(
        ctor,
        oxide_builtins::broadcast_channel::broadcast_channel_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_constructor!(
        core,
        global,
        "BroadcastChannel",
        ctor_ptr,
        oxide_builtins::broadcast_channel::broadcast_channel_constructor::<crate::vm::Vm>,
        1,
        hash: true
    );

    // ---- 填原型占位 ----
    // constructor 槽 → 构造器对象（描述符 {t,f,t}）。
    let si_constructor = sf.intern("constructor").0;
    if sh.lookup_position(proto.shape_id(), si_constructor).is_none() {
        let shape = sh.make_shape(proto.shape_id(), si_constructor);
        proto.set_shape_id(shape);
        let pos = proto.push_prop(JsValue::from_js_object(ctor_ptr));
        proto.set_data_meta(pos, PropAttributes::new(true, false, true));
        proto.bump_generation();
    }

    // [[Prototype]] → Object.prototype（幂等，裸写槽）。
    let object_proto_val = JsValue::from_js_object(world.object_proto.as_ptr() as *mut JsObject);
    let _ = proto.set_proto(object_proto_val);

    // 原型方法：postMessage / close（每方法先 lookup_position 守卫）。
    let si_post_message = sf.intern("postMessage").0;
    if sh.lookup_position(proto.shape_id(), si_post_message).is_none() {
        apply_binding_table(
            world,
            proto,
            core,
            &[
                (
                    "postMessage",
                    oxide_builtins::broadcast_channel::broadcast_channel_post_message::<crate::vm::Vm> as *const (),
                    1,
                ),
                (
                    "close",
                    oxide_builtins::broadcast_channel::broadcast_channel_close::<crate::vm::Vm> as *const (),
                    0,
                ),
            ],
        );
    }

    // name 原型访问器（set 恒 undefined），lookup_position 守卫。
    let si_name_proto = sf.intern("name").0;
    if sh.lookup_position(proto.shape_id(), si_name_proto).is_none() {
        bind_accessor_getter(
            core,
            session,
            proto,
            "name",
            oxide_builtins::broadcast_channel::broadcast_channel_name::<crate::vm::Vm> as *const (),
        );
    }

    // readyState 原型访问器（set 恒 undefined），lookup_position 守卫。
    let si_ready_state = sf.intern("readyState").0;
    if sh.lookup_position(proto.shape_id(), si_ready_state).is_none() {
        bind_accessor_getter(
            core,
            session,
            proto,
            "readyState",
            oxide_builtins::broadcast_channel::broadcast_channel_ready_state::<crate::vm::Vm> as *const (),
        );
    }
}
