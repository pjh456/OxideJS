//! 内置对象绑定：把 `oxide_builtins` 中的 native 实现安装到每个 session 的
//! builtin 构造器/原型及 global object 上。绑定在 session 创建（`init_kernel_builtins`）
//! 与 dirty reset（`rebind_dirty_builtins`）两个时机执行。

/// Array 构造器与原型的 native 方法绑定。
pub mod bind_array;
/// ArrayBuffer 构造器与原型的 native 方法绑定。
pub mod bind_array_buffer;
/// AsyncDisposableStack 构造器与原型的 native 方法绑定（含 dirty reset 同步）。
pub mod bind_async_disposable_stack;
/// BigInt 构造器与原型的 native 方法绑定。
pub mod bind_bigint;
/// Boolean 构造器与原型的 native 方法绑定。
pub mod bind_boolean;
/// Console 单例对象及其方法绑定。
pub mod bind_console;
/// DataView 构造器与原型的 native 方法绑定。
pub mod bind_data_view;
/// Date 构造器与原型的 native 方法绑定。
pub mod bind_date;
/// DisposableStack 构造器与原型的 native 方法绑定（含 dirty reset 同步）。
pub mod bind_disposable_stack;
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
        let val = $crate::JsValue::from_js_object($ctor_ptr);
        if let Some(pos) = $core.shape_forge().lookup_position($global.shape_id(), si) {
            // 既有槽（重建后保留的 global）：原位更新槽值，不追加新槽——旧家族
            // 构造器指针不得滞留属性 vec，shape 链不得继续增长。
            $global.set_prop_at(pos, val);
        } else {
            let shape = $core.shape_forge().make_shape($global.shape_id(), si);
            $global.set_shape_id(shape);
            if $hash {
                $global.ensure_hash_props().push(val);
            } else {
                $global.push_prop(val);
            }
            // 全局构造器槽位：规范描述符 { writable:true, enumerable:false,
            // configurable:true }，不设 meta 时默认全枚举会泄漏进
            // Object.keys(globalThis) / for-in。
            let global_pos = $global.prop_vec_len().saturating_sub(1) as u32;
            $global.set_data_meta(global_pos, oxide_types::object::PropAttributes::new(true, false, true));
            $global.bump_generation();
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
        // length 描述符 { writable:false, enumerable:false, configurable:true }
        // （规范构造器通用面；不写 meta 缺省全枚举会泄漏进 Object.keys）。
        let length_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        ctor.set_data_meta(length_pos, oxide_types::object::PropAttributes::new(false, false, true));
    }};
}

/// 给既有构造器对象安装 native 函数项指针、形参个数与构造器类型标签。
///
/// 绑定模块构造器注册的公共步骤：只更新既有对象的函数元数据，不新建对象、不写属性表。
/// 函数项指针经 `NativeFnPtr::from_raw` 存入。
///
/// # 注意事项
/// - `native_fn` 须为调用方转成 `*const ()` 的合法 `NativeFn` 函数项指针。
/// - 不设置 `length` 属性：需要 Function.length 的调用方自行补写。
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

/// 把 `(name, native_fn_ptr, nargs)` 绑定表逐条安装为目标对象的命名方法。
///
/// 每条经 `world.bind_method` 在 `target` 上建/复用方法 wrapper 并写命名属性；shape 与
/// 字符串 interner 由 `core` 借出，循环内共享。
///
/// # 注意事项
/// - 表中所有函数项指针须为合法的 `NativeFn`（转成 `*const ()`），此处经
///   `NativeFnPtr::from_raw` 转换。
/// - `bind_method` 的返回值被丢弃：单条绑定失败不中断后续条目。
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

/// 在原型上绑定一个字符串键的原生访问器 getter（如 Set/Map 的 `size`），
/// set 恒为 undefined。
///
/// # 步骤
/// 1. 以 `get <prop>` 为 name 标签委托到 [`Self::bind_accessor_getter_key`]。
///
/// # 注意事项
/// getter 函数对象登记进 world 释放表，与 `bind_method` 的方法 wrapper 同一生命周期约定
/// （session 收尾时统一释放）。
pub(crate) fn bind_accessor_getter(
    core: &Arc<KernelCore>, session: &KernelSession, proto: &mut JsObject, name: &str, getter_fn: *const (),
) {
    let si = core.perm_interner().intern(name).0;
    let getter_name = format!("get {name}");
    bind_accessor_getter_key(core, session, proto, si, &getter_name, getter_fn)
}

/// 在原型上按任意属性键（字符串 intern 键或 well-known symbol 键）绑定一个
/// 原生访问器 getter，set 恒为 undefined。
///
/// # 步骤
/// 1. 构造一个 native getter 函数对象（name 为 `getter_label`，length 0）。
/// 2. 为键开 shape 槽位并写入访问器 meta（enumerable=false, configurable=true）。
///
/// # 注意事项
/// getter 函数对象登记进 world 释放表，与 `bind_method` 的方法 wrapper 同一生命周期约定
/// （session 收尾时统一释放）。
pub(crate) fn bind_accessor_getter_key(
    core: &Arc<KernelCore>, session: &KernelSession, proto: &mut JsObject, key: u32, getter_label: &str,
    getter_fn: *const (),
) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    let world = session.builtin_world();

    let si_label = string_forge.intern(getter_label).0;
    // SAFETY: getter_fn 是转成 *const () 的 NativeFn 函数项指针。
    let getter_fn_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(getter_fn) };
    // 选择性重建复用：同家族同槽旧 getter 迁移到新 proto 的访问器槽（proto
    // 槽已由重建收尾重指），不再新建对象。
    let reuse_key =
        oxide_kernel::builtin::FnWrapperKey::new(world.wrapper_family_of(proto as *const JsObject), 0, key, si_label);
    let getter_ptr = match world.find_fn_wrapper(reuse_key, getter_fn_ptr, 0) {
        Some(ptr) => ptr,
        None => {
            let fn_proto_val = JsValue::from_js_object(world.function_proto.as_ptr() as *mut JsObject);
            let mut getter = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
            getter.set_function(true);
            getter.set_native_fn(Some(getter_fn_ptr));
            getter.set_native_arg_count(0);

            let si_name = string_forge.intern("name").0;
            let name_shape = shape_forge.make_shape(getter.shape_id(), si_name);
            getter.set_shape_id(name_shape);
            getter
                .ensure_hash_props()
                .push(JsValue::perm_string(string_forge.string_ptr(si_label)));
            let name_pos = getter.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
            getter.set_data_meta(name_pos, PropAttributes::new(false, false, true));

            let si_length = string_forge.intern("length").0;
            let length_shape = shape_forge.make_shape(getter.shape_id(), si_length);
            getter.set_shape_id(length_shape);
            getter.ensure_hash_props().push(JsValue::int(0));
            let length_pos = getter.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
            getter.set_data_meta(length_pos, PropAttributes::new(false, false, true));

            let getter_ptr = Box::into_raw(getter);
            world.track_fn_wrapper(getter_ptr, reuse_key);
            getter_ptr
        }
    };
    let getter_val = JsValue::from_js_object(getter_ptr);

    let new_shape = shape_forge.make_shape(proto.shape_id(), key);
    proto.set_shape_id(new_shape);
    let pos = proto.push_prop(JsValue::undefined());
    proto.set_accessor_meta(pos, getter_val, JsValue::undefined(), PropAttributes::new(true, false, true));
    proto.bump_generation();
}

/// 在原型上绑定一个原生访问器属性（getter + setter 成对），键可为字符串 intern 键
/// 或 well-known symbol 键（shape 键直接传入，不要求字符串 intern）。
///
/// # 步骤
/// 1. 构造 getter 函数对象（name = `getter_name`，length 0）与 setter 函数对象
///    （name = `setter_name`，length 1）。
/// 2. 为键开 shape 槽位并写入访问器 meta（enumerable=false, configurable=true）。
///
/// # 注意事项
/// getter/setter 函数对象登记进 world 释放表，session 收尾时统一释放，
/// 与 `bind_accessor_getter` 的 getter 及方法 wrapper 同一生命周期约定。
#[expect(clippy::too_many_arguments)]
pub(crate) fn bind_accessor_getset(
    core: &Arc<KernelCore>, session: &KernelSession, proto: &mut JsObject, key: u32, getter_name: &str,
    setter_name: &str, getter_fn: *const (), setter_fn: *const (),
) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    let world = session.builtin_world();
    let family = world.wrapper_family_of(proto as *const JsObject);
    let si_length = string_forge.intern("length").0;
    let si_name = string_forge.intern("name").0;

    // 选择性重建复用：同家族同槽旧 getter/setter 迁移到新 proto 的访问器槽
    // （proto 槽已由重建收尾重指），不再新建对象。
    let build = |si_label: u32,
                 fn_ptr_native: oxide_types::object::NativeFnPtr,
                 arg_count: u8,
                 length_val: i32|
     -> *mut JsObject {
        let reuse_key = oxide_kernel::builtin::FnWrapperKey::new(family, 0, key, si_label);
        if let Some(ptr) = world.find_fn_wrapper(reuse_key, fn_ptr_native, arg_count) {
            return ptr;
        }
        let fn_proto_val = JsValue::from_js_object(world.function_proto.as_ptr() as *mut JsObject);
        let mut func = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
        func.set_function(true);
        func.set_native_fn(Some(fn_ptr_native));
        func.set_native_arg_count(arg_count);
        // 给函数对象开 name/length 槽位（描述符均 { writable:false, enumerable:false, configurable:true }）。
        let name_shape = shape_forge.make_shape(func.shape_id(), si_name);
        func.set_shape_id(name_shape);
        func.ensure_hash_props()
            .push(JsValue::perm_string(string_forge.string_ptr(si_label)));
        let name_pos = func.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        func.set_data_meta(name_pos, PropAttributes::new(false, false, true));
        let length_shape = shape_forge.make_shape(func.shape_id(), si_length);
        func.set_shape_id(length_shape);
        func.ensure_hash_props().push(JsValue::int(length_val));
        let length_pos = func.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        func.set_data_meta(length_pos, PropAttributes::new(false, false, true));
        let func_ptr = Box::into_raw(func);
        world.track_fn_wrapper(func_ptr, reuse_key);
        func_ptr
    };
    // SAFETY: getter_fn / setter_fn 是转成 *const () 的 NativeFn 函数项指针。
    let getter_fn_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(getter_fn) };
    let setter_fn_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(setter_fn) };
    let si_getter = string_forge.intern(getter_name).0;
    let si_setter = string_forge.intern(setter_name).0;
    let getter_ptr = build(si_getter, getter_fn_ptr, 0, 0);
    let setter_ptr = build(si_setter, setter_fn_ptr, 1, 1);
    let getter_val = JsValue::from_js_object(getter_ptr);
    let setter_val = JsValue::from_js_object(setter_ptr);

    // 访问器属性槽：enumerable=false、configurable=true（get/set 函数对象已持有）。
    let new_shape = shape_forge.make_shape(proto.shape_id(), key);
    proto.set_shape_id(new_shape);
    let pos = proto.push_prop(JsValue::undefined());
    proto.set_accessor_meta(pos, getter_val, setter_val, PropAttributes::new(true, false, true));
    proto.bump_generation();
}

/// 为 `Iterator` 构造器绑定 `length`=0 与 `name`="Iterator" 属性（规范描述符均
/// { writable:false, enumerable:false, configurable:true }）。
///
/// `bind_iterator`（初始化路径）与 `bind_iterator_global`（dirty reset 路径）共用，
/// 防两处漂移导致 dirty reset 后 `Iterator.length`/`Iterator.name` 消失。
pub(crate) fn bind_iterator_ctor_identity(core: &Arc<KernelCore>, ctor: &mut JsObject) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    let si_length = string_forge.intern("length").0;
    let length_shape = shape_forge.make_shape(ctor.shape_id(), si_length);
    ctor.set_shape_id(length_shape);
    ctor.ensure_hash_props().push(JsValue::int(0));
    let length_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));
    let si_name = string_forge.intern("name").0;
    let name_shape = shape_forge.make_shape(ctor.shape_id(), si_name);
    ctor.set_shape_id(name_shape);
    ctor.ensure_hash_props()
        .push(JsValue::perm_string(string_forge.string_ptr(string_forge.intern("Iterator").0)));
    let name_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(name_pos, PropAttributes::new(false, false, true));
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
        world,
    );
}

/// 把原型上 `source` 属性已绑定的函数值复制到 well-known symbol 键名下（共享同一函数对象）。
///
/// # 边界
/// `source` 键不在原型上时静默跳过。
///
/// # 注意
/// well-known 键已在位时原位更新值，不追加重复 shape 槽：初始化与重绑可多轮
/// 经过同一原型，重复槽会让 delete 只删其一、读仍命中残留槽。
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
    if let Some(existing) = shape_forge.lookup_position(proto.shape_id(), key) {
        proto.set_prop_at(existing, value);
        proto.set_data_meta(existing, PropAttributes::new(true, false, true));
        proto.bump_generation();
        return;
    }
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

/// 在 Iterator 函数对象上绑定 `prototype` 属性 = %IteratorPrototype%（规范形状：
/// 构造器函数带 prototype 属性；描述符不可写/不可枚举/不可配置）。
pub(crate) fn bind_iterator_function_prototype(
    core: &Arc<KernelCore>, session: &KernelSession, iterator: &mut JsObject,
) {
    let iter_proto_ptr = session.builtin_world().iterator_proto.as_ptr() as *mut JsObject;
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();
    let si_prototype = sf.intern("prototype").0;
    let iterator_shape = sh.make_shape(iterator.shape_id(), si_prototype);
    iterator.set_shape_id(iterator_shape);
    let pos = iterator.ensure_hash_props().len() as u32;
    iterator.ensure_hash_props().push(JsValue::from_js_object(iter_proto_ptr));
    iterator.set_data_meta(pos, PropAttributes::new(false, false, false));
}

/// 在集合迭代器原型上绑定 `next` 方法（规范形状：next 挂原型，wrapper 不设 own）。
///
/// `next` 键已存在时跳过：保留原型在 dirty reset 重复经过时不再追加重复属性槽。
pub(crate) fn bind_iterator_proto_next(
    core: &Arc<KernelCore>, session: &KernelSession, proto: *mut JsObject, func: *const (),
) {
    let proto = unsafe { &mut *proto };
    let si_next = core.perm_interner().intern("next").0;
    if core.shape_forge().lookup_position(proto.shape_id(), si_next).is_some() {
        return;
    }
    apply_binding_table(session.builtin_world(), proto, core, &[("next", func, 0)]);
}

/// 把迭代器原型方法安装到当前 builtin world 的迭代器原型上：
/// `%IteratorPrototype%` 的 `@@iterator`（返回 this）与 Array/Map/Set/
/// RegExpString 四个集合原型的 `next`。
///
/// 在迭代器原型全新重建后调用（object 家族 dirty 或全量初始化）；各绑定点
/// 自带幂等检查，保留原型重复经过时安全跳过。
fn bind_iterator_protos(core: &Arc<KernelCore>, session: &KernelSession) {
    let world = session.builtin_world();

    // %IteratorPrototype% 自身可迭代：@@iterator 返回 this，经原型链被全部
    // 集合迭代器继承（it[Symbol.iterator]() === it 恒等）。
    let iter_proto_ptr = world.iterator_proto.as_ptr() as *mut JsObject;
    let iter_proto = unsafe { &mut *iter_proto_ptr };
    let sym_iter = oxide_types::private_key::make_well_known_symbol_key(0);
    if core.shape_forge().lookup_position(iter_proto.shape_id(), sym_iter).is_none() {
        bind_well_known_method(
            world,
            core,
            iter_proto,
            0,
            "iterator",
            oxide_builtins::iterator::iterator_symbol_iterator::<crate::vm::Vm> as *const (),
            0,
        );
    }

    // %IteratorPrototype% 的 constructor / @@toStringTag 访问器：getter 动态读
    // global（dirty reset 后 global 重建，不能缓存指针），setter 实现
    // SetterThatIgnoresPrototypeProperties（home 赋值抛 TypeError）。
    let si_constructor = core.perm_interner().intern("constructor").0;
    if core
        .shape_forge()
        .lookup_position(iter_proto.shape_id(), si_constructor)
        .is_none()
    {
        bind_accessor_getset(
            core,
            session,
            iter_proto,
            si_constructor,
            "get constructor",
            "set constructor",
            oxide_builtins::iterator::iterator_constructor_getter::<crate::vm::Vm> as *const (),
            oxide_builtins::iterator::iterator_constructor_setter::<crate::vm::Vm> as *const (),
        );
    }
    let sym_to_string_tag =
        oxide_types::private_key::make_well_known_symbol_key(oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG);
    if core
        .shape_forge()
        .lookup_position(iter_proto.shape_id(), sym_to_string_tag)
        .is_none()
    {
        bind_accessor_getset(
            core,
            session,
            iter_proto,
            sym_to_string_tag,
            "get [Symbol.toStringTag]",
            "set [Symbol.toStringTag]",
            oxide_builtins::iterator::iterator_to_string_tag_getter::<crate::vm::Vm> as *const (),
            oxide_builtins::iterator::iterator_to_string_tag_setter::<crate::vm::Vm> as *const (),
        );
    }

    // %IteratorPrototype% 的 @@dispose：显式资源管理下用 return 关闭迭代器。
    let dispose_id = oxide_types::private_key::WELL_KNOWN_SYMBOL_DISPOSE;
    let sym_dispose = oxide_types::private_key::make_well_known_symbol_key(dispose_id);
    if core.shape_forge().lookup_position(iter_proto.shape_id(), sym_dispose).is_none() {
        bind_well_known_method(
            world,
            core,
            iter_proto,
            dispose_id,
            "[Symbol.dispose]",
            oxide_builtins::iterator::iterator_dispose::<crate::vm::Vm> as *const (),
            0,
        );
    }

    // %IteratorPrototype% 的 6 个终端方法（就地消费底层迭代器，无 wrapper）：
    // forEach/every/some/find/reduce 带回调校验（失败也关底层），toArray 无参。
    // length：前 5 个为 1，toArray 为 0（规范 length/name 描述符由 bind_method 设置）。
    let si_for_each = core.perm_interner().intern("forEach").0;
    if core.shape_forge().lookup_position(iter_proto.shape_id(), si_for_each).is_none() {
        apply_binding_table(
            world,
            iter_proto,
            core,
            &[
                ("forEach", oxide_builtins::iterator::iterator_for_each::<crate::vm::Vm> as *const (), 1),
                ("every", oxide_builtins::iterator::iterator_every::<crate::vm::Vm> as *const (), 1),
                ("some", oxide_builtins::iterator::iterator_some::<crate::vm::Vm> as *const (), 1),
                ("find", oxide_builtins::iterator::iterator_find::<crate::vm::Vm> as *const (), 1),
                ("reduce", oxide_builtins::iterator::iterator_reduce::<crate::vm::Vm> as *const (), 1),
                ("toArray", oxide_builtins::iterator::iterator_to_array::<crate::vm::Vm> as *const (), 0),
            ],
        );
    }

    // %IteratorPrototype% 的 5 个返回迭代器方法（建 wrapper 挂
    // %IteratorHelperPrototype%）：map/filter/take/drop/flatMap，length 均为 1。
    let si_map = core.perm_interner().intern("map").0;
    if core.shape_forge().lookup_position(iter_proto.shape_id(), si_map).is_none() {
        apply_binding_table(
            world,
            iter_proto,
            core,
            &[
                ("map", oxide_builtins::iterator::iterator_map::<crate::vm::Vm> as *const (), 1),
                ("filter", oxide_builtins::iterator::iterator_filter::<crate::vm::Vm> as *const (), 1),
                ("take", oxide_builtins::iterator::iterator_take::<crate::vm::Vm> as *const (), 1),
                ("drop", oxide_builtins::iterator::iterator_drop::<crate::vm::Vm> as *const (), 1),
                ("flatMap", oxide_builtins::iterator::iterator_flat_map::<crate::vm::Vm> as *const (), 1),
            ],
        );
    }

    // %IteratorHelperPrototype%：wrapper 结果对象的共享原型（链到
    // %IteratorPrototype%），绑 next/return/throw 三方法（throw length=1，
    // next/return 为 0）+ @@toStringTag 数据属性 "Iterator Helper"。
    let helper_proto_ptr = world.iterator_helper_proto.as_ptr() as *mut JsObject;
    let helper_proto = unsafe { &mut *helper_proto_ptr };
    let si_next = core.perm_interner().intern("next").0;
    if core.shape_forge().lookup_position(helper_proto.shape_id(), si_next).is_none() {
        apply_binding_table(
            world,
            helper_proto,
            core,
            &[
                ("next", oxide_builtins::iterator::iterator_helper_next::<crate::vm::Vm> as *const (), 0),
                (
                    "return",
                    oxide_builtins::iterator::iterator_helper_return::<crate::vm::Vm> as *const (),
                    0,
                ),
                ("throw", oxide_builtins::iterator::iterator_helper_throw::<crate::vm::Vm> as *const (), 1),
            ],
        );
    }
    if core
        .shape_forge()
        .lookup_position(helper_proto.shape_id(), sym_to_string_tag)
        .is_none()
    {
        let sf = core.perm_interner().as_ref();
        let tag = JsValue::perm_string(sf.string_ptr(sf.intern("Iterator Helper").0));
        bind_well_known_data_property(core, helper_proto, 9, tag, PropAttributes::new(false, false, true));
    }

    // %ArrayIteratorPrototype% 服务 Array/TA 两族；Map/Set 共用按 `__mode__`
    // 分发的实现；%RegExpStringIteratorPrototype% 供 matchAll。
    bind_iterator_proto_next(
        core,
        session,
        world.array_iterator_proto.as_ptr() as *mut JsObject,
        oxide_builtins::array::array_iterator_next::<crate::vm::Vm> as *const (),
    );
    bind_iterator_proto_next(
        core,
        session,
        world.map_iterator_proto.as_ptr() as *mut JsObject,
        oxide_builtins::iterator::map_set_iterator_next::<crate::vm::Vm> as *const (),
    );
    bind_iterator_proto_next(
        core,
        session,
        world.set_iterator_proto.as_ptr() as *mut JsObject,
        oxide_builtins::iterator::map_set_iterator_next::<crate::vm::Vm> as *const (),
    );
    bind_iterator_proto_next(
        core,
        session,
        world.regexp_string_iterator_proto.as_ptr() as *mut JsObject,
        oxide_builtins::string::string_match_all_next::<crate::vm::Vm> as *const (),
    );
}

/// 给标准内置原型补装缺失的 `@@toStringTag` 数据属性
/// （`{ writable:false, enumerable:false, configurable:true }`）。
///
/// 幂等：目标已有 tag 键槽位时跳过（dirty reset 重复经过不追加重复属性槽）。
/// `%IteratorPrototype%` / BigInt 原型的 tag 已由各家族绑定点安装，不在此列。
fn install_to_string_tags(core: &Arc<KernelCore>, session: &KernelSession) {
    let world = session.builtin_world();
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();
    let tag_id = oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG;
    let tag_key = oxide_types::private_key::make_well_known_symbol_key(tag_id);
    let cases: [(*mut JsObject, &str); 12] = [
        (world.map_proto.as_ptr() as *mut JsObject, "Map"),
        (world.array_buffer_proto.as_ptr() as *mut JsObject, "ArrayBuffer"),
        (world.set_proto.as_ptr() as *mut JsObject, "Set"),
        (world.data_view_proto.as_ptr() as *mut JsObject, "DataView"),
        (world.array_iterator_proto.as_ptr() as *mut JsObject, "Array Iterator"),
        (world.map_iterator_proto.as_ptr() as *mut JsObject, "Map Iterator"),
        (world.set_iterator_proto.as_ptr() as *mut JsObject, "Set Iterator"),
        (world.string_iterator_proto.as_ptr() as *mut JsObject, "String Iterator"),
        (world.regexp_string_iterator_proto.as_ptr() as *mut JsObject, "String Iterator"),
        (world.math_object.as_ptr() as *mut JsObject, "Math"),
        (world.json_object.as_ptr() as *mut JsObject, "JSON"),
        (world.symbol_proto.as_ptr() as *mut JsObject, "Symbol"),
    ];
    for (ptr, tag) in cases {
        // SAFETY: ptr 为当前 builtin world 的内置原型，session 独占期间始终有效。
        let proto = unsafe { &mut *ptr };
        if sh.lookup_position(proto.shape_id(), tag_key).is_some() {
            continue;
        }
        let tag_val = JsValue::perm_string(sf.string_ptr(sf.intern(tag).0));
        bind_well_known_data_property(core, proto, tag_id, tag_val, PropAttributes::new(false, false, true));
    }
}

/// 对齐保留 global 上 `Iterator` 函数对象的 `prototype` 属性：object 家族重建会
/// 产生新的 `%IteratorPrototype%`，保留的 Iterator 函数对象须指向新原型。
///
/// global 同时重建（global dirty）或初始化时无 `Iterator` 属性，直接跳过——
/// 彼时由 `bind_iterator_global` 以新原型创建函数对象。
fn sync_iterator_function_prototype(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let si_iterator = core.perm_interner().intern("Iterator").0;
    let Some(pos) = core.shape_forge().lookup_position(global.shape_id(), si_iterator) else {
        return;
    };
    let iterator_val = global.get_prop_at(pos);
    if !iterator_val.is_object() {
        return;
    }
    let iterator = unsafe { &mut *iterator_val.as_js_object_ptr() };
    let si_prototype = core.perm_interner().intern("prototype").0;
    let Some(proto_pos) = core.shape_forge().lookup_position(iterator.shape_id(), si_prototype) else {
        return;
    };
    let new_proto = JsValue::from_js_object(session.builtin_world().iterator_proto.as_ptr() as *mut JsObject);
    iterator.set_prop_at(proto_pos, new_proto);
}

/// 在目标对象（global 或命名空间对象）上绑定一个具名槽：既有槽（选择性
/// 重建后保留的目标）原位更新槽值，无槽时开新槽。内置全局值统一非枚举：
/// 描述符 { writable:true, enumerable:false, configurable:true }，与构造器/
/// 命名空间对象（Math/JSON/Reflect/Temporal 等）规范一致。
pub(crate) fn bind_global_value(core: &Arc<KernelCore>, global: &mut JsObject, name: &str, value: JsValue) {
    let si = core.perm_interner().intern(name).0;
    if let Some(pos) = core.shape_forge().lookup_position(global.shape_id(), si) {
        // 原位更新：旧对象指针不得滞留在属性 vec，shape 链不得继续增长。
        global.set_prop_at(pos, value);
        return;
    }
    let shape = core.shape_forge().make_shape(global.shape_id(), si);
    global.set_shape_id(shape);
    global.ensure_hash_props().push(value);
    let pos = global.prop_vec_len().saturating_sub(1) as u32;
    global.set_data_meta(pos, PropAttributes::new(true, false, true));
    global.bump_generation();
}

fn bind_existing_global(core: &Arc<KernelCore>, global: &mut JsObject, name: &str, value: JsValue) {
    bind_global_value(core, global, name, value);
}

fn bind_error_subtype_global(
    core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject, name: &str, proto: &P<JsObject>,
    ctor_fn: *const (), arg_count: u8,
) {
    let constructor_si = core.perm_interner().intern("constructor").0;
    if let Some(pos) = core.shape_forge().lookup_position(proto.shape_id(), constructor_si) {
        let existing_ctor = proto.get_prop_at(pos);
        if existing_ctor.is_object() {
            bind_existing_global(core, global, name, existing_ctor);
            return;
        }
    }

    // fallback 自建路径与 bind_error 的 Box 自建路径保持同一形态：[[Prototype]] 指向
    // Error 构造器（NativeError 构造器继承 Error 构造器），prototype/name 描述符按规范。
    // 家族重建后新原型无既有槽：按复用键查找前轮旧构造器迁移，避免每轮新建。
    let world = session.builtin_world();
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();
    let si_prototype = sf.intern("prototype").0;
    let name_si = sf.intern(name).0;
    // SAFETY: ctor_fn 是转成 *const () 的 NativeFn 函数项指针。
    let ctor_fn_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(ctor_fn) };
    let reuse_key = oxide_kernel::builtin::FnWrapperKey::new(0, 0, name_si, name_si);
    let ctor_ptr = match world.find_fn_wrapper(reuse_key, ctor_fn_ptr, arg_count) {
        Some(ptr) => {
            // 迁移内引用：prototype 槽改指新子类型原型（旧原型已被重建替换）。
            let ctor = unsafe { &mut *ptr };
            if let Some(pos) = sh.lookup_position(ctor.shape_id(), si_prototype) {
                ctor.set_prop_at(pos, JsValue::from_js_object(proto.as_ptr() as *mut JsObject));
            }
            ptr
        }
        None => {
            let error_ctor_ptr = world.error_constructor.as_ptr() as *mut JsObject;
            let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(error_ctor_ptr)));
            ctor.set_function(true);
            configure_native_constructor(&mut ctor, ctor_fn, arg_count);

            let si_name = sf.intern("name").0;
            let ctor_shape1 = sh.make_shape(EMPTY_SHAPE_ID, si_prototype);
            let ctor_shape2 = sh.make_shape(ctor_shape1, si_name);
            ctor.set_shape_id(ctor_shape2);
            ctor.ensure_hash_props()
                .push(JsValue::from_js_object(proto.as_ptr() as *mut JsObject));
            ctor.ensure_hash_props().push(JsValue::perm_string(sf.string_ptr(name_si)));
            // prototype/name 描述符与 Box 自建路径一致：{f,f,f} / {f,f,t}。
            ctor.set_data_meta(0u32, PropAttributes::new(false, false, false));
            ctor.set_data_meta(1u32, PropAttributes::new(false, false, true));

            let ctor_ptr = Box::into_raw(ctor);
            world.track_fn_wrapper(ctor_ptr, reuse_key);
            ctor_ptr
        }
    };
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
            ("eval", oxide_builtins::eval::eval::<crate::vm::Vm> as *const (), 1),
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
    let reflect_ptr = Box::into_raw(reflect);
    session.builtin_world().track_leaked_object(reflect_ptr);
    bind_existing_global(core, global, "Reflect", JsValue::from_js_object(reflect_ptr));
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
    bind_iterator_ctor_identity(core, &mut iterator);
    bind_iterator_function_prototype(core, session, &mut iterator);
    apply_binding_table(
        session.builtin_world(),
        &mut iterator,
        core,
        &[("from", oxide_builtins::iterator::iterator_from::<crate::vm::Vm> as *const (), 1)],
    );
    let iterator_ptr = Box::into_raw(iterator);
    session.builtin_world().track_leaked_object(iterator_ptr);
    bind_existing_global(core, global, "Iterator", JsValue::from_js_object(iterator_ptr));
    // 迭代器原型方法（%IteratorPrototype% 的 @@iterator 与各集合原型 next）由
    // `bind_iterator_protos` 在 object 家族重建时统一安装，这里不重复绑定，
    // 避免保留原型经 dirty reset 时属性槽膨胀。
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
            oxide_builtins::array_buffer::shared_array_buffer_constructor::<crate::vm::Vm> as *const (),
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
        1,
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
        1,
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "ReferenceError",
        &world.reference_error_proto,
        oxide_builtins::error::reference_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "RangeError",
        &world.range_error_proto,
        oxide_builtins::error::range_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "SyntaxError",
        &world.syntax_error_proto,
        oxide_builtins::error::syntax_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "URIError",
        &world.uri_error_proto,
        oxide_builtins::error::uri_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    bind_error_subtype_global(
        core,
        session,
        global,
        "EvalError",
        &world.eval_error_proto,
        oxide_builtins::error::eval_error_constructor::<crate::vm::Vm> as *const (),
        1,
    );

    // SuppressedError 为三参构造器（error, suppressed, message），length=3；
    // 与 bind_error 的 Box 自建路径参数一致，保证 reset 重建后 length 不回落。
    bind_error_subtype_global(
        core,
        session,
        global,
        "SuppressedError",
        &world.suppressed_error_proto,
        oxide_builtins::error::suppressed_error_constructor::<crate::vm::Vm> as *const (),
        3,
    );

    bind_reflect_global(core, session, global);
    bind_iterator_global(core, session, global);
    bind_disposable_stack::bind_disposable_stack(core, session, global);
    bind_async_disposable_stack::bind_async_disposable_stack(core, session, global);
    bind_stub_globals(core, session, global);
    bind_bigint::bind_bigint(core, session, global);
    bind_global_functions(core, session, global);
    crate::test262_host::bind_test262_host(core, session, global);
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
        // object 家族重建连带重建 6 个迭代器原型与两资源栈原型（其链到新
        // Object.prototype），须同步安装原型方法并让保留的 global 构造器指向新原型。
        bind_iterator_protos(core, session);
        sync_iterator_function_prototype(core, session, global);
        bind_disposable_stack::bind_disposable_stack_protos(core, session);
        bind_disposable_stack::sync_disposable_stack_ctor(core, session, global);
        bind_async_disposable_stack::bind_async_disposable_stack_protos(core, session);
        bind_async_disposable_stack::sync_async_disposable_stack_ctor(core, session, global);
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
    if dirty.map_or(true, |d| d.console) {
        bind_console::bind_console(core, session, global);
    }
    // tag 目标横跨多个脏分组（Map/Set/DataView/Math/JSON/迭代器原型），
    // 各组各自重绑后统一补装；安装点自带幂等检查。
    install_to_string_tags(core, session);
}

/// 完整初始化一个 session 的内置对象（全量绑定 + `globalThis` + 快照记录）。
pub fn init_kernel_builtins(core: &Arc<KernelCore>, session: &mut KernelSession) {
    rebind_dirty_builtins(core, session, None);
    let global_ptr = session.global_object().as_ptr() as *mut oxide_types::object::JsObject;
    let global = unsafe { &mut *global_ptr };
    bind_iterator::bind_iterator(core, session, global);
    bind_disposable_stack::bind_disposable_stack(core, session, global);
    bind_async_disposable_stack::bind_async_disposable_stack(core, session, global);
    bind_reflect::bind_reflect(core, session, global);
    bind_global::bind_global(core, session, global);
    bind_global_value(core, global, "globalThis", JsValue::from_js_object(global_ptr));
    bind_console::bind_console(core, session, global);
    crate::test262_host::bind_test262_host(core, session, global);
    session.record_snapshot();
}
