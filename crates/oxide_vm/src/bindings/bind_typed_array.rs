use std::sync::Arc;

use crate::bind_constructor_hash;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

macro_rules! bind_typed_array_constructor {
    ($core:expr, $global:expr, $name:literal, $ctor_ptr:expr, $ctor_fn:path) => {{
        let ctor = unsafe { &mut *$ctor_ptr };
        configure_native_constructor(ctor, $ctor_fn as *const (), 1);
        bind_constructor_hash!($core, $global, $name, $ctor_ptr, $ctor_fn, 1);
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
            ("at", crate::builtins::typed_array::typed_array_at as *const (), 1),
            ("fill", crate::builtins::typed_array::typed_array_fill as *const (), 3),
            ("set", crate::builtins::typed_array::typed_array_set as *const (), 2),
            ("slice", crate::builtins::typed_array::typed_array_slice as *const (), 2),
            ("subarray", crate::builtins::typed_array::typed_array_subarray as *const (), 2),
            ("toString", crate::builtins::typed_array::typed_array_to_string as *const (), 0),
        ],
    );

    bind_typed_array_constructor!(
        core,
        global,
        "Int8Array",
        session.builtin_world().int8array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::int8array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint8Array",
        session.builtin_world().uint8array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::uint8array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint8ClampedArray",
        session.builtin_world().uint8clampedarray_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::uint8clampedarray_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Int16Array",
        session.builtin_world().int16array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::int16array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint16Array",
        session.builtin_world().uint16array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::uint16array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Int32Array",
        session.builtin_world().int32array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::int32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint32Array",
        session.builtin_world().uint32array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::uint32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Float32Array",
        session.builtin_world().float32array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::float32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Float64Array",
        session.builtin_world().float64array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::float64array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "BigInt64Array",
        session.builtin_world().bigint64array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::bigint64array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "BigUint64Array",
        session.builtin_world().biguint64array_constructor.as_ptr() as *mut JsObject,
        crate::builtins::typed_array::biguint64array_constructor
    );
}
