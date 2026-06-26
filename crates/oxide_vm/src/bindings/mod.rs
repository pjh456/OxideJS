pub mod bind_array;
pub mod bind_array_buffer;
pub mod bind_boolean;
pub mod bind_data_view;
pub mod bind_date;
pub mod bind_error;
pub mod bind_function;
pub mod bind_global;
pub mod bind_iterator;
pub mod bind_json;
pub mod bind_map;
pub mod bind_math;
pub mod bind_number;
pub mod bind_object;
pub mod bind_reflect;
pub mod bind_regexp;
pub mod bind_set;
pub mod bind_string;
pub mod bind_stubs;
pub mod bind_symbol;
pub mod bind_typed_array;

use std::sync::Arc;

use oxide_kernel::builtin::BuiltinWorld;
use oxide_kernel::kernel::{BuiltinDirtySet, KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

#[macro_export]
macro_rules! bind_constructor {
    ($core:expr, $global:expr, $name:literal, $ctor_ptr:expr, $ctor_fn:path, $nargs:literal) => {{
        bind_constructor!($core, $global, $name, $ctor_ptr, $ctor_fn, $nargs, hash: false)
    }};
    ($core:expr, $global:expr, $name:literal, $ctor_ptr:expr, $ctor_fn:path, $nargs:literal, hash: $hash:literal) => {{
        let si = $core.perm_interner().intern($name).0;
        let shape = $core.shape_forge().make_shape($global.shape_id(), si);
        let val = $crate::JsValue::from_js_object($ctor_ptr);
        $global.set_shape_id(shape);
        if $hash {
            $global.ensure_hash_props().push(val);
            $global.bump_generation();
        } else {
            $global.push_prop(val);
        }
        let ctor = unsafe { &mut *$ctor_ptr };
        let ptr: *const () = ($ctor_fn as fn(&mut $crate::vm::Vm, &[u8]) -> oxide_runtime_api::NativeResult) as *const ();
        ctor.set_native_fn(Some(unsafe { oxide_types::object::NativeFnPtr::from_raw(ptr) }));
        ctor.set_native_arg_count($nargs);
    }};
}

pub(crate) fn configure_native_constructor(ctor: &mut JsObject, native_fn: *const (), arg_count: u8) {
    // SAFETY: native_fn is always a valid NativeFn fn-item pointer cast to *const () by callers.
    ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(native_fn) }));
    ctor.set_native_arg_count(arg_count);
}

fn configure_existing_ctor(ctor: &P<JsObject>, native_fn: *const (), arg_count: u8) {
    let ctor_ptr = ctor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    configure_native_constructor(ctor, native_fn, arg_count);
}

pub(crate) fn apply_binding_table(
    world: &BuiltinWorld, target: &mut JsObject, core: &Arc<KernelCore>, bindings: &[(&'static str, *const (), u8)],
) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    for (name, func, nargs) in bindings {
        // SAFETY: all entries in the binding table are NativeFn fn-item pointers cast to *const ().
        let fn_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(*func) };
        let _ = world.bind_method(target, shape_forge, string_forge, name, fn_ptr, *nargs);
    }
}

pub(crate) fn bind_global_value(core: &Arc<KernelCore>, global: &mut JsObject, name: &str, value: JsValue) {
    let si = core.perm_interner().intern(name).0;
    let shape = core.shape_forge().make_shape(global.shape_id(), si);
    global.set_shape_id(shape);
    global.ensure_hash_props().push(value);
    global.bump_generation();
}

fn bind_existing_global(core: &Arc<KernelCore>, global: &mut JsObject, name: &str, value: JsValue) {
    bind_global_value(core, global, name, value);
}

fn bind_error_subtype_global(
    core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject, name: &str, proto: &P<JsObject>,
    ctor_fn: *const (),
) {
    let constructor_si = core.perm_interner().intern("constructor").0;
    if let Some(pos) = core.shape_forge().lookup_position(proto.shape_id(), constructor_si) {
        let existing_ctor = proto.get_prop_at(pos);
        if existing_ctor.is_object() {
            bind_existing_global(core, global, name, existing_ctor);
            return;
        }
    }

    let function_proto_ptr = session.builtin_world().function_proto.as_ptr() as *mut JsObject;
    let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto_ptr)));
    ctor.set_function(true);
    configure_native_constructor(&mut ctor, ctor_fn, 1);

    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();
    let si_prototype = sf.intern("prototype").0;
    let si_name = sf.intern("name").0;
    let name_si = sf.intern(name).0;
    let ctor_shape1 = sh.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let ctor_shape2 = sh.make_shape(ctor_shape1, si_name);
    ctor.set_shape_id(ctor_shape2);
    ctor.ensure_hash_props()
        .push(JsValue::from_js_object(proto.as_ptr() as *mut JsObject));
    ctor.ensure_hash_props().push(JsValue::perm_string(sf.string_ptr(name_si)));

    let ctor_ptr = Box::into_raw(ctor);
    bind_existing_global(core, global, name, JsValue::from_js_object(ctor_ptr));
}

fn bind_global_functions(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    apply_binding_table(
        session.builtin_world(),
        global,
        core,
        &[
            ("parseInt", oxide_builtins::number::number_parse_int::<crate::vm::Vm> as *const (), 1),
            ("parseFloat", oxide_builtins::number::number_parse_float::<crate::vm::Vm> as *const (), 1),
            ("escape", oxide_builtins::global::js_escape::<crate::vm::Vm> as *const (), 1),
            ("unescape", oxide_builtins::global::js_unescape::<crate::vm::Vm> as *const (), 1),
            ("encodeURI", oxide_builtins::global::encode_uri::<crate::vm::Vm> as *const (), 1),
            ("decodeURI", oxide_builtins::global::decode_uri::<crate::vm::Vm> as *const (), 1),
            (
                "encodeURIComponent",
                oxide_builtins::global::encode_uri_component::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "decodeURIComponent",
                oxide_builtins::global::decode_uri_component::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );
}

fn bind_reflect_global(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let mut reflect = Box::new(JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(session.builtin_world().object_proto.as_ptr() as *mut JsObject),
    ));
    apply_binding_table(
        session.builtin_world(),
        &mut reflect,
        core,
        &[
            ("apply", oxide_builtins::reflect::reflect_apply::<crate::vm::Vm> as *const (), 3),
            ("construct", oxide_builtins::reflect::reflect_construct::<crate::vm::Vm> as *const (), 2),
            (
                "defineProperty",
                oxide_builtins::reflect::reflect_define_property::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "deleteProperty",
                oxide_builtins::reflect::reflect_delete_property::<crate::vm::Vm> as *const (),
                2,
            ),
            ("get", oxide_builtins::reflect::reflect_get::<crate::vm::Vm> as *const (), 2),
            (
                "getOwnPropertyDescriptor",
                oxide_builtins::reflect::reflect_get_own_property_descriptor::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getPrototypeOf",
                oxide_builtins::reflect::reflect_get_prototype_of::<crate::vm::Vm> as *const (),
                1,
            ),
            ("has", oxide_builtins::reflect::reflect_has::<crate::vm::Vm> as *const (), 2),
            (
                "isExtensible",
                oxide_builtins::reflect::reflect_is_extensible::<crate::vm::Vm> as *const (),
                1,
            ),
            ("ownKeys", oxide_builtins::reflect::reflect_own_keys::<crate::vm::Vm> as *const (), 1),
            (
                "preventExtensions",
                oxide_builtins::reflect::reflect_prevent_extensions::<crate::vm::Vm> as *const (),
                1,
            ),
            ("set", oxide_builtins::reflect::reflect_set::<crate::vm::Vm> as *const (), 3),
            (
                "setPrototypeOf",
                oxide_builtins::reflect::reflect_set_prototype_of::<crate::vm::Vm> as *const (),
                2,
            ),
        ],
    );
    bind_existing_global(core, global, "Reflect", JsValue::from_js_object(Box::into_raw(reflect)));
}

fn bind_iterator_global(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let function_proto = session.builtin_world().function_proto.as_ptr() as *mut JsObject;
    let mut iterator = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto)));
    iterator.set_function(true);
    configure_native_constructor(
        &mut iterator,
        oxide_builtins::iterator::iterator_constructor::<crate::vm::Vm> as *const (),
        0,
    );
    apply_binding_table(
        session.builtin_world(),
        &mut iterator,
        core,
        &[("from", oxide_builtins::iterator::iterator_from::<crate::vm::Vm> as *const (), 1)],
    );
    bind_existing_global(core, global, "Iterator", JsValue::from_js_object(Box::into_raw(iterator)));
}

fn bind_stub_globals(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    for (index, (name, native_fn, arg_count)) in [
        ("Proxy", oxide_builtins::stubs::proxy_stub::<crate::vm::Vm> as *const (), 2),
        ("BigInt", oxide_builtins::stubs::bigint_stub::<crate::vm::Vm> as *const (), 1),
        ("WeakMap", oxide_builtins::stubs::weakmap_stub::<crate::vm::Vm> as *const (), 0),
        ("WeakSet", oxide_builtins::stubs::weakset_stub::<crate::vm::Vm> as *const (), 0),
        ("WeakRef", oxide_builtins::stubs::weakref_stub::<crate::vm::Vm> as *const (), 1),
        (
            "FinalizationRegistry",
            oxide_builtins::stubs::finalization_registry_stub::<crate::vm::Vm> as *const (),
            1,
        ),
        (
            "SharedArrayBuffer",
            oxide_builtins::stubs::shared_array_buffer_stub::<crate::vm::Vm> as *const (),
            1,
        ),
        ("Atomics", oxide_builtins::stubs::atomics_stub::<crate::vm::Vm> as *const (), 0),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(stub) = session.builtin_world().stub_objects.get(index) {
            configure_existing_ctor(stub, native_fn, arg_count);
            bind_existing_global(core, global, name, JsValue::from_js_object(stub.as_ptr() as *mut JsObject));
        }
    }
}

pub fn bind_global_builtin_slots(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let world = session.builtin_world();

    configure_existing_ctor(
        &world.object_constructor,
        oxide_builtins::object::object_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    configure_existing_ctor(
        &world.array_constructor,
        oxide_builtins::array::array_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    configure_existing_ctor(
        &world.array_buffer_constructor,
        oxide_builtins::array_buffer::array_buffer_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    configure_existing_ctor(
        &world.data_view_constructor,
        oxide_builtins::data_view::data_view_constructor::<crate::vm::Vm> as *const (),
        3,
    );
    configure_existing_ctor(
        &world.error_constructor,
        oxide_builtins::error::error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    configure_existing_ctor(
        &world.number_constructor,
        oxide_builtins::number::number_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    configure_existing_ctor(
        &world.date_constructor,
        oxide_builtins::date::date_constructor::<crate::vm::Vm> as *const (),
        7,
    );
    configure_existing_ctor(
        &world.set_constructor,
        oxide_builtins::set::set_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    configure_existing_ctor(
        &world.map_constructor,
        oxide_builtins::map::map_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    configure_existing_ctor(
        &world.boolean_constructor,
        oxide_builtins::boolean::boolean_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    configure_existing_ctor(
        &world.regexp_constructor,
        oxide_builtins::regexp::regexp_constructor::<crate::vm::Vm> as *const (),
        2,
    );
    configure_existing_ctor(
        &world.symbol_constructor,
        oxide_builtins::symbol::symbol_constructor::<crate::vm::Vm> as *const (),
        1,
    );

    for (name, value) in [
        ("Object", JsValue::from_js_object(world.object_constructor.as_ptr() as *mut JsObject)),
        ("Array", JsValue::from_js_object(world.array_constructor.as_ptr() as *mut JsObject)),
        (
            "ArrayBuffer",
            JsValue::from_js_object(world.array_buffer_constructor.as_ptr() as *mut JsObject),
        ),
        ("DataView", JsValue::from_js_object(world.data_view_constructor.as_ptr() as *mut JsObject)),
        ("Error", JsValue::from_js_object(world.error_constructor.as_ptr() as *mut JsObject)),
        ("String", JsValue::from_js_object(world.string_constructor.as_ptr() as *mut JsObject)),
        ("Number", JsValue::from_js_object(world.number_constructor.as_ptr() as *mut JsObject)),
        ("Date", JsValue::from_js_object(world.date_constructor.as_ptr() as *mut JsObject)),
        ("Set", JsValue::from_js_object(world.set_constructor.as_ptr() as *mut JsObject)),
        ("Map", JsValue::from_js_object(world.map_constructor.as_ptr() as *mut JsObject)),
        ("Boolean", JsValue::from_js_object(world.boolean_constructor.as_ptr() as *mut JsObject)),
        ("Function", JsValue::from_js_object(world.function_constructor.as_ptr() as *mut JsObject)),
        ("RegExp", JsValue::from_js_object(world.regexp_constructor.as_ptr() as *mut JsObject)),
        ("Symbol", JsValue::from_js_object(world.symbol_constructor.as_ptr() as *mut JsObject)),
        ("Math", JsValue::from_js_object(world.math_object.as_ptr() as *mut JsObject)),
        ("JSON", JsValue::from_js_object(world.json_object.as_ptr() as *mut JsObject)),
    ] {
        bind_existing_global(core, global, name, value);
    }

    for (name, ctor, native_fn) in [
        (
            "Int8Array",
            &world.int8array_constructor,
            oxide_builtins::typed_array::int8array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "Uint8Array",
            &world.uint8array_constructor,
            oxide_builtins::typed_array::uint8array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "Uint8ClampedArray",
            &world.uint8clampedarray_constructor,
            oxide_builtins::typed_array::uint8clampedarray_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "Int16Array",
            &world.int16array_constructor,
            oxide_builtins::typed_array::int16array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "Uint16Array",
            &world.uint16array_constructor,
            oxide_builtins::typed_array::uint16array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "Int32Array",
            &world.int32array_constructor,
            oxide_builtins::typed_array::int32array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "Uint32Array",
            &world.uint32array_constructor,
            oxide_builtins::typed_array::uint32array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "Float32Array",
            &world.float32array_constructor,
            oxide_builtins::typed_array::float32array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "Float64Array",
            &world.float64array_constructor,
            oxide_builtins::typed_array::float64array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "BigInt64Array",
            &world.bigint64array_constructor,
            oxide_builtins::typed_array::bigint64array_constructor::<crate::vm::Vm> as *const (),
        ),
        (
            "BigUint64Array",
            &world.biguint64array_constructor,
            oxide_builtins::typed_array::biguint64array_constructor::<crate::vm::Vm> as *const (),
        ),
    ] {
        configure_existing_ctor(ctor, native_fn, 1);
        bind_existing_global(core, global, name, JsValue::from_js_object(ctor.as_ptr() as *mut JsObject));
    }

    bind_error_subtype_global(
        core,
        session,
        global,
        "TypeError",
        &world.type_error_proto,
        oxide_builtins::error::type_error_constructor::<crate::vm::Vm> as *const (),
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "ReferenceError",
        &world.reference_error_proto,
        oxide_builtins::error::reference_error_constructor::<crate::vm::Vm> as *const (),
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "RangeError",
        &world.range_error_proto,
        oxide_builtins::error::range_error_constructor::<crate::vm::Vm> as *const (),
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "SyntaxError",
        &world.syntax_error_proto,
        oxide_builtins::error::syntax_error_constructor::<crate::vm::Vm> as *const (),
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "URIError",
        &world.uri_error_proto,
        oxide_builtins::error::uri_error_constructor::<crate::vm::Vm> as *const (),
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "EvalError",
        &world.eval_error_proto,
        oxide_builtins::error::eval_error_constructor::<crate::vm::Vm> as *const (),
    );

    bind_reflect_global(core, session, global);
    bind_iterator_global(core, session, global);
    bind_stub_globals(core, session, global);
    bind_global_functions(core, session, global);
    let global_this = JsValue::from_js_object(global as *mut JsObject);
    bind_existing_global(core, global, "globalThis", global_this);
}

/// Maintenance: when adding a `BuiltinDirtySet` group, update this rebind map,
/// `BuiltinSnapshot`, and `BuiltinWorld::rebuild_with_dirty()` together.
pub fn rebind_dirty_builtins(core: &Arc<KernelCore>, session: &mut KernelSession, dirty: Option<&BuiltinDirtySet>) {
    let global_ptr = session.global_object().as_ptr() as *mut JsObject;
    let global = unsafe { &mut *global_ptr };

    if dirty.map_or(true, |d| d.object) {
        bind_object::bind_object(core, session, global);
    }
    if dirty.map_or(true, |d| d.array) {
        bind_array::bind_array(core, session, global);
    }
    if dirty.map_or(true, |d| d.array_buffer) {
        bind_array_buffer::bind_array_buffer(core, session, global);
    }
    if dirty.map_or(true, |d| d.data_view) {
        bind_data_view::bind_data_view(core, session, global);
    }
    if dirty.map_or(true, |d| d.typed_array_family) {
        bind_typed_array::bind_typed_array(core, session, global);
    }
    if dirty.map_or(true, |d| d.error_family) {
        bind_error::bind_error(core, session, global);
    }
    if dirty.map_or(true, |d| d.string) {
        bind_string::bind_string(core, session, global);
    }
    if dirty.map_or(true, |d| d.number) {
        bind_number::bind_number(core, session, global);
    }
    if dirty.map_or(true, |d| d.math) {
        bind_math::bind_math(core, session, global);
    }
    if dirty.map_or(true, |d| d.json) {
        bind_json::bind_json(core, session, global);
    }
    if dirty.map_or(true, |d| d.date) {
        bind_date::bind_date(core, session, global);
    }
    if dirty.map_or(true, |d| d.set) {
        bind_set::bind_set(core, session, global);
    }
    if dirty.map_or(true, |d| d.map) {
        bind_map::bind_map(core, session, global);
    }
    if dirty.map_or(true, |d| d.boolean) {
        bind_boolean::bind_boolean(core, session, global);
    }
    if dirty.map_or(true, |d| d.function) {
        bind_function::bind_function(core, session, global);
    }
    if dirty.map_or(true, |d| d.regexp) {
        bind_regexp::bind_regexp(core, session, global);
    }
    if dirty.map_or(true, |d| d.symbol_family) {
        bind_symbol::bind_symbol(core, session, global);
    }
    if dirty.map_or(true, |d| d.stubs) {
        bind_stubs::bind_stubs(core, session, global);
    }
}

pub fn init_kernel_builtins(core: &Arc<KernelCore>, session: &mut KernelSession) {
    rebind_dirty_builtins(core, session, None);
    let global_ptr = session.global_object().as_ptr() as *mut oxide_types::object::JsObject;
    let global = unsafe { &mut *global_ptr };
    bind_iterator::bind_iterator(core, session, global);
    bind_reflect::bind_reflect(core, session, global);
    bind_global::bind_global(core, session, global);
    bind_global_value(core, global, "globalThis", JsValue::from_js_object(global_ptr));
    session.record_snapshot();
}
