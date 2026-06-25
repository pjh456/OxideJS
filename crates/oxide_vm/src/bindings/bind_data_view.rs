use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

pub fn bind_data_view(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().data_view_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().data_view_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(
        ctor,
        oxide_builtins::data_view::data_view_constructor::<crate::vm::Vm> as *const (),
        3,
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("getInt8", oxide_builtins::data_view::data_view_get_int8::<crate::vm::Vm> as *const (), 1),
            (
                "getUint8",
                oxide_builtins::data_view::data_view_get_uint8::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getInt16",
                oxide_builtins::data_view::data_view_get_int16::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getUint16",
                oxide_builtins::data_view::data_view_get_uint16::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getInt32",
                oxide_builtins::data_view::data_view_get_int32::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getUint32",
                oxide_builtins::data_view::data_view_get_uint32::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getFloat32",
                oxide_builtins::data_view::data_view_get_float32::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getFloat64",
                oxide_builtins::data_view::data_view_get_float64::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getBigInt64",
                oxide_builtins::data_view::data_view_get_big_int64::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getBigUint64",
                oxide_builtins::data_view::data_view_get_big_uint64::<crate::vm::Vm> as *const (),
                2,
            ),
            ("setInt8", oxide_builtins::data_view::data_view_set_int8::<crate::vm::Vm> as *const (), 2),
            (
                "setUint8",
                oxide_builtins::data_view::data_view_set_uint8::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setInt16",
                oxide_builtins::data_view::data_view_set_int16::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "setUint16",
                oxide_builtins::data_view::data_view_set_uint16::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "setInt32",
                oxide_builtins::data_view::data_view_set_int32::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "setUint32",
                oxide_builtins::data_view::data_view_set_uint32::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "setFloat32",
                oxide_builtins::data_view::data_view_set_float32::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "setFloat64",
                oxide_builtins::data_view::data_view_set_float64::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "setBigInt64",
                oxide_builtins::data_view::data_view_set_big_int64::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "setBigUint64",
                oxide_builtins::data_view::data_view_set_big_uint64::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "toString",
                oxide_builtins::data_view::data_view_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
        ],
    );

    bind_constructor!(core, global, "DataView", ctor_ptr, oxide_builtins::data_view::data_view_constructor::<crate::vm::Vm>, 3, hash: true);
}
