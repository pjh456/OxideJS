//! 方法安装职责：绑定层经公开入口调用的 native 方法 wrapper 工厂与五个家族的
//! bind_*_methods；选择性重建复用键（FnWrapperKey）命中旧 wrapper 时只迁移槽位。

use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::encode_symbol_key;
use oxide_types::value::JsValue;

use super::{ArrayMethods, BuiltinWorld, ErrorMethods, FnWrapperKey, FunctionMethods, ObjectMethods, StringMethods};
use crate::bind_methods;
use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::PermInterner;

/// 绑定环境：把绑定层三环境要素（builtin world、shape forge、string forge）
/// 打包为单一首参，整体传入 `bind_method*` 家族与五家族函数。
///
/// `Copy`：值语义轻量（三个引用），可自由复制传递。
///
/// vm 绑定层有同名字段集不同的 `BindEnv`（core/session/realm_id）：其
/// `world()` / `shape_forge()` / `string_forge()` 访问器是本结构的构造来源。
#[derive(Copy, Clone)]
pub struct BindEnv<'a> {
    pub world: &'a BuiltinWorld,
    pub shape_forge: &'a ShapeForge,
    pub string_forge: &'a PermInterner,
}

impl BuiltinWorld {
    /// 把 Object 家族方法安装到 Object 构造器与原型上（含 `hasOwnProperty` 等非枚举元数据修正）。
    pub fn bind_object_methods(env: BindEnv<'_>, methods: &ObjectMethods) {
        let world = env.world;
        let string_forge = env.string_forge;
        let shape_forge = env.shape_forge;
        let ctor_ptr = P::as_ptr(&world.object_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(
            env,
            ctor,
            ("keys", methods.keys, 1),
            ("create", methods.create, 2),
            ("assign", methods.assign, 2),
            ("is", methods.is, 2),
            ("defineProperty", methods.define_property, 3),
            ("getOwnPropertyDescriptor", methods.get_own_property_descriptor, 2),
            ("getOwnPropertyDescriptors", methods.get_own_property_descriptors, 1),
            ("freeze", methods.freeze, 1),
            ("seal", methods.seal, 1),
            ("preventExtensions", methods.prevent_extensions, 1),
            ("isFrozen", methods.is_frozen, 1),
            ("isSealed", methods.is_sealed, 1),
            ("isExtensible", methods.is_extensible, 1),
            ("getOwnPropertyNames", methods.get_own_property_names, 1),
            ("getOwnPropertySymbols", methods.get_own_property_symbols, 1),
            ("defineProperties", methods.define_properties, 2),
            ("fromEntries", methods.from_entries, 1),
            ("getPrototypeOf", methods.get_prototype_of, 1),
            ("setPrototypeOf", methods.set_prototype_of, 2),
            ("hasOwn", methods.has_own, 2),
            ("entries", methods.entries, 1),
            ("values", methods.values, 1),
            ("groupBy", methods.group_by, 2),
        );

        let proto_ptr = P::as_ptr(&world.object_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            env,
            proto,
            ("hasOwnProperty", methods.has_own_property, 1),
            ("propertyIsEnumerable", methods.property_is_enumerable, 1),
        );
        for name in ["hasOwnProperty", "propertyIsEnumerable"] {
            let si = string_forge.intern(name).0;
            if let Some(pos) = shape_forge.lookup_position(proto.shape_id(), si) {
                proto.set_data_meta(pos, PropAttributes::new(true, false, true));
            }
        }
    }

    /// 把 Array 家族方法安装到 Array 构造器与原型上。
    ///
    /// `realm_id` 是安装目标所属 realm 的编号：`@@iterator` 符号键按
    /// (realm 编号, 局部下标) 编码，realm 编号为 0 时与旧编码逐字节一致。
    pub fn bind_array_methods(env: BindEnv<'_>, methods: &ArrayMethods, realm_id: u32) {
        let world = env.world;
        let shape_forge = env.shape_forge;
        let ctor_ptr = P::as_ptr(&world.array_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(
            env,
            ctor,
            ("isArray", methods.is_array, 1),
            ("from", methods.from, 1),
            ("of", methods.of, 0),
        );

        let proto_ptr = P::as_ptr(&world.array_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            env,
            proto,
            ("push", methods.push, 1),
            ("pop", methods.pop, 0),
            ("slice", methods.slice, 2),
            ("splice", methods.splice, 2),
            ("concat", methods.concat, 1),
            ("join", methods.join, 1),
            ("indexOf", methods.index_of, 1),
            ("includes", methods.includes, 1),
            ("reverse", methods.reverse, 0),
            ("forEach", methods.for_each, 1),
            ("map", methods.map, 1),
            ("filter", methods.filter, 1),
            ("reduce", methods.reduce, 1),
            ("find", methods.find, 1),
            ("some", methods.some, 1),
            ("every", methods.every, 1),
            ("flat", methods.flat, 0),
            ("flatMap", methods.flat_map, 1),
            ("shift", methods.shift, 0),
            ("unshift", methods.unshift, 1),
            ("fill", methods.fill, 1),
            ("copyWithin", methods.copy_within, 2),
            ("at", methods.at, 1),
            ("lastIndexOf", methods.last_index_of, 1),
            ("findIndex", methods.find_index, 1),
            ("findLast", methods.find_last, 1),
            ("reduceRight", methods.reduce_right, 1),
            ("sort", methods.sort, 0),
            ("values", methods.values, 0),
            ("entries", methods.entries, 0),
            ("keys", methods.keys, 0),
            ("findLastIndex", methods.find_last_index, 1),
            ("toSorted", methods.to_sorted, 1),
            ("toReversed", methods.to_reversed, 0),
            ("toSpliced", methods.to_spliced, 2),
            ("with", methods.with_method, 2),
        );

        let iterator_key = encode_symbol_key(realm_id, 0);
        let raw = methods.values;
        // SAFETY: methods.values 是 VM 绑定层传入的 NativeFn 函数项。
        let func_ptr = unsafe { NativeFnPtr::from_raw(raw) };
        let _ = Self::bind_method_key_static(env, proto, iterator_key, "@@iterator", func_ptr, 0);
        debug_assert!(shape_forge.lookup_position(proto.shape_id(), iterator_key).is_some());
    }

    /// 把 Error 家族方法安装到 Error 及各子类型构造器与原型上。
    pub fn bind_error_methods(env: BindEnv<'_>, methods: &ErrorMethods) {
        let world = env.world;
        let string_forge = env.string_forge;
        let shape_forge = env.shape_forge;
        let ctor_ptr = P::as_ptr(&world.error_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(
            env,
            ctor,
            ("Error", methods.error, 1),
            ("TypeError", methods.type_error, 1),
            ("ReferenceError", methods.reference_error, 1),
            ("RangeError", methods.range_error, 1),
            ("SyntaxError", methods.syntax_error, 1),
            ("URIError", methods.uri_error, 1),
            ("EvalError", methods.eval_error, 1),
            ("SuppressedError", methods.suppressed_error, 3),
            ("isError", methods.is_error, 1),
        );

        let proto_ptr = P::as_ptr(&world.error_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            env,
            proto,
            ("toString", methods.to_string, 0),
            ("toJSON", methods.to_json, 1),
        );

        let si_name = string_forge.intern("name").0;
        let error_si = string_forge.intern("Error").0;
        let error_name_val = JsValue::perm_string(string_forge.string_ptr(error_si));
        let name_shape = shape_forge.make_shape(proto.shape_id(), si_name);
        proto.set_shape_id(name_shape);
        let name_pos = proto.push_prop(error_name_val);
        // 原型上的 name/message 按规范为非枚举数据属性，避免泄漏进 Object.keys/for-in。
        proto.set_data_meta(name_pos, oxide_types::object::PropAttributes::new(true, false, true));

        let si_message = string_forge.intern("message").0;
        let empty_si = string_forge.intern("").0;
        let empty_val = JsValue::perm_string(string_forge.string_ptr(empty_si));
        let msg_shape = shape_forge.make_shape(proto.shape_id(), si_message);
        proto.set_shape_id(msg_shape);
        let msg_pos = proto.push_prop(empty_val);
        proto.set_data_meta(msg_pos, oxide_types::object::PropAttributes::new(true, false, true));

        Self::set_subtype_proto_name(env, &world.type_error_proto, "TypeError", si_name);
        Self::set_subtype_proto_name(env, &world.reference_error_proto, "ReferenceError", si_name);
        Self::set_subtype_proto_name(env, &world.range_error_proto, "RangeError", si_name);
        Self::set_subtype_proto_name(env, &world.syntax_error_proto, "SyntaxError", si_name);
        Self::set_subtype_proto_name(env, &world.uri_error_proto, "URIError", si_name);
        Self::set_subtype_proto_name(env, &world.eval_error_proto, "EvalError", si_name);
        Self::set_subtype_proto_name(env, &world.suppressed_error_proto, "SuppressedError", si_name);
    }

    fn set_subtype_proto_name(env: BindEnv<'_>, proto_p: &P<JsObject>, name: &str, si_name: u32) {
        let string_forge = env.string_forge;
        let shape_forge = env.shape_forge;
        let proto_ptr = P::as_ptr(proto_p) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        let name_si = string_forge.intern(name).0;
        let name_val = JsValue::perm_string(string_forge.string_ptr(name_si));
        let name_shape = shape_forge.make_shape(proto.shape_id(), si_name);
        proto.set_shape_id(name_shape);
        let name_pos = proto.push_prop(name_val);
        // 子类型原型上的 name 同 Error.prototype.name：非枚举数据属性。
        proto.set_data_meta(name_pos, oxide_types::object::PropAttributes::new(true, false, true));
        // 子类型原型上的 message 初始值为空串，描述符同 name。
        let si_message = string_forge.intern("message").0;
        let empty_si = string_forge.intern("").0;
        let empty_val = JsValue::perm_string(string_forge.string_ptr(empty_si));
        let msg_shape = shape_forge.make_shape(proto.shape_id(), si_message);
        proto.set_shape_id(msg_shape);
        let msg_pos = proto.push_prop(empty_val);
        proto.set_data_meta(msg_pos, oxide_types::object::PropAttributes::new(true, false, true));
    }

    /// 把 String 家族方法安装到 String 构造器与原型上。
    pub fn bind_string_methods(env: BindEnv<'_>, methods: &StringMethods) {
        let world = env.world;
        let string_forge = env.string_forge;
        let shape_forge = env.shape_forge;
        let ctor_ptr = P::as_ptr(&world.string_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(
            env,
            ctor,
            ("fromCharCode", methods.from_char_code, 1),
            ("fromCodePoint", methods.from_code_point, 1),
            ("raw", methods.from_raw, 1),
        );

        let proto_ptr = P::as_ptr(&world.string_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            env,
            proto,
            ("indexOf", methods.index_of, 1),
            ("includes", methods.includes, 1),
            ("charAt", methods.char_at, 1),
            ("charCodeAt", methods.char_code_at, 1),
            ("concat", methods.concat, 1),
            ("slice", methods.slice, 2),
            ("substring", methods.substring, 2),
            ("toUpperCase", methods.to_upper_case, 0),
            ("toLowerCase", methods.to_lower_case, 0),
            ("toLocaleUpperCase", methods.to_locale_upper_case, 0),
            ("toLocaleLowerCase", methods.to_locale_lower_case, 0),
            ("localeCompare", methods.locale_compare, 1),
            ("trim", methods.trim, 0),
            ("repeat", methods.repeat, 1),
            ("padStart", methods.pad_start, 1),
            ("padEnd", methods.pad_end, 1),
            ("startsWith", methods.starts_with, 1),
            ("endsWith", methods.ends_with, 1),
            ("split", methods.split, 2),
            ("replace", methods.replace, 2),
            ("match", methods.match_fn, 1),
            ("search", methods.search, 1),
            ("trimStart", methods.trim_start, 0),
            ("trimEnd", methods.trim_end, 0),
            ("codePointAt", methods.code_point_at, 1),
            ("normalize", methods.normalize, 0),
            ("matchAll", methods.match_all, 1),
            ("replaceAll", methods.replace_all, 2),
            ("valueOf", methods.value_of, 0),
            ("substr", methods.substr, 2),
            ("at", methods.at, 1),
            ("lastIndexOf", methods.last_index_of, 1),
            ("isWellFormed", methods.is_well_formed, 0),
            ("toWellFormed", methods.to_well_formed, 0),
            ("anchor", methods.anchor, 1),
            ("big", methods.big, 0),
            ("blink", methods.blink, 0),
            ("bold", methods.bold, 0),
            ("fixed", methods.fixed, 0),
            ("fontcolor", methods.fontcolor, 1),
            ("fontsize", methods.fontsize, 1),
            ("italics", methods.italics, 0),
            ("link", methods.link, 1),
            ("small", methods.small, 0),
            ("strike", methods.strike, 0),
            ("sub", methods.sub, 0),
            ("sup", methods.sup, 0),
        );
        // trimLeft/trimRight 是 trimStart/trimEnd 的别名：规范上必须是同一函数
        // 对象（name 分别为 "trimStart"/"trimEnd"），不得新建 wrapper。
        for (source, alias) in [("trimStart", "trimLeft"), ("trimEnd", "trimRight")] {
            let src_si = string_forge.intern(source).0;
            if let Some(pos) = shape_forge.lookup_position(proto.shape_id(), src_si) {
                let value = proto.get_prop_at(pos);
                let alias_si = string_forge.intern(alias).0;
                let new_shape = shape_forge.make_shape(proto.shape_id(), alias_si);
                proto.set_shape_id(new_shape);
                let alias_pos = proto.push_prop(value);
                proto.set_data_meta(alias_pos, PropAttributes::new(true, false, true));
                proto.bump_generation();
            }
        }
    }

    /// 把 Function 原型方法安装到 Function.prototype 上。
    ///
    /// `realm_id` 是安装目标所属 realm 的编号：`@@hasInstance` 符号键按
    /// (realm 编号, 局部下标) 编码，realm 编号为 0 时与旧编码逐字节一致。
    pub fn bind_function_methods(env: BindEnv<'_>, methods: &FunctionMethods, realm_id: u32) {
        let world = env.world;
        let proto_ptr = P::as_ptr(&world.function_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            env,
            proto,
            ("call", methods.call, 1),
            ("apply", methods.apply, 2),
            ("bind", methods.bind, 1),
            ("toString", methods.to_string, 0),
        );
        // @@hasInstance 走 well-known symbol 键（id 6）：instanceof 运算符经
        // dispatch_instanceof 读该属性调用，绑定后属性存在性测试与全局改写生效。
        let _ = Self::bind_method_key_static(
            env,
            proto,
            encode_symbol_key(realm_id, 6),
            "[Symbol.hasInstance]",
            unsafe { NativeFnPtr::from_raw(methods.has_instance) },
            1,
        );
    }

    /// 在指定原型上安装一个 native 方法，wrapper 函数以本 world 的 Function 原型为原型。
    pub fn bind_method(
        env: BindEnv<'_>, proto: &mut JsObject, method_name: &str, native_fn_ptr: NativeFnPtr, arg_count: u8,
    ) -> Result<(), String> {
        let si = env.string_forge.intern(method_name).0;
        Self::bind_method_key_static(env, proto, si, method_name, native_fn_ptr, arg_count)
    }

    /// 按指定属性键安装方法 wrapper（键不要求字符串 intern，well-known symbol 键用此路径）。
    ///
    /// `method_name` 只用于 wrapper 的 `name` 属性；`key` 是属性的实际存储键。
    /// 目标站点标签取 0：P 目标由家族下标区分，非 P 目标须经
    /// [`Self::bind_method_key_labeled_static`] 传站点标签。
    pub fn bind_method_key_static(
        env: BindEnv<'_>, proto: &mut JsObject, key: u32, method_name: &str, native_fn_ptr: NativeFnPtr, arg_count: u8,
    ) -> Result<(), String> {
        Self::bind_method_key_labeled_static(env, proto, key, method_name, native_fn_ptr, arg_count, 0)
    }

    /// [`Self::bind_method`] 的站点标签版本：属性键由方法名 intern 得出，
    /// 供非 P 目标（VM 内建原型 Box 对象）的静态绑定宏使用。
    pub fn bind_method_labeled_static(
        env: BindEnv<'_>, proto: &mut JsObject, method_name: &str, native_fn_ptr: NativeFnPtr, arg_count: u8, label: u32,
    ) -> Result<(), String> {
        let si = env.string_forge.intern(method_name).0;
        Self::bind_method_key_labeled_static(env, proto, si, method_name, native_fn_ptr, arg_count, label)
    }

    /// 按指定属性键安装方法 wrapper，显式指定非 P 目标的绑定站点标签。
    ///
    /// 同一 session 内多个非 P 目标（VM 内建原型 Box 对象等）可绑定同名方法槽
    /// （如 next/return/throw）：站点标签（站点名 perm intern 键）使复用键跨
    /// 站点唯一，重绑时只迁移本站点的旧 wrapper。
    pub fn bind_method_key_labeled_static(
        env: BindEnv<'_>, proto: &mut JsObject, key: u32, method_name: &str, native_fn_ptr: NativeFnPtr, arg_count: u8,
        label: u32,
    ) -> Result<(), String> {
        let world = env.world;
        let shape_forge = env.shape_forge;
        let string_forge = env.string_forge;
        let si = string_forge.intern(method_name).0;
        // 选择性重建复用：同键旧 wrapper 迁移到新 proto 槽位——其 proto 槽
        // 已由重建收尾改写到新指针（或 Function 家族保留而不变），length/name
        // 属性区随对象保留，不再新建对象、登记表跨重建不增长。
        let reuse_key = FnWrapperKey::new(world.wrapper_family_of(proto as *const JsObject), label, key, si);
        let wrapper_ptr = match world.find_fn_wrapper(reuse_key, native_fn_ptr, arg_count) {
            Some(ptr) => ptr,
            None => {
                let wrapper_proto_val = world.fn_proto_val();
                let wrapper_proto_ptr = if wrapper_proto_val.is_object() {
                    wrapper_proto_val.as_js_object_ptr()
                } else {
                    std::ptr::null_mut()
                };
                let mut wrapper = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
                if !wrapper_proto_ptr.is_null() {
                    wrapper.set_proto(wrapper_proto_val).ok();
                }
                wrapper.set_function(true);
                // NativeFnPtr 不变量由调用方维护（bind_method 家族的调用方
                // 均使用函数项表达式）。
                wrapper.set_native_fn(Some(native_fn_ptr));
                wrapper.set_native_arg_count(arg_count);
                // 设置 .length（JS 规范：值为形式参数个数，
                // writable:false, enumerable:false, configurable:true）。
                let si_length = string_forge.intern("length").0;
                let length_shape = shape_forge.make_shape(wrapper.shape_id(), si_length);
                wrapper.set_shape_id(length_shape);
                wrapper.ensure_hash_props().push(JsValue::int(arg_count as i32));
                let length_pos = wrapper.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
                wrapper.set_data_meta(length_pos, oxide_types::object::PropAttributes::new(false, false, true));
                // 设置 .name（writable:false, enumerable:false, configurable:true）。
                let si_name = string_forge.intern("name").0;
                let name_shape = shape_forge.make_shape(wrapper.shape_id(), si_name);
                wrapper.set_shape_id(name_shape);
                wrapper
                    .ensure_hash_props()
                    .push(JsValue::perm_string(string_forge.string_ptr(si)));
                let name_pos = wrapper.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
                wrapper.set_data_meta(name_pos, oxide_types::object::PropAttributes::new(false, false, true));
                // 登记进 world 释放表：session 收尾时统一释放 wrapper 的属性区与本体。
                let wrapper_ptr = Box::into_raw(wrapper);
                world.track_fn_wrapper(wrapper_ptr, reuse_key);
                wrapper_ptr
            }
        };
        let wrapper_val = JsValue::from_js_object(wrapper_ptr);
        let new_shape = shape_forge.make_shape(proto.shape_id(), key);
        proto.set_shape_id(new_shape);
        proto.ensure_hash_props().push(wrapper_val);
        // 内置原型方法按 ES 规范非枚举；否则会泄漏进 for-in 枚举
        // （例如 `for k in []` 中会出现 array push/pop）。
        let method_pos = proto.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        proto.set_data_meta(method_pos, oxide_types::object::PropAttributes::new(true, false, true));
        proto.bump_generation();
        Ok(())
    }
}
