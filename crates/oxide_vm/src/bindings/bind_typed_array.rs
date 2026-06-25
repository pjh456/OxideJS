use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

macro_rules! bind_typed_array_constructor {
    ($core:expr, $global:expr, $name:literal, $ctor_ptr:expr, $ctor_fn:path) => {{
        let ctor = unsafe { &mut *$ctor_ptr };
        configure_native_constructor(ctor, ($ctor_fn as fn(&mut $crate::vm::Vm, &[u8]) -> oxide_runtime_api::NativeResult) as *const (), 1);
        bind_constructor!($core, $global, $name, $ctor_ptr, $ctor_fn, 1, hash: true);
    }};
}

pub fn bind_typed_array(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let shared_proto_ptr = session.builtin_world().typed_array_proto.as_ptr() as *mut JsObject;
    let shared_proto = unsafe { &mut *shared_proto_ptr };
    apply_binding_table(
        session.builtin_world(),
        shared_proto,
        core,
        &[
            ("at", oxide_builtins::typed_array::typed_array_at::<crate::vm::Vm> as *const (), 1),
            ("fill", oxide_builtins::typed_array::typed_array_fill::<crate::vm::Vm> as *const (), 3),
            ("set", oxide_builtins::typed_array::typed_array_set::<crate::vm::Vm> as *const (), 2),
            ("slice", oxide_builtins::typed_array::typed_array_slice::<crate::vm::Vm> as *const (), 2),
            (
                "subarray",
                oxide_builtins::typed_array::typed_array_subarray::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "toString",
                oxide_builtins::typed_array::typed_array_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
        ],
    );

    bind_typed_array_constructor!(
        core,
        global,
        "Int8Array",
        session.builtin_world().int8array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::int8array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint8Array",
        session.builtin_world().uint8array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::uint8array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint8ClampedArray",
        session.builtin_world().uint8clampedarray_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::uint8clampedarray_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Int16Array",
        session.builtin_world().int16array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::int16array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint16Array",
        session.builtin_world().uint16array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::uint16array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Int32Array",
        session.builtin_world().int32array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::int32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint32Array",
        session.builtin_world().uint32array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::uint32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Float32Array",
        session.builtin_world().float32array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::float32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Float64Array",
        session.builtin_world().float64array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::float64array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "BigInt64Array",
        session.builtin_world().bigint64array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::bigint64array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "BigUint64Array",
        session.builtin_world().biguint64array_constructor.as_ptr() as *mut JsObject,
        oxide_builtins::typed_array::biguint64array_constructor
    );
}
