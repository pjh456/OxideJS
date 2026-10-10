//! Worker 构造器绑定：Box 分配构造器（不占 `BuiltinWorld` P 字段、不新增
//! `BuiltinId`、不占 `BuiltinDirtySet` 脏位）加 `track_leaked_object` 加填原型
//! 占位加全局槽，全槽 `lookup_position` 幂等守卫。
//!
//! 与 `bind_iterator_global` 同型：构造器经 `Box::into_raw` 登记 world 释放
//! 登记表（session 收尾统一释放），全局 `Worker` 槽经 `bind_constructor!` 原位
//! 安装（既有槽更新槽值、无槽新开）。

use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 Worker 构造器（Box 分配）与 self 面方法绑定到 global。
///
/// # 幂等
/// 本函数从三条路径到达：`init_kernel_builtins`（初始）、`rebind_dirty_builtins`
/// 的 `worker` 分支（无条件，构造器不在脏体系内）、global-dirty 路径
/// （`bind_global_builtin_slots`）。构造器本体经 `lookup_position` 守卫复用（有
/// 槽即不新建）；原型槽位经 `lookup_position` 守卫（有槽即跳过）；
/// `bind_constructor!` 的全局槽臂自带既有槽原位更新。
///
/// # 副作用
/// - 构造器与原型对象经 `Box::into_raw` 登记 world 释放登记表（session 收尾统一
///   释放）；全局 `Worker` 槽经 `bind_constructor!` 原位安装。
/// - self 面方法（`postMessage`/`close` 方法、`name`/`location` getter）绑 global
///   （worker realm 经 `self` === global 可达，主 realm 无 `self` 时为无害占位）。
///
/// `realm_id` 未使用（Worker 无 well-known 符号键），保留签名与其他绑定函数一致。
pub fn bind_worker(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject, _realm_id: u32) {
    let world = session.builtin_world();
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();

    // ---- 构造器本体（Box 分配，不占 P 字段）----
    // 幂等：全局 `Worker` 槽已有构造器时复用（不新建）。
    let si_worker = sf.intern("Worker").0;
    let ctor_ptr: *mut JsObject = match sh.lookup_position(global.shape_id(), si_worker) {
        Some(pos) if global.get_prop_at(pos).is_object() => global.get_prop_at(pos).as_js_object_ptr(),
        _ => {
            let function_proto = world.function_proto.as_ptr() as *mut JsObject;
            let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto));
            ctor.set_function(true);
            configure_native_constructor(&mut ctor, crate::worker::bindings::worker_constructor as *const (), 1);
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
                .push(JsValue::perm_string(sf.string_ptr(sf.intern("Worker").0)));
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
                // Worker.prototype 的 [[Prototype]] 指向 EventTarget.prototype
                // （事件三方法经原型链可达；EventTarget 无构造器，仅暴露原型）。
                let et_proto = world.event_target_proto.as_ptr() as *mut JsObject;
                let proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(et_proto));
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

        // postMessage / terminate 原型方法（幂等守卫）。
        let si_post_message = sf.intern("postMessage").0;
        if sh.lookup_position(proto.shape_id(), si_post_message).is_none() {
            apply_binding_table(
                world,
                proto,
                core,
                &[
                    ("postMessage", crate::worker::bindings::worker_post_message as *const (), 1),
                    ("terminate", crate::worker::bindings::worker_terminate as *const (), 0),
                ],
            );
        }
    }

    // 全局 `Worker` 槽（bind_constructor! 自带既有槽原位更新、无槽新开）。
    bind_constructor!(
        core,
        global,
        "Worker",
        ctor_ptr,
        crate::worker::bindings::worker_constructor,
        1,
        hash: true
    );

    // ---- self 面（绑 global，worker realm 经 self === global 可达）----
    // postMessage / close 方法（幂等守卫）。
    let si_self_post = sf.intern("postMessage").0;
    if sh.lookup_position(global.shape_id(), si_self_post).is_none() {
        apply_binding_table(
            world,
            global,
            core,
            &[
                ("postMessage", crate::worker::bindings::self_post_message as *const (), 1),
                ("close", crate::worker::bindings::self_close as *const (), 0),
            ],
        );
    }
    // EventTarget 三方法直绑全局（worker realm 经 self === global 可达；全局原型链
    // 是 Object.prototype，首版不改全局原型，三方法直绑全局等价可见）。幂等守卫。
    let si_self_add = sf.intern("addEventListener").0;
    if sh.lookup_position(global.shape_id(), si_self_add).is_none() {
        apply_binding_table(
            world,
            global,
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
    // self 全局属性：值为 global 对象本身（worker realm 经 self === global 可达，
    // self.onmessage / self.postMessage 由此成立）。幂等守卫。
    let si_self = sf.intern("self").0;
    if sh.lookup_position(global.shape_id(), si_self).is_none() {
        let global_ptr = global as *mut JsObject;
        let shape = sh.make_shape(global.shape_id(), si_self);
        global.set_shape_id(shape);
        global.ensure_hash_props().push(JsValue::from_js_object(global_ptr));
        let pos = global.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        global.set_data_meta(pos, PropAttributes::new(true, false, true));
        global.bump_generation();
    }
    // name / location getter（幂等守卫）。
    // let si_name_global = sf.intern("name").0;
    // if sh.lookup_position(global.shape_id(), si_name_global).is_none() {
    //     bind_accessor_getter(core, session, global, "name", crate::worker::bindings::self_name as *const ());
    // }
    // let si_location = sf.intern("location").0;
    // if sh.lookup_position(global.shape_id(), si_location).is_none() {
    //     bind_accessor_getter(
    //         core,
    //         session,
    //         global,
    //         "location",
    //         crate::worker::bindings::self_location as *const (),
    //     );
    // }
}
