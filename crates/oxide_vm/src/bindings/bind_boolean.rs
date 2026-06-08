use std::sync::Arc;

use crate::bind_constructor_hash;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

pub fn bind_boolean(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().boolean_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().boolean_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(
        ctor,
        crate::builtins::boolean::boolean_constructor as *const (),
        1,
    );

    apply_binding_table(
        kernel.builtin_world(),
        proto,
        kernel,
        &[
            (
                "valueOf",
                crate::builtins::boolean::boolean_prototype_value_of as *const (),
                0,
            ),
            (
                "toString",
                crate::builtins::boolean::boolean_prototype_to_string as *const (),
                0,
            ),
        ],
    );

    bind_constructor_hash!(
        kernel,
        global,
        "Boolean",
        ctor_ptr,
        crate::builtins::boolean::boolean_constructor,
        1
    );
}
