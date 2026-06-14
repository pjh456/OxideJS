use std::sync::Arc;

use crate::bind_constructor_hash;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

pub fn bind_array_buffer(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().array_buffer_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().array_buffer_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, crate::builtins::array_buffer::array_buffer_constructor as *const (), 1);

    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[("isView", crate::builtins::array_buffer::array_buffer_is_view as *const (), 1)],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("byteLength", crate::builtins::array_buffer::array_buffer_byte_length as *const (), 0),
            ("slice", crate::builtins::array_buffer::array_buffer_slice as *const (), 2),
            ("toString", crate::builtins::array_buffer::array_buffer_to_string as *const (), 0),
        ],
    );

    bind_constructor_hash!(
        core,
        global,
        "ArrayBuffer",
        ctor_ptr,
        crate::builtins::array_buffer::array_buffer_constructor,
        1
    );
}
