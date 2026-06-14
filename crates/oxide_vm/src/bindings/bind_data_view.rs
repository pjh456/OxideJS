use std::sync::Arc;

use crate::bind_constructor_hash;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

pub fn bind_data_view(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().data_view_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().data_view_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, crate::builtins::data_view::data_view_constructor as *const (), 3);

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("getInt8", crate::builtins::data_view::data_view_get_int8 as *const (), 1),
            ("getUint8", crate::builtins::data_view::data_view_get_uint8 as *const (), 1),
            ("getInt16", crate::builtins::data_view::data_view_get_int16 as *const (), 2),
            ("getUint16", crate::builtins::data_view::data_view_get_uint16 as *const (), 2),
            ("getInt32", crate::builtins::data_view::data_view_get_int32 as *const (), 2),
            ("getUint32", crate::builtins::data_view::data_view_get_uint32 as *const (), 2),
            ("getFloat32", crate::builtins::data_view::data_view_get_float32 as *const (), 2),
            ("getFloat64", crate::builtins::data_view::data_view_get_float64 as *const (), 2),
            ("getBigInt64", crate::builtins::data_view::data_view_get_big_int64 as *const (), 2),
            ("getBigUint64", crate::builtins::data_view::data_view_get_big_uint64 as *const (), 2),
            ("setInt8", crate::builtins::data_view::data_view_set_int8 as *const (), 2),
            ("setUint8", crate::builtins::data_view::data_view_set_uint8 as *const (), 2),
            ("setInt16", crate::builtins::data_view::data_view_set_int16 as *const (), 3),
            ("setUint16", crate::builtins::data_view::data_view_set_uint16 as *const (), 3),
            ("setInt32", crate::builtins::data_view::data_view_set_int32 as *const (), 3),
            ("setUint32", crate::builtins::data_view::data_view_set_uint32 as *const (), 3),
            ("setFloat32", crate::builtins::data_view::data_view_set_float32 as *const (), 3),
            ("setFloat64", crate::builtins::data_view::data_view_set_float64 as *const (), 3),
            ("setBigInt64", crate::builtins::data_view::data_view_set_big_int64 as *const (), 3),
            ("setBigUint64", crate::builtins::data_view::data_view_set_big_uint64 as *const (), 3),
            ("toString", crate::builtins::data_view::data_view_to_string as *const (), 0),
        ],
    );

    bind_constructor_hash!(core, global, "DataView", ctor_ptr, crate::builtins::data_view::data_view_constructor, 3);
}
