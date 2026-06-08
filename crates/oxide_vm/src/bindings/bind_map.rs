use std::sync::Arc;

use crate::bind_constructor_hash;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

pub fn bind_map(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().map_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().map_proto.as_ptr() as *mut JsObject;

    configure_native_constructor(ctor, crate::builtins::map::map_constructor as *const (), 1);
    let proto = unsafe { &mut *proto_ptr };

    apply_binding_table(
        kernel.builtin_world(),
        proto,
        kernel,
        &[
            ("set", crate::builtins::map::map_set as *const (), 2),
            ("get", crate::builtins::map::map_get as *const (), 1),
            ("has", crate::builtins::map::map_has as *const (), 1),
            ("delete", crate::builtins::map::map_delete as *const (), 1),
            ("clear", crate::builtins::map::map_clear as *const (), 0),
            ("size", crate::builtins::map::map_size as *const (), 0),
        ],
    );

    bind_constructor_hash!(kernel, global, "Map", ctor_ptr, crate::builtins::map::map_constructor, 1);
}
