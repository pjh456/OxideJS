use std::sync::Arc;

use oxide_kernel::bind_methods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

use crate::bind_constructor_hash;

pub fn bind_regexp(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().regexp_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();

    ctor.set_native_fn(Some(
        crate::builtins::regexp::regexp_constructor as *const (),
    ));
    ctor.set_native_arg_count(2);

    bind_methods!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        ("exec", crate::builtins::regexp::regexp_exec, 1),
        ("test", crate::builtins::regexp::regexp_test, 1),
        ("toString", crate::builtins::regexp::regexp_to_string, 0),
    );

    bind_constructor_hash!(
        kernel,
        global,
        "RegExp",
        ctor_ptr,
        crate::builtins::regexp::regexp_constructor,
        2
    );
}
