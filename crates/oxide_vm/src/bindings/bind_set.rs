use std::sync::Arc;

use crate::bind_constructor_hash;
use oxide_kernel::bind_method;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

pub fn bind_set(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().set_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().set_proto.as_ptr() as *mut JsObject;

    ctor.set_native_fn(Some(crate::builtins::set::set_constructor as *const ()));
    ctor.set_native_arg_count(1);
    let proto = unsafe { &mut *proto_ptr };
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();

    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "add",
        crate::builtins::set::set_add,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "has",
        crate::builtins::set::set_has,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "delete",
        crate::builtins::set::set_delete,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "clear",
        crate::builtins::set::set_clear,
        0
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "size",
        crate::builtins::set::set_size,
        0
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
