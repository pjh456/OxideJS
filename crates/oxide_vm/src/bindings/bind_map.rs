use std::sync::Arc;

use crate::bind_constructor_hash;
use oxide_kernel::bind_method;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

pub fn bind_map(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().map_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().map_proto.as_ptr() as *mut JsObject;

    ctor.set_native_fn(Some(crate::builtins::map::map_constructor as *const ()));
    ctor.set_native_arg_count(1);
    let proto = unsafe { &mut *proto_ptr };
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();

    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "set",
        crate::builtins::map::map_set,
        2
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "get",
        crate::builtins::map::map_get,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "has",
        crate::builtins::map::map_has,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "delete",
        crate::builtins::map::map_delete,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "clear",
        crate::builtins::map::map_clear,
        0
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "size",
        crate::builtins::map::map_size,
        0
    );

    bind_constructor_hash!(
        kernel,
        global,
        "Map",
        ctor_ptr,
        crate::builtins::map::map_constructor,
        1
    );
}
