use std::sync::Arc;

use crate::bind_constructor_hash;
use oxide_kernel::bind_method;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

pub fn bind_boolean(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().boolean_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().boolean_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();

    ctor.set_native_fn(Some(
        crate::builtins::boolean::boolean_constructor as *const (),
    ));
    ctor.set_native_arg_count(1);

    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "valueOf",
        crate::builtins::boolean::boolean_prototype_value_of,
        0
    );
    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "toString",
        crate::builtins::boolean::boolean_prototype_to_string,
        0
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
