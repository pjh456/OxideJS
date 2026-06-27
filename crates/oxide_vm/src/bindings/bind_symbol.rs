use std::sync::Arc;

use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bind_constructor;

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
        &[("toString", oxide_builtins::symbol::symbol_to_string::<crate::vm::Vm> as *const (), 0)],
    );

    for (name, val) in [
        (
            "match",
            JsValue::from_js_object(session.builtin_world().sym_match.as_ptr() as *mut JsObject),
        ),
        (
            "replace",
            JsValue::from_js_object(session.builtin_world().sym_replace.as_ptr() as *mut JsObject),
        ),
        (
            "search",
            JsValue::from_js_object(session.builtin_world().sym_search.as_ptr() as *mut JsObject),
        ),
        (
            "split",
            JsValue::from_js_object(session.builtin_world().sym_split.as_ptr() as *mut JsObject),
        ),
        (
            "iterator",
            JsValue::from_js_object(session.builtin_world().sym_iterator.as_ptr() as *mut JsObject),
        ),
        (
            "toPrimitive",
            JsValue::from_js_object(session.builtin_world().sym_to_primitive.as_ptr() as *mut JsObject),
        ),
    ] {
        bind_well_known_symbol(core, ctor, name, val);
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
