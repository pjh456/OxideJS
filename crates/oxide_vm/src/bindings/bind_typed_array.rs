use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 给构造器与对应原型设置 `BYTES_PER_ELEMENT`（只读、不可枚举、不可配置）。
///
/// # 步骤
/// 1. 构造器走 hash_props 存储（与 `bind_constructor` 的 hash 路径一致）。
/// 2. 原型走 shape 属性存储（与 `bind_method`/`bind_accessor_getter` 一致）。
fn set_bypes_per_element(core: &Arc<KernelCore>, ctor_ptr: *mut JsObject, proto_ptr: *mut JsObject, bpe: u32) {
    let bpe_si = core.perm_interner().intern("BYTES_PER_ELEMENT").0;
    let bpe_val = JsValue::int(bpe as i32);
    let attrs = PropAttributes::new(false, false, false);

    let ctor = unsafe { &mut *ctor_ptr };
    ctor.ensure_hash_props().push(bpe_val);
    let ctor_pos = ctor.ensure_hash_props().len() - 1;
    let ctor_shape = core.shape_forge().make_shape(ctor.shape_id(), bpe_si);
    ctor.set_shape_id(ctor_shape);
    ctor.set_data_meta(ctor_pos as u32, attrs);

    let proto = unsafe { &mut *proto_ptr };
    let proto_shape = core.shape_forge().make_shape(proto.shape_id(), bpe_si);
    proto.set_shape_id(proto_shape);
    let proto_pos = proto.push_prop(bpe_val);
    proto.set_data_meta(proto_pos as u32, attrs);
    proto.bump_generation();
}

macro_rules! bind_typed_array_constructor {
    ($core:expr, $global:expr, $name:literal, $ctor_ptr:expr, $proto_ptr:expr, $bpe:expr, $ctor_fn:path) => {{
        let ctor = unsafe { &mut *$ctor_ptr };
        configure_native_constructor(ctor, ($ctor_fn as fn(&mut $crate::vm::Vm, &[u8]) -> oxide_runtime_api::NativeResult) as *const (), 1);
        bind_constructor!($core, $global, $name, $ctor_ptr, $ctor_fn, 1, hash: true);
        set_bypes_per_element($core, $ctor_ptr, $proto_ptr, $bpe);
    }};
}

/// 把所有 TypedArray 家族构造器（Int8Array/Uint8Array/.../BigUint64Array）与其共享原型绑定到 global。
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
        session.builtin_world().int8array_proto.as_ptr() as *mut JsObject,
        1,
        oxide_builtins::typed_array::int8array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint8Array",
        session.builtin_world().uint8array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().uint8array_proto.as_ptr() as *mut JsObject,
        1,
        oxide_builtins::typed_array::uint8array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint8ClampedArray",
        session.builtin_world().uint8clampedarray_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().uint8clampedarray_proto.as_ptr() as *mut JsObject,
        1,
        oxide_builtins::typed_array::uint8clampedarray_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Int16Array",
        session.builtin_world().int16array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().int16array_proto.as_ptr() as *mut JsObject,
        2,
        oxide_builtins::typed_array::int16array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint16Array",
        session.builtin_world().uint16array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().uint16array_proto.as_ptr() as *mut JsObject,
        2,
        oxide_builtins::typed_array::uint16array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Int32Array",
        session.builtin_world().int32array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().int32array_proto.as_ptr() as *mut JsObject,
        4,
        oxide_builtins::typed_array::int32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Uint32Array",
        session.builtin_world().uint32array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().uint32array_proto.as_ptr() as *mut JsObject,
        4,
        oxide_builtins::typed_array::uint32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Float32Array",
        session.builtin_world().float32array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().float32array_proto.as_ptr() as *mut JsObject,
        4,
        oxide_builtins::typed_array::float32array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "Float64Array",
        session.builtin_world().float64array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().float64array_proto.as_ptr() as *mut JsObject,
        8,
        oxide_builtins::typed_array::float64array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "BigInt64Array",
        session.builtin_world().bigint64array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().bigint64array_proto.as_ptr() as *mut JsObject,
        8,
        oxide_builtins::typed_array::bigint64array_constructor
    );
    bind_typed_array_constructor!(
        core,
        global,
        "BigUint64Array",
        session.builtin_world().biguint64array_constructor.as_ptr() as *mut JsObject,
        session.builtin_world().biguint64array_proto.as_ptr() as *mut JsObject,
        8,
        oxide_builtins::typed_array::biguint64array_constructor
    );
}
