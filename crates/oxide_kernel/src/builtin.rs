use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::StringForge;

#[macro_export]
macro_rules! bind_method {
    ($world:expr, $target:expr, $sf:expr, $sh:expr, $name:literal, $func:expr, $nargs:expr) => {{
        let _raw: *const () = $func as *const ();
        // SAFETY: $func is a NativeFn fn-item; a fn-item coerced to *const () is always valid.
        let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
        let _ = $world.bind_method($target, $sh, $sf, $name, _func_ptr, $nargs);
    }};
}

#[macro_export]
macro_rules! bind_methods {
    ($world:expr, $target:expr, $sf:expr, $sh:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $( $crate::bind_method!($world, $target, $sf, $sh, $name, $func, $nargs); )*
    };
}

#[macro_export]
macro_rules! bind_methods_static {
    ($target:expr, $sf:expr, $sh:expr, $wrapper_proto:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $({
            let _raw: *const () = $func as *const ();
            // SAFETY: $func is a NativeFn fn-item; valid to coerce and wrap.
            let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
            let _ = $crate::builtin::BuiltinWorld::bind_method_static(
                $target, $sh, $sf, $name, _func_ptr, $nargs, $wrapper_proto,
            );
        })*
    };
}

pub struct ObjectMethods {
    pub keys: *const (),
    pub create: *const (),
    pub assign: *const (),
    pub is: *const (),
    pub define_property: *const (),
    pub get_own_property_descriptor: *const (),
    pub freeze: *const (),
    pub seal: *const (),
    pub prevent_extensions: *const (),
    pub is_frozen: *const (),
    pub is_sealed: *const (),
    pub is_extensible: *const (),
    pub get_own_property_names: *const (),
    pub define_properties: *const (),
    pub from_entries: *const (),
    pub get_prototype_of: *const (),
    pub has_own: *const (),
    pub entries: *const (),
    pub values: *const (),
    pub has_own_property: *const (),
    pub property_is_enumerable: *const (),
}

pub struct ArrayMethods {
    pub is_array: *const (),
    pub push: *const (),
    pub pop: *const (),
    pub slice: *const (),
    pub splice: *const (),
    pub concat: *const (),
    pub join: *const (),
    pub index_of: *const (),
    pub includes: *const (),
    pub reverse: *const (),
    pub for_each: *const (),
    pub map: *const (),
    pub filter: *const (),
    pub reduce: *const (),
    pub find: *const (),
    pub some: *const (),
    pub every: *const (),
    pub flat: *const (),
    pub flat_map: *const (),
    pub shift: *const (),
    pub unshift: *const (),
    pub fill: *const (),
    pub copy_within: *const (),
    pub at: *const (),
    pub last_index_of: *const (),
    pub find_index: *const (),
    pub find_last: *const (),
    pub reduce_right: *const (),
    pub sort: *const (),
}

pub struct ErrorMethods {
    pub error: *const (),
    pub type_error: *const (),
    pub reference_error: *const (),
    pub range_error: *const (),
    pub syntax_error: *const (),
    pub uri_error: *const (),
    pub eval_error: *const (),
    pub to_string: *const (),
    pub stack: *const (),
}

pub struct StringMethods {
    pub index_of: *const (),
    pub includes: *const (),
    pub char_at: *const (),
    pub char_code_at: *const (),
    pub concat: *const (),
    pub slice: *const (),
    pub substring: *const (),
    pub to_upper_case: *const (),
    pub to_lower_case: *const (),
    pub trim: *const (),
    pub repeat: *const (),
    pub pad_start: *const (),
    pub pad_end: *const (),
    pub starts_with: *const (),
    pub ends_with: *const (),
    pub split: *const (),
    pub replace: *const (),
    pub match_fn: *const (),
    pub search: *const (),
    pub trim_start: *const (),
    pub trim_end: *const (),
    pub code_point_at: *const (),
    pub normalize: *const (),
    pub match_all: *const (),
    pub replace_all: *const (),
    pub value_of: *const (),
}

pub struct RegExpMethods {
    pub exec: *const (),
    pub test: *const (),
    pub to_string: *const (),
}

pub struct NumberMethods {
    pub is_nan: *const (),
    pub is_finite: *const (),
    pub parse_int: *const (),
    pub parse_float: *const (),
    pub to_string: *const (),
    pub to_fixed: *const (),
    pub is_integer: *const (),
    pub is_safe_integer: *const (),
    pub to_exponential: *const (),
    pub to_precision: *const (),
    pub value_of: *const (),
}

pub struct FunctionMethods {
    pub call: *const (),
    pub apply: *const (),
    pub bind: *const (),
    pub to_string: *const (),
}

pub struct BuiltinWorld {
    pub object_proto: P<JsObject>,
    pub array_proto: P<JsObject>,
    pub function_proto: P<JsObject>,
    pub string_proto: P<JsObject>,
    pub number_proto: P<JsObject>,
    pub boolean_proto: P<JsObject>,
    pub error_proto: P<JsObject>,
    pub symbol_proto: P<JsObject>,
    pub object_constructor: P<JsObject>,
    pub array_constructor: P<JsObject>,
    pub function_constructor: P<JsObject>,
    pub string_constructor: P<JsObject>,
    pub number_constructor: P<JsObject>,
    pub boolean_constructor: P<JsObject>,
    pub error_constructor: P<JsObject>,
    pub symbol_constructor: P<JsObject>,
    pub type_error_proto: P<JsObject>,
    pub reference_error_proto: P<JsObject>,
    pub range_error_proto: P<JsObject>,
    pub syntax_error_proto: P<JsObject>,
    pub uri_error_proto: P<JsObject>,
    pub eval_error_proto: P<JsObject>,
    pub math_object: P<JsObject>,
    pub json_object: P<JsObject>,
    pub date_constructor: P<JsObject>,
    pub date_proto: P<JsObject>,
    pub set_constructor: P<JsObject>,
    pub set_proto: P<JsObject>,
    pub map_constructor: P<JsObject>,
    pub map_proto: P<JsObject>,
    pub regexp_constructor: P<JsObject>,
    pub regexp_proto: P<JsObject>,
    pub sym_match: P<JsObject>,
    pub sym_replace: P<JsObject>,
    pub sym_search: P<JsObject>,
    pub sym_split: P<JsObject>,
    pub sym_iterator: P<JsObject>,
    pub stub_objects: Vec<P<JsObject>>,
}

fn intern_label(string_forge: &StringForge, label: &str) -> u32 {
    string_forge.intern(label).0
}

fn make_pair(
    string_forge: &StringForge, shape_forge: &ShapeForge, name: &str, si_prototype: u32, si_constructor: u32,
    si_name: u32,
) -> (P<JsObject>, P<JsObject>) {
    intern_label(string_forge, name);

    let mut proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());

    let proto_shape = shape_forge.make_shape(EMPTY_SHAPE_ID, si_constructor);
    proto.set_shape_id(proto_shape);
    // proto shape: EMPTY -> "constructor" (slot 0)

    let ctor_shape1 = shape_forge.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let ctor_shape2 = shape_forge.make_shape(ctor_shape1, si_name);
    ctor.set_shape_id(ctor_shape2);
    // ctor shape: EMPTY -> "prototype" (slot 0) -> "name" (slot 1)
    ctor.set_function(true);

    // Pre-allocate Vec slots so wire_ctor_proto can overwrite vec[0] later.
    // Proto: 1 property ("constructor"), Ctor: 2 properties ("prototype", "name").
    proto.ensure_hash_props().push(JsValue::undefined()); // slot 0: "constructor" placeholder
    ctor.ensure_hash_props().push(JsValue::undefined()); // slot 0: "prototype" placeholder
    ctor.ensure_hash_props().push(JsValue::undefined()); // slot 1: "name" placeholder

    (P::new(proto), P::new(ctor))
}

impl BuiltinWorld {
    fn fn_proto_val(&self) -> JsValue {
        JsValue::from_js_object(self.function_proto.as_ptr() as *mut JsObject)
    }

    pub fn new(string_forge: &StringForge, shape_forge: &ShapeForge) -> Self {
        let si_prototype = intern_label(string_forge, "prototype");
        let si_constructor = intern_label(string_forge, "constructor");
        let si_name = intern_label(string_forge, "name");
        intern_label(string_forge, "length");
        intern_label(string_forge, "toString");
        intern_label(string_forge, "valueOf");

        let (object_proto, object_constructor) =
            make_pair(string_forge, shape_forge, "Object", si_prototype, si_constructor, si_name);
        let (array_proto, array_constructor) =
            make_pair(string_forge, shape_forge, "Array", si_prototype, si_constructor, si_name);
        let (function_proto, function_constructor) =
            make_pair(string_forge, shape_forge, "Function", si_prototype, si_constructor, si_name);
        let (string_proto, string_constructor) =
            make_pair(string_forge, shape_forge, "String", si_prototype, si_constructor, si_name);
        let (number_proto, number_constructor) =
            make_pair(string_forge, shape_forge, "Number", si_prototype, si_constructor, si_name);
        let (boolean_proto, boolean_constructor) =
            make_pair(string_forge, shape_forge, "Boolean", si_prototype, si_constructor, si_name);
        let (error_proto, error_constructor) =
            make_pair(string_forge, shape_forge, "Error", si_prototype, si_constructor, si_name);
        let (symbol_proto, symbol_constructor) =
            make_pair(string_forge, shape_forge, "Symbol", si_prototype, si_constructor, si_name);

        let error_proto_val = JsValue::from_js_object(error_proto.as_ptr() as *mut JsObject);
        let type_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let reference_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let range_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let syntax_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let uri_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let eval_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));

        let math_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let json_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let (date_proto, date_constructor) =
            make_pair(string_forge, shape_forge, "Date", si_prototype, si_constructor, si_name);
        let (set_proto, set_constructor) =
            make_pair(string_forge, shape_forge, "Set", si_prototype, si_constructor, si_name);
        let (map_proto, map_constructor) =
            make_pair(string_forge, shape_forge, "Map", si_prototype, si_constructor, si_name);
        let (regexp_proto, regexp_constructor) =
            make_pair(string_forge, shape_forge, "RegExp", si_prototype, si_constructor, si_name);

        let sym_match = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_replace = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_search = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_split = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_iterator = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let stub_objects = Vec::new();

        /// Overwrite placeholder slots (set up by make_pair) with real values.
        /// ctor.vec[0] = constructor.prototype -> proto
        /// proto.vec[0] = proto.constructor -> ctor
        fn wire_ctor_proto(ctor: &P<JsObject>, proto: &P<JsObject>) {
            let ctor_ptr = ctor.as_ptr() as *mut JsObject;
            let ctor = unsafe { &mut *ctor_ptr };
            let vec = ctor.ensure_hash_props();
            if !vec.is_empty() {
                vec[0] = JsValue::from_js_object(proto.as_ptr() as *mut JsObject);
            }
            let proto_ptr = proto.as_ptr() as *mut JsObject;
            let proto = unsafe { &mut *proto_ptr };
            let pvec = proto.ensure_hash_props();
            if !pvec.is_empty() {
                pvec[0] = JsValue::from_js_object(ctor_ptr);
            }
        }

        wire_ctor_proto(&object_constructor, &object_proto);
        wire_ctor_proto(&array_constructor, &array_proto);
        wire_ctor_proto(&function_constructor, &function_proto);
        wire_ctor_proto(&string_constructor, &string_proto);
        wire_ctor_proto(&number_constructor, &number_proto);
        wire_ctor_proto(&boolean_constructor, &boolean_proto);
        wire_ctor_proto(&error_constructor, &error_proto);
        wire_ctor_proto(&symbol_constructor, &symbol_proto);
        wire_ctor_proto(&date_constructor, &date_proto);
        wire_ctor_proto(&set_constructor, &set_proto);
        wire_ctor_proto(&map_constructor, &map_proto);

        // Wire prototype chains: all constructor prototypes inherit from Object.prototype.
        let obj_proto_val = JsValue::from_js_object(object_proto.as_ptr() as *mut JsObject);
        let non_object_protos: [&P<JsObject>; 11] = [
            &array_proto,
            &function_proto,
            &string_proto,
            &number_proto,
            &boolean_proto,
            &error_proto,
            &symbol_proto,
            &date_proto,
            &set_proto,
            &map_proto,
            &regexp_proto,
        ];
        for proto in &non_object_protos {
            let p = proto.as_ptr() as *mut JsObject;
            unsafe { &mut *p }.set_proto(obj_proto_val).ok();
        }
        wire_ctor_proto(&regexp_constructor, &regexp_proto);

        Self {
            object_proto,
            array_proto,
            function_proto,
            string_proto,
            number_proto,
            boolean_proto,
            error_proto,
            symbol_proto,
            object_constructor,
            array_constructor,
            function_constructor,
            string_constructor,
            number_constructor,
            boolean_constructor,
            error_constructor,
            symbol_constructor,
            type_error_proto,
            reference_error_proto,
            range_error_proto,
            syntax_error_proto,
            uri_error_proto,
            eval_error_proto,
            math_object,
            json_object,
            date_constructor,
            date_proto,
            set_constructor,
            set_proto,
            map_constructor,
            map_proto,
            regexp_constructor,
            regexp_proto,
            sym_match,
            sym_replace,
            sym_search,
            sym_split,
            sym_iterator,
            stub_objects,
        }
    }

    pub fn bind_object_methods(&self, methods: &ObjectMethods, string_forge: &StringForge, shape_forge: &ShapeForge) {
        let ctor_ptr = P::as_ptr(&self.object_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(
            self,
            ctor,
            string_forge,
            shape_forge,
            ("keys", methods.keys, 1),
            ("create", methods.create, 2),
            ("assign", methods.assign, 2),
            ("is", methods.is, 2),
            ("defineProperty", methods.define_property, 3),
            ("getOwnPropertyDescriptor", methods.get_own_property_descriptor, 2),
            ("freeze", methods.freeze, 1),
            ("seal", methods.seal, 1),
            ("preventExtensions", methods.prevent_extensions, 1),
            ("isFrozen", methods.is_frozen, 1),
            ("isSealed", methods.is_sealed, 1),
            ("isExtensible", methods.is_extensible, 1),
            ("getOwnPropertyNames", methods.get_own_property_names, 1),
            ("defineProperties", methods.define_properties, 2),
            ("fromEntries", methods.from_entries, 1),
            ("getPrototypeOf", methods.get_prototype_of, 1),
            ("hasOwn", methods.has_own, 2),
            ("entries", methods.entries, 1),
            ("values", methods.values, 1),
        );

        let proto_ptr = P::as_ptr(&self.object_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            self,
            proto,
            string_forge,
            shape_forge,
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

    pub fn bind_array_methods(&self, methods: &ArrayMethods, string_forge: &StringForge, shape_forge: &ShapeForge) {
        let ctor_ptr = P::as_ptr(&self.array_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(self, ctor, string_forge, shape_forge, ("isArray", methods.is_array, 1),);

        let proto_ptr = P::as_ptr(&self.array_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            self,
            proto,
            string_forge,
            shape_forge,
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
        );
    }

    pub fn bind_error_methods(&self, methods: &ErrorMethods, string_forge: &StringForge, shape_forge: &ShapeForge) {
        let ctor_ptr = P::as_ptr(&self.error_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(
            self,
            ctor,
            string_forge,
            shape_forge,
            ("Error", methods.error, 1),
            ("TypeError", methods.type_error, 1),
            ("ReferenceError", methods.reference_error, 1),
            ("RangeError", methods.range_error, 1),
            ("SyntaxError", methods.syntax_error, 1),
            ("URIError", methods.uri_error, 1),
            ("EvalError", methods.eval_error, 1),
        );

        let proto_ptr = P::as_ptr(&self.error_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            self,
            proto,
            string_forge,
            shape_forge,
            ("toString", methods.to_string, 0),
            ("stack", methods.stack, 0),
        );
    }

    pub fn bind_string_methods(&self, methods: &StringMethods, string_forge: &StringForge, shape_forge: &ShapeForge) {
        let proto_ptr = P::as_ptr(&self.string_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            self,
            proto,
            string_forge,
            shape_forge,
            ("indexOf", methods.index_of, 1),
            ("includes", methods.includes, 1),
            ("charAt", methods.char_at, 1),
            ("charCodeAt", methods.char_code_at, 1),
            ("concat", methods.concat, 1),
            ("slice", methods.slice, 2),
            ("substring", methods.substring, 2),
            ("toUpperCase", methods.to_upper_case, 0),
            ("toLowerCase", methods.to_lower_case, 0),
            ("trim", methods.trim, 0),
            ("repeat", methods.repeat, 1),
            ("padStart", methods.pad_start, 1),
            ("padEnd", methods.pad_end, 1),
            ("startsWith", methods.starts_with, 1),
            ("endsWith", methods.ends_with, 1),
            ("split", methods.split, 1),
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
        );
    }

    pub fn bind_number_methods(&self, methods: &NumberMethods, string_forge: &StringForge, shape_forge: &ShapeForge) {
        let ctor_ptr = P::as_ptr(&self.number_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        let proto_ptr = P::as_ptr(&self.number_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        bind_methods!(
            self,
            ctor,
            string_forge,
            shape_forge,
            ("isNaN", methods.is_nan, 1),
            ("isFinite", methods.is_finite, 1),
            ("parseInt", methods.parse_int, 1),
            ("parseFloat", methods.parse_float, 1),
            ("isInteger", methods.is_integer, 1),
            ("isSafeInteger", methods.is_safe_integer, 1),
        );
        bind_methods!(
            self,
            proto,
            string_forge,
            shape_forge,
            ("toString", methods.to_string, 0),
            ("toFixed", methods.to_fixed, 0),
            ("toExponential", methods.to_exponential, 0),
            ("toPrecision", methods.to_precision, 0),
            ("valueOf", methods.value_of, 0),
        );
    }

    pub fn bind_function_methods(
        &self, methods: &FunctionMethods, string_forge: &StringForge, shape_forge: &ShapeForge,
    ) {
        let proto_ptr = P::as_ptr(&self.function_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        let fp = self.fn_proto_val();
        bind_methods_static!(
            proto,
            string_forge,
            shape_forge,
            fp,
            ("call", methods.call, 1),
            ("apply", methods.apply, 2),
            ("bind", methods.bind, 1),
            ("toString", methods.to_string, 0),
        );
    }

    pub fn bind_method(
        &self, proto: &mut JsObject, shape_forge: &ShapeForge, string_forge: &StringForge, method_name: &str,
        native_fn_ptr: NativeFnPtr, arg_count: u8,
    ) -> Result<(), String> {
        Self::bind_method_static(
            proto,
            shape_forge,
            string_forge,
            method_name,
            native_fn_ptr,
            arg_count,
            self.fn_proto_val(),
        )
    }

    pub fn bind_method_static(
        proto: &mut JsObject, shape_forge: &ShapeForge, string_forge: &StringForge, method_name: &str,
        native_fn_ptr: NativeFnPtr, arg_count: u8, wrapper_proto: JsValue,
    ) -> Result<(), String> {
        let si = string_forge.intern(method_name).0;
        let wrapper_proto_ptr = if wrapper_proto.is_object() {
            wrapper_proto.as_js_object_ptr()
        } else {
            std::ptr::null_mut()
        };
        let mut wrapper = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        if !wrapper_proto_ptr.is_null() {
            wrapper.set_proto(wrapper_proto).ok();
        }
        wrapper.set_function(true);
        // The NativeFnPtr invariant is upheld by callers (see bind_method / bind_method_static
        // callers, all of which use fn-item expressions).
        wrapper.set_native_fn(Some(native_fn_ptr));
        wrapper.set_native_arg_count(arg_count);
        let si_name = string_forge.intern("name").0;
        let name_shape = shape_forge.make_shape(wrapper.shape_id(), si_name);
        wrapper.set_shape_id(name_shape);
        wrapper
            .ensure_hash_props()
            .push(JsValue::string(string_forge.intern(method_name).0, 0));
        let wrapper_val = JsValue::from_js_object(Box::into_raw(wrapper));
        let new_shape = shape_forge.make_shape(proto.shape_id(), si);
        proto.set_shape_id(new_shape);
        proto.ensure_hash_props().push(wrapper_val);
        proto.bump_generation();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
    use crate::string_forge::StringForge;

    fn make_world() -> BuiltinWorld {
        let sf = StringForge::new();
        let sh = ShapeForge::new();
        BuiltinWorld::new(&sf, &sh)
    }

    #[test]
    fn test_all_protos_valid() {
        let w = make_world();
        let protos = [
            &w.object_proto,
            &w.array_proto,
            &w.function_proto,
            &w.string_proto,
            &w.number_proto,
            &w.boolean_proto,
            &w.error_proto,
            &w.symbol_proto,
        ];
        for p in protos {
            assert!(p.shape_id() > EMPTY_SHAPE_ID, "proto should have a non-empty shape");
        }
    }

    #[test]
    fn test_all_constructors_valid() {
        let w = make_world();
        assert!(w.object_constructor.is_function());
        assert!(w.array_constructor.is_function());
        assert!(w.function_constructor.is_function());
        assert!(w.string_constructor.is_function());
        assert!(w.number_constructor.is_function());
        assert!(w.boolean_constructor.is_function());
        assert!(w.error_constructor.is_function());
        assert!(w.symbol_constructor.is_function());
    }

    #[test]
    fn test_prototypes_are_not_functions() {
        let w = make_world();
        assert!(!w.object_proto.is_function());
        assert!(!w.array_proto.is_function());
        assert!(!w.function_proto.is_function());
        assert!(!w.string_proto.is_function());
        assert!(!w.number_proto.is_function());
        assert!(!w.boolean_proto.is_function());
        assert!(!w.error_proto.is_function());
        assert!(!w.symbol_proto.is_function());
    }

    #[test]
    fn test_protos_have_null_proto() {
        let w = make_world();
        // Object.prototype is the root - its __proto__ is null.
        assert!(w.object_proto.proto().is_null());
        // All other constructor prototypes inherit from Object.prototype.
        assert!(w.array_proto.proto().is_object());
        assert!(w.function_proto.proto().is_object());
        assert!(w.string_proto.proto().is_object());
        assert!(w.number_proto.proto().is_object());
        assert!(w.boolean_proto.proto().is_object());
        assert!(w.error_proto.proto().is_object());
        assert!(w.symbol_proto.proto().is_object());
    }

    #[test]
    fn test_shapes_populated() {
        let w = make_world();
        assert!(
            w.object_constructor.shape_id() > EMPTY_SHAPE_ID,
            "constructor should have prototype + name shape"
        );
        assert!(w.object_proto.shape_id() > EMPTY_SHAPE_ID, "prototype should have constructor shape");
    }
}
