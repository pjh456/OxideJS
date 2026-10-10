//! Event 基类绑定：就地填充 `event_constructor` / `event_proto` 两枚 P 字段
//! 占位（构造器本体、原型方法、原型 getter 与 Event 常量），并绑全局 `Event`。
//!
//! 九枚 P 字段中其余七枚（MessageEvent / ErrorEvent / CustomEvent 四对加
//! EventTarget 原型）由后续子任务就地填充，本函数不读不写。

use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, bind_accessor_getter, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 Event 构造器与原型方法绑定到 global。
///
/// `event_constructor` / `event_proto` 两枚 P 字段是空对象占位，本函数就地
/// 填充（不新建对象——P 字段本身就是槽位，快照/脏检查/收尾枚举已覆盖）。
///
/// # 幂等
/// 本函数从三条路径到达：`init_kernel_builtins`（占位是空对象）、
/// `rebind_dirty_builtins` 的 `event` 分支（占位是重建后的空对象）、global-dirty
/// 路径（占位可能已填满）。全部槽写经 `lookup_position` 前置守卫，有槽即跳过；
/// `bind_constructor!` 的全局槽臂自带既有槽原位更新，天然幂等。
///
/// `realm_id` 未使用（Event 无 well-known 符号键），保留签名与其他绑定函数一致。
pub fn bind_event(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject, _realm_id: u32) {
    let world = session.builtin_world();
    let ctor_ptr = world.event_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = world.event_proto.as_ptr() as *mut JsObject;
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

    // name 槽 → "Event"（描述符 {f,f,t}）。
    let si_name = sf.intern("name").0;
    if sh.lookup_position(ctor.shape_id(), si_name).is_none() {
        let shape = sh.make_shape(ctor.shape_id(), si_name);
        ctor.set_shape_id(shape);
        ctor.ensure_hash_props()
            .push(JsValue::perm_string(sf.string_ptr(sf.intern("Event").0)));
        let pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        ctor.set_data_meta(pos, PropAttributes::new(false, false, true));
        ctor.bump_generation();
    }

    // [[Prototype]] → Function.prototype（幂等，裸写槽）。
    let fn_proto_val = JsValue::from_js_object(world.function_proto.as_ptr() as *mut JsObject);
    let _ = ctor.set_proto(fn_proto_val);

    configure_native_constructor(ctor, oxide_builtins::event::event_constructor::<crate::vm::Vm> as *const (), 2);
    bind_constructor!(
        core,
        global,
        "Event",
        ctor_ptr,
        oxide_builtins::event::event_constructor::<crate::vm::Vm>,
        2,
        hash: true
    );

    // Event 常量：CAPTURING_PHASE / AT_TARGET / BUBBLING_PHASE（绑构造器，
    // 描述符 {t,f,t}）。
    let constants: [(&str, i32); 3] = [("CAPTURING_PHASE", 1), ("AT_TARGET", 2), ("BUBBLING_PHASE", 3)];
    for (name, value) in constants {
        let si = sf.intern(name).0;
        if sh.lookup_position(ctor.shape_id(), si).is_none() {
            let shape = sh.make_shape(ctor.shape_id(), si);
            ctor.set_shape_id(shape);
            let pos = ctor.push_prop(JsValue::int(value));
            ctor.set_data_meta(pos, PropAttributes::new(true, false, true));
            ctor.bump_generation();
        }
    }

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

    // 原型方法：preventDefault / stopPropagation / stopImmediatePropagation /
    // composedPath（每方法先 lookup_position 守卫）。
    let si_prevent_default = sf.intern("preventDefault").0;
    if sh.lookup_position(proto.shape_id(), si_prevent_default).is_none() {
        apply_binding_table(
            world,
            proto,
            core,
            &[
                (
                    "preventDefault",
                    oxide_builtins::event::event_prevent_default::<crate::vm::Vm> as *const (),
                    0,
                ),
                (
                    "stopPropagation",
                    oxide_builtins::event::event_stop_propagation::<crate::vm::Vm> as *const (),
                    0,
                ),
                (
                    "stopImmediatePropagation",
                    oxide_builtins::event::event_stop_immediate_propagation::<crate::vm::Vm> as *const (),
                    0,
                ),
                (
                    "composedPath",
                    oxide_builtins::event::event_composed_path::<crate::vm::Vm> as *const (),
                    0,
                ),
            ],
        );
    }

    // 原型 getter：type / target / currentTarget / eventPhase / isTrusted /
    // bubbles / cancelable / defaultPrevented / composed（每 getter 先
    // lookup_position 守卫）。
    let getter_fns: [(&str, *const ()); 9] = [
        ("type", oxide_builtins::event::event_type_getter::<crate::vm::Vm> as *const ()),
        ("target", oxide_builtins::event::event_target_getter::<crate::vm::Vm> as *const ()),
        (
            "currentTarget",
            oxide_builtins::event::event_current_target_getter::<crate::vm::Vm> as *const (),
        ),
        ("eventPhase", oxide_builtins::event::event_phase_getter::<crate::vm::Vm> as *const ()),
        ("isTrusted", oxide_builtins::event::event_is_trusted_getter::<crate::vm::Vm> as *const ()),
        ("bubbles", oxide_builtins::event::event_bubbles_getter::<crate::vm::Vm> as *const ()),
        ("cancelable", oxide_builtins::event::event_cancelable_getter::<crate::vm::Vm> as *const ()),
        (
            "defaultPrevented",
            oxide_builtins::event::event_default_prevented_getter::<crate::vm::Vm> as *const (),
        ),
        ("composed", oxide_builtins::event::event_composed_getter::<crate::vm::Vm> as *const ()),
    ];
    for (name, fn_ptr) in getter_fns {
        let si = sf.intern(name).0;
        if sh.lookup_position(proto.shape_id(), si).is_none() {
            bind_accessor_getter(core, session, proto, name, fn_ptr);
        }
    }

    // ---- 填 EventTarget 原型占位（无构造器，仅暴露原型与三方法）----
    // 幂等：三方法经 lookup_position 守卫，[[Prototype]] 裸写槽幂等。
    let et_proto_ptr = world.event_target_proto.as_ptr() as *mut JsObject;
    let et_proto = unsafe { &mut *et_proto_ptr };
    // [[Prototype]] → Object.prototype（幂等，裸写槽）。
    let object_proto_val = JsValue::from_js_object(world.object_proto.as_ptr() as *mut JsObject);
    let _ = et_proto.set_proto(object_proto_val);
    // 三方法：addEventListener / removeEventListener / dispatchEvent（每方法先
    // lookup_position 守卫）。
    let si_add = sf.intern("addEventListener").0;
    if sh.lookup_position(et_proto.shape_id(), si_add).is_none() {
        apply_binding_table(
            world,
            et_proto,
            core,
            &[
                (
                    "addEventListener",
                    oxide_builtins::event_target::event_target_add_event_listener::<crate::vm::Vm> as *const (),
                    3,
                ),
                (
                    "removeEventListener",
                    oxide_builtins::event_target::event_target_remove_event_listener::<crate::vm::Vm> as *const (),
                    3,
                ),
                (
                    "dispatchEvent",
                    oxide_builtins::event_target::event_target_dispatch_event::<crate::vm::Vm> as *const (),
                    1,
                ),
            ],
        );
    }
}
