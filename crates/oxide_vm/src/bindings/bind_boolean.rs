use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

pub fn bind_boolean(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().boolean_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().boolean_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, crate::builtins::boolean::boolean_constructor as *const (), 1);

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("valueOf", crate::builtins::boolean::boolean_prototype_value_of as *const (), 0),
            ("toString", crate::builtins::boolean::boolean_prototype_to_string as *const (), 0),
        ],
    );

    bind_constructor!(core, global, "Boolean", ctor_ptr, crate::builtins::boolean::boolean_constructor, 1, hash: true);
}
