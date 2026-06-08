use std::sync::Arc;

use crate::bind_constructor_hash;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

pub fn bind_set(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().set_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().set_proto.as_ptr() as *mut JsObject;

    configure_native_constructor(ctor, crate::builtins::set::set_constructor as *const (), 1);
    let proto = unsafe { &mut *proto_ptr };

    apply_binding_table(
        kernel.builtin_world(),
        proto,
        kernel,
        &[
            ("add", crate::builtins::set::set_add as *const (), 1),
            ("has", crate::builtins::set::set_has as *const (), 1),
            ("delete", crate::builtins::set::set_delete as *const (), 1),
            ("clear", crate::builtins::set::set_clear as *const (), 0),
            ("size", crate::builtins::set::set_size as *const (), 0),
        ],
    );

    bind_constructor_hash!(
        kernel,
        global,
        "Set",
        ctor_ptr,
        crate::builtins::set::set_constructor,
        1
    );
}
