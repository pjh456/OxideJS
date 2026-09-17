use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_accessor_getter, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bind_constructor;

/// 把 Symbol 构造器与原型方法绑定到 global（含 `Symbol.iterator` 等 well-known symbols）。
pub fn bind_symbol(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().symbol_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().symbol_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::symbol::symbol_constructor::<crate::vm::Vm> as *const (), 1);

    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[
            ("for", oxide_builtins::symbol::symbol_for::<crate::vm::Vm> as *const (), 1),
            ("keyFor", oxide_builtins::symbol::symbol_key_for::<crate::vm::Vm> as *const (), 1),
        ],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("toString", oxide_builtins::symbol::symbol_to_string::<crate::vm::Vm> as *const (), 0),
            ("valueOf", oxide_builtins::symbol::symbol_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );

    // description 是访问器 getter（Symbol 原始值成员访问经 Symbol.prototype 链命中）。
    bind_accessor_getter(
        core,
        session,
        proto,
        "description",
        oxide_builtins::symbol::symbol_description_getter::<crate::vm::Vm> as *const (),
    );

    // well-known symbol 以符号原语绑定，下标与内建符号表（0..WELL_KNOWN_SYMBOL_COUNT）
    // 一一对应；键编码由该下标直接推出。
    for (name, id) in [
        ("match", 1u32),
        ("replace", 2),
        ("search", 3),
        ("split", 4),
        ("iterator", 0),
        ("toPrimitive", 5),
        ("hasInstance", 6),
        ("matchAll", 7),
        ("asyncIterator", 8),
        ("toStringTag", 9),
        ("species", 10),
        ("asyncDispose", 11),
        ("dispose", 12),
    ] {
        bind_well_known_symbol(core, ctor, name, JsValue::symbol(id));
    }

    bind_constructor!(core, global, "Symbol", ctor_ptr, oxide_builtins::symbol::symbol_constructor::<crate::vm::Vm>, 1, hash: true);
}

fn bind_well_known_symbol(core: &Arc<KernelCore>, ctor: &mut JsObject, name: &str, val: JsValue) {
    let si = core.perm_interner().intern(name).0;
    let shape_id = core.shape_forge().make_shape(ctor.shape_id(), si);
    ctor.set_shape_id(shape_id);
    ctor.ensure_hash_props().push(val);
    ctor.bump_generation();
}
