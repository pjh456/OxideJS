use std::sync::Arc;

use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

use crate::bind_constructor;

pub fn bind_regexp(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().regexp_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::regexp::regexp_constructor::<crate::vm::Vm> as *const (), 2);

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("exec", oxide_builtins::regexp::regexp_exec::<crate::vm::Vm> as *const (), 1),
            ("test", oxide_builtins::regexp::regexp_test::<crate::vm::Vm> as *const (), 1),
            ("toString", oxide_builtins::regexp::regexp_to_string::<crate::vm::Vm> as *const (), 0),
        ],
    );

    bind_constructor!(core, global, "RegExp", ctor_ptr, oxide_builtins::regexp::regexp_constructor::<crate::vm::Vm>, 2, hash: true);
}
