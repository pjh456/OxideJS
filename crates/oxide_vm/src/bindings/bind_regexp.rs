use std::sync::Arc;

use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

use crate::bind_constructor_hash;

pub fn bind_regexp(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().regexp_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, crate::builtins::regexp::regexp_constructor as *const (), 2);

    apply_binding_table(
        kernel.builtin_world(),
        proto,
        kernel,
        &[
            ("exec", crate::builtins::regexp::regexp_exec as *const (), 1),
            ("test", crate::builtins::regexp::regexp_test as *const (), 1),
            ("toString", crate::builtins::regexp::regexp_to_string as *const (), 0),
        ],
    );

    bind_constructor_hash!(kernel, global, "RegExp", ctor_ptr, crate::builtins::regexp::regexp_constructor, 2);
}
