use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

pub fn bind_map(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().map_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().map_proto.as_ptr() as *mut JsObject;

    configure_native_constructor(ctor, oxide_builtins::map::map_constructor::<crate::vm::Vm> as *const (), 1);
    let proto = unsafe { &mut *proto_ptr };

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("set", oxide_builtins::map::map_set::<crate::vm::Vm> as *const (), 2),
            ("get", oxide_builtins::map::map_get::<crate::vm::Vm> as *const (), 1),
            ("has", oxide_builtins::map::map_has::<crate::vm::Vm> as *const (), 1),
            ("delete", oxide_builtins::map::map_delete::<crate::vm::Vm> as *const (), 1),
            ("clear", oxide_builtins::map::map_clear::<crate::vm::Vm> as *const (), 0),
            ("size", oxide_builtins::map::map_size::<crate::vm::Vm> as *const (), 0),
            ("entries", oxide_builtins::map::map_entries::<crate::vm::Vm> as *const (), 0),
            ("values", oxide_builtins::map::map_values::<crate::vm::Vm> as *const (), 0),
            ("keys", oxide_builtins::map::map_keys::<crate::vm::Vm> as *const (), 0),
        ],
    );

    bind_constructor!(core, global, "Map", ctor_ptr, oxide_builtins::map::map_constructor::<crate::vm::Vm>, 1, hash: true);
}
