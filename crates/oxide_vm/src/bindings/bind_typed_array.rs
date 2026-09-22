use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_accessor_getter_key, bind_global_value, bind_method_alias,
    bind_well_known_method_alias, configure_native_constructor,
};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 `%TypedArray%` 抽象构造器的 native 实现配置为恒抛 TypeError（不可 new/不可调用），
/// 设置 `length` 属性为 0，并挂载静态 `of`/`from`（具体构造器经原型链继承）。
fn bind_typed_array_abstract_ctor(core: &Arc<KernelCore>, session: &KernelSession) {
    let ctor_ptr = session.builtin_world().typed_array_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    configure_native_constructor(
        ctor,
        oxide_builtins::typed_array::typed_array_abstract_constructor::<crate::vm::Vm> as *const (),
        0,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(ctor.shape_id(), length_si);
    ctor.set_shape_id(length_shape);
    ctor.ensure_hash_props().push(JsValue::int(0));
    let pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(pos, PropAttributes::new(false, false, true));

    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();
    let world = session.builtin_world();
    let ctor_ptr = world.typed_array_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    // SAFETY: of/from 是转成 *const () 的 NativeFn 函数项指针。
    let _ = world.bind_method(
        ctor,
        sh,
        sf,
        "of",
        unsafe {
            oxide_types::object::NativeFnPtr::from_raw(
                oxide_builtins::typed_array::typed_array_of::<crate::vm::Vm> as *const (),
            )
        },
        0,
    );
    let _ = world.bind_method(
        ctor,
        sh,
        sf,
        "from",
        unsafe {
            oxide_types::object::NativeFnPtr::from_raw(
                oxide_builtins::typed_array::typed_array_from::<crate::vm::Vm> as *const (),
            )
        },
        1,
    );
}

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
    proto.set_data_meta(proto_pos, attrs);
    proto.bump_generation();
}

macro_rules! bind_typed_array_constructor {
    ($core:expr, $global:expr, $name:literal, $ctor_ptr:expr, $proto_ptr:expr, $bpe:expr, $ctor_fn:path) => {{
        let ctor = unsafe { &mut *$ctor_ptr };
        configure_native_constructor(ctor, ($ctor_fn as fn(&mut $crate::vm::Vm, &[u8]) -> oxide_runtime_api::NativeResult) as *const (), 3);
        bind_constructor!($core, $global, $name, $ctor_ptr, $ctor_fn, 3, hash: true);
        // Function.length 描述符：不可写、不可枚举、可配置。
        let len_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        ctor.set_data_meta(len_pos, PropAttributes::new(false, false, true));
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
            ("map", oxide_builtins::typed_array::typed_array_map::<crate::vm::Vm> as *const (), 1),
            ("filter", oxide_builtins::typed_array::typed_array_filter::<crate::vm::Vm> as *const (), 1),
            ("reduce", oxide_builtins::typed_array::typed_array_reduce::<crate::vm::Vm> as *const (), 1),
            (
                "reduceRight",
                oxide_builtins::typed_array::typed_array_reduce_right::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "forEach",
                oxide_builtins::typed_array::typed_array_for_each::<crate::vm::Vm> as *const (),
                1,
            ),
            ("every", oxide_builtins::typed_array::typed_array_every::<crate::vm::Vm> as *const (), 1),
            ("some", oxide_builtins::typed_array::typed_array_some::<crate::vm::Vm> as *const (), 1),
            ("find", oxide_builtins::typed_array::typed_array_find::<crate::vm::Vm> as *const (), 1),
            (
                "findIndex",
                oxide_builtins::typed_array::typed_array_find_index::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "findLast",
                oxide_builtins::typed_array::typed_array_find_last::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "findLastIndex",
                oxide_builtins::typed_array::typed_array_find_last_index::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "indexOf",
                oxide_builtins::typed_array::typed_array_index_of::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "lastIndexOf",
                oxide_builtins::typed_array::typed_array_last_index_of::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "includes",
                oxide_builtins::typed_array::typed_array_includes::<crate::vm::Vm> as *const (),
                1,
            ),
            ("join", oxide_builtins::typed_array::typed_array_join::<crate::vm::Vm> as *const (), 1),
            ("values", oxide_builtins::typed_array::typed_array_values::<crate::vm::Vm> as *const (), 0),
            ("keys", oxide_builtins::typed_array::typed_array_keys::<crate::vm::Vm> as *const (), 0),
            (
                "entries",
                oxide_builtins::typed_array::typed_array_entries::<crate::vm::Vm> as *const (),
                0,
            ),
            ("sort", oxide_builtins::typed_array::typed_array_sort::<crate::vm::Vm> as *const (), 1),
            (
                "reverse",
                oxide_builtins::typed_array::typed_array_reverse::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "copyWithin",
                oxide_builtins::typed_array::typed_array_copy_within::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "toLocaleString",
                oxide_builtins::typed_array::typed_array_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toReversed",
                oxide_builtins::typed_array::typed_array_to_reversed::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toSorted",
                oxide_builtins::typed_array::typed_array_to_sorted::<crate::vm::Vm> as *const (),
                1,
            ),
            ("with", oxide_builtins::typed_array::typed_array_with::<crate::vm::Vm> as *const (), 2),
        ],
    );
    bind_method_alias(core, shared_proto, "values", "keys");
    bind_well_known_method_alias(core, shared_proto, "values", 0);

    // 原型访问器：视图属性（buffer/byteOffset/byteLength/length）读内部数据槽，
    // @@toStringTag 返回具体类型名，供 Object.prototype.toString 区分类型。
    bind_accessor_getter(
        core,
        session,
        shared_proto,
        "buffer",
        oxide_builtins::typed_array::typed_array_buffer_getter::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        shared_proto,
        "byteOffset",
        oxide_builtins::typed_array::typed_array_byte_offset_getter::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        shared_proto,
        "byteLength",
        oxide_builtins::typed_array::typed_array_byte_length_getter::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        shared_proto,
        "length",
        oxide_builtins::typed_array::typed_array_length_getter::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter_key(
        core,
        session,
        shared_proto,
        oxide_types::private_key::make_well_known_symbol_key(oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG),
        "get @@toStringTag",
        oxide_builtins::typed_array::typed_array_to_string_tag_getter::<crate::vm::Vm> as *const (),
    );

    // 共享原型的 own toString 与 Array.prototype.toString 共享同一函数对象
    // （规范：同一内置函数对象；身份与描述符均按 own 属性验收）。
    let to_string_si = core.perm_interner().intern("toString").0;
    let array_proto_ref = unsafe { &*session.builtin_world().array_proto.as_ptr() };
    // SAFETY: array_proto 由 BuiltinWorld 构造期建立，session 存活期内恒有效。
    if let Some(pos) = core.shape_forge().lookup_position(array_proto_ref.shape_id(), to_string_si) {
        let value = array_proto_ref.get_prop_at(pos);
        let new_shape = core.shape_forge().make_shape(shared_proto.shape_id(), to_string_si);
        shared_proto.set_shape_id(new_shape);
        let new_pos = shared_proto.push_prop(value);
        shared_proto.set_data_meta(new_pos, PropAttributes::new(true, false, true));
        shared_proto.bump_generation();
    }

    // base64/hex 四原型法挂 Uint8Array 专属原型（Uint8ClampedArray 经链继承、
    // kind 校验拒绝）；两静态法挂 Uint8Array 构造器。
    let world = session.builtin_world();
    let sh = core.shape_forge().as_ref();
    let sf = core.perm_interner().as_ref();
    let u8_proto_ptr = session.builtin_world().uint8array_proto.as_ptr() as *mut JsObject;
    let u8_proto = unsafe { &mut *u8_proto_ptr };
    // SAFETY: 指针来自 session 存活期内的 BuiltinWorld 对象。
    let _ = world.bind_method(
        u8_proto,
        sh,
        sf,
        "toBase64",
        unsafe {
            oxide_types::object::NativeFnPtr::from_raw(
                oxide_builtins::typed_array::uint8array_to_base64::<crate::vm::Vm> as *const (),
            )
        },
        0,
    );
    let _ = world.bind_method(
        u8_proto,
        sh,
        sf,
        "toHex",
        unsafe {
            oxide_types::object::NativeFnPtr::from_raw(
                oxide_builtins::typed_array::uint8array_to_hex::<crate::vm::Vm> as *const (),
            )
        },
        0,
    );
    let _ = world.bind_method(
        u8_proto,
        sh,
        sf,
        "setFromBase64",
        unsafe {
            oxide_types::object::NativeFnPtr::from_raw(
                oxide_builtins::typed_array::uint8array_set_from_base64::<crate::vm::Vm> as *const (),
            )
        },
        1,
    );
    let _ = world.bind_method(
        u8_proto,
        sh,
        sf,
        "setFromHex",
        unsafe {
            oxide_types::object::NativeFnPtr::from_raw(
                oxide_builtins::typed_array::uint8array_set_from_hex::<crate::vm::Vm> as *const (),
            )
        },
        1,
    );
    let u8_ctor_ptr = session.builtin_world().uint8array_constructor.as_ptr() as *mut JsObject;
    let u8_ctor = unsafe { &mut *u8_ctor_ptr };
    let _ = world.bind_method(
        u8_ctor,
        sh,
        sf,
        "fromBase64",
        unsafe {
            oxide_types::object::NativeFnPtr::from_raw(
                oxide_builtins::typed_array::uint8array_from_base64::<crate::vm::Vm> as *const (),
            )
        },
        1,
    );
    let _ = world.bind_method(
        u8_ctor,
        sh,
        sf,
        "fromHex",
        unsafe {
            oxide_types::object::NativeFnPtr::from_raw(
                oxide_builtins::typed_array::uint8array_from_hex::<crate::vm::Vm> as *const (),
            )
        },
        1,
    );

    bind_typed_array_abstract_ctor(core, session);

    // 把 `%TypedArray%` 抽象构造器暴露为全局 `TypedArray`（具体构造器的 [[Prototype]]）。
    let abstract_ctor_val =
        JsValue::from_js_object(session.builtin_world().typed_array_constructor.as_ptr() as *mut JsObject);
    bind_global_value(core, global, "TypedArray", abstract_ctor_val);

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
