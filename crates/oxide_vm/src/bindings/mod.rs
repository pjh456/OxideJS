//! 内置对象绑定：把 `oxide_builtins` 中的 native 实现安装到每个 session 的
//! builtin 构造器/原型及 global object 上。绑定在 session 创建（`init_kernel_builtins`）
//! 与 dirty reset（`rebind_dirty_builtins`）两个时机执行。

/// Array 构造器与原型的 native 方法绑定。
pub mod bind_array;
/// ArrayBuffer 构造器与原型的 native 方法绑定。
pub mod bind_array_buffer;
/// BigInt 构造器与原型的 native 方法绑定。
pub mod bind_bigint;
/// Boolean 构造器与原型的 native 方法绑定。
pub mod bind_boolean;
/// DataView 构造器与原型的 native 方法绑定。
pub mod bind_data_view;
/// Date 构造器与原型的 native 方法绑定。
pub mod bind_date;
/// Error 家族构造器与原型的 native 方法绑定（含各子类型构造器创建）。
pub mod bind_error;
/// Function 构造器与原型的 native 方法绑定。
pub mod bind_function;
/// global 对象上的普通全局函数（parseInt、isNaN 等）绑定。
pub mod bind_global;
/// Iterator 相关全局辅助对象（%IteratorPrototype% 等）绑定。
pub mod bind_iterator;
/// JSON 单例对象及其 native 方法绑定。
pub mod bind_json;
/// Map 构造器与原型的 native 方法绑定。
pub mod bind_map;
/// Math 单例对象及其 native 方法绑定。
pub mod bind_math;
/// Number 构造器与原型的 native 方法绑定（含常量属性）。
pub mod bind_number;
/// Object 构造器与原型的 native 方法绑定。
pub mod bind_object;
/// Reflect 单例对象及其 native 方法绑定。
pub mod bind_reflect;
/// RegExp 构造器与原型的 native 方法绑定。
pub mod bind_regexp;
/// Set 构造器与原型的 native 方法绑定。
pub mod bind_set;
/// String 构造器与原型的 native 方法绑定。
pub mod bind_string;
/// 未实现内置（Proxy/BigInt/WeakMap 等）的 stub 构造器绑定。
pub mod bind_stubs;
/// Symbol 构造器与原型的 native 方法绑定。
pub mod bind_symbol;
/// Temporal 命名空间对象（Now/Instant/PlainDate/PlainTime）绑定。
pub mod bind_temporal;
/// 各 TypedArray 构造器与共享原型的 native 方法绑定。
pub mod bind_typed_array;

use std::sync::Arc;

use oxide_kernel::builtin::BuiltinWorld;
use oxide_kernel::kernel::{BuiltinDirtySet, KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
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
        ctor.type_tag = oxide_types::object::JsObject::OBJ_TYPE_CONSTRUCTOR;
        // 设置 constructor.length (Function.length = formal parameter count)
        let length_si = $core.perm_interner().intern("length").0;
        let length_shape = $core.shape_forge().make_shape(ctor.shape_id(), length_si);
        ctor.set_shape_id(length_shape);
        ctor.ensure_hash_props().push($crate::JsValue::int($nargs as i32));
    }};
}

pub(crate) fn configure_native_constructor(ctor: &mut JsObject, native_fn: *const (), arg_count: u8) {
    // SAFETY: native_fn 始终是调用方转成 *const () 的合法 NativeFn 函数项指针。
    ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(native_fn) }));
    ctor.set_native_arg_count(arg_count);
    ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
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
        // SAFETY: 绑定表中所有条目都是转成 *const () 的 NativeFn 函数项指针。
        let fn_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(*func) };
        let _ = world.bind_method(target, shape_forge, string_forge, name, fn_ptr, *nargs);
    }
}

/// 在原型上绑定一个原生访问器 getter（如 Set/Map 的 `size`），set 恒为 undefined。
///
/// # 步骤
/// 1. 构造一个 native getter 函数对象（name 为 `get <prop>`，length 0）。
/// 2. 为属性名开 shape 槽位并写入访问器 meta。
///
/// # 注意事项
/// getter 函数对象经 `Box::into_raw` 持有，与 `bind_method` 的方法 wrapper 同一生命周期约定
/// （内置对象在 session 生命周期内不被回收）。
pub(crate) fn bind_accessor_getter(
    core: &Arc<KernelCore>, session: &KernelSession, proto: &mut JsObject, name: &str, getter_fn: *const (),
) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    let fn_proto_val = JsValue::from_js_object(session.builtin_world().function_proto.as_ptr() as *mut JsObject);

    let getter_name = format!("get {name}");
    let mut getter = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    getter.set_function(true);
    // SAFETY: getter_fn 是转成 *const () 的 NativeFn 函数项指针。
    getter.set_native_fn(Some(unsafe { oxide_types::object::NativeFnPtr::from_raw(getter_fn) }));
    getter.set_native_arg_count(0);

    let si_name = string_forge.intern("name").0;
    let name_shape = shape_forge.make_shape(getter.shape_id(), si_name);
    getter.set_shape_id(name_shape);
    getter
        .ensure_hash_props()
        .push(JsValue::perm_string(string_forge.string_ptr(string_forge.intern(&getter_name).0)));
    let name_pos = getter.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    getter.set_data_meta(name_pos, PropAttributes::new(false, false, true));

    let si_length = string_forge.intern("length").0;
    let length_shape = shape_forge.make_shape(getter.shape_id(), si_length);
    getter.set_shape_id(length_shape);
    getter.ensure_hash_props().push(JsValue::int(0));
    let length_pos = getter.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    getter.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    let getter_val = JsValue::from_js_object(Box::into_raw(getter));

    let si = string_forge.intern(name).0;
    let new_shape = shape_forge.make_shape(proto.shape_id(), si);
    proto.set_shape_id(new_shape);
    let pos = proto.push_prop(JsValue::undefined());
    proto.set_accessor_meta(pos, getter_val, JsValue::undefined(), PropAttributes::new(true, false, true));
    proto.bump_generation();
}

/// 把原型上 `source` 属性已绑定的函数值复制到 `alias` 名下（共享同一函数对象）。
///
/// 用于规范要求的方法别名（如 Set 的 `keys`/`@@iterator` 与 `values` 同一函数对象）。
pub(crate) fn bind_method_alias(core: &Arc<KernelCore>, proto: &mut JsObject, source: &str, alias: &str) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    let src_si = string_forge.intern(source).0;
    let Some(pos) = shape_forge.lookup_position(proto.shape_id(), src_si) else {
        return;
    };
    let value = proto.get_prop_at(pos);
    let alias_si = string_forge.intern(alias).0;
    let new_shape = shape_forge.make_shape(proto.shape_id(), alias_si);
    proto.set_shape_id(new_shape);
    let alias_pos = proto.push_prop(value);
    proto.set_data_meta(alias_pos, PropAttributes::new(true, false, true));
    proto.bump_generation();
}

/// 在原型上按 well-known symbol 键绑定方法（键不字符串 intern，读键经
/// `property_key_si` 的 well-known 分支映射到同一键）。
pub(crate) fn bind_well_known_method(
    world: &BuiltinWorld, core: &Arc<KernelCore>, target: &mut JsObject, well_known_id: u32, method_name: &str,
    func: *const (), nargs: u8,
) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    // SAFETY: func 是转成 *const () 的 NativeFn 函数项指针。
    let fn_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(func) };
    let key = oxide_types::private_key::make_well_known_symbol_key(well_known_id);
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method_key_static(
        target,
        shape_forge,
        string_forge,
        key,
        method_name,
        fn_ptr,
        nargs,
        world.fn_proto_val(),
    );
}

/// 把原型上 `source` 属性已绑定的函数值复制到 well-known symbol 键名下（共享同一函数对象）。
pub(crate) fn bind_well_known_method_alias(
    core: &Arc<KernelCore>, proto: &mut JsObject, source: &str, well_known_id: u32,
) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    let src_si = string_forge.intern(source).0;
    let Some(pos) = shape_forge.lookup_position(proto.shape_id(), src_si) else {
        return;
    };
    let value = proto.get_prop_at(pos);
    let key = oxide_types::private_key::make_well_known_symbol_key(well_known_id);
    let new_shape = shape_forge.make_shape(proto.shape_id(), key);
    proto.set_shape_id(new_shape);
    let alias_pos = proto.push_prop(value);
    proto.set_data_meta(alias_pos, PropAttributes::new(true, false, true));
    proto.bump_generation();
}

/// 在对象上按 well-known symbol 键绑定数据属性。
pub(crate) fn bind_well_known_data_property(
    core: &Arc<KernelCore>, target: &mut JsObject, well_known_id: u32, value: JsValue, attributes: PropAttributes,
) {
    let key = oxide_types::private_key::make_well_known_symbol_key(well_known_id);
    let new_shape = core.shape_forge().make_shape(target.shape_id(), key);
    target.set_shape_id(new_shape);
    let pos = target.push_prop(value);
    target.set_data_meta(pos, attributes);
    target.bump_generation();
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
            ("isNaN", oxide_builtins::global::global_is_nan::<crate::vm::Vm> as *const (), 1),
            ("isFinite", oxide_builtins::global::global_is_finite::<crate::vm::Vm> as *const (), 1),
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

    // %IteratorPrototype% 自身可迭代（@@iterator 返回 this，经原型链被全部集合迭代器继承）。
    let iter_proto_ptr = session.builtin_world().iterator_proto.as_ptr() as *mut JsObject;
    let iter_proto = unsafe { &mut *iter_proto_ptr };
    bind_well_known_method(
        session.builtin_world(),
        core,
        iter_proto,
        0,
        "iterator",
        oxide_builtins::iterator::iterator_symbol_iterator::<crate::vm::Vm> as *const (),
        0,
    );
}

fn bind_stub_globals(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    for (index, (name, native_fn, arg_count)) in [
        ("Proxy", oxide_builtins::stubs::proxy_stub::<crate::vm::Vm> as *const (), 2),
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

/// 把已有 builtin 构造器的 native 实现与 global 槽位一次性装配完整。
///
/// 在 global 对象重建（dirty reset）后调用：重新配置构造器 native 函数并重绑
/// `Object`/`Array`/`Math` 等全局名、TypedArray 家族、stub 与 `globalThis`。
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
        ("Temporal", JsValue::from_js_object(world.temporal_object.as_ptr() as *mut JsObject)),
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
        configure_existing_ctor(ctor, native_fn, 3);
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
    bind_bigint::bind_bigint(core, session, global);
    bind_global_functions(core, session, global);
    let global_this = JsValue::from_js_object(global as *mut JsObject);
    bind_existing_global(core, global, "globalThis", global_this);
}

/// 按脏标记重绑被污染的内置对象；`dirty` 为 `None` 时绑定全部（初始化路径）。
///
/// # 注意事项
/// 维护：新增 `BuiltinDirtySet` 分组时，须同步更新本重绑映射、`BuiltinSnapshot` 与
/// `BuiltinWorld::rebuild_with_dirty()`。
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
    if dirty.map_or(true, |d| d.temporal) {
        bind_temporal::bind_temporal(core, session, global);
    }
    if dirty.map_or(true, |d| d.stubs) {
        bind_stubs::bind_stubs(core, session, global);
    }
    if dirty.map_or(true, |d| d.stubs) {
        bind_bigint::bind_bigint(core, session, global);
    }
}

/// 完整初始化一个 session 的内置对象（全量绑定 + `globalThis` + 快照记录）。
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
