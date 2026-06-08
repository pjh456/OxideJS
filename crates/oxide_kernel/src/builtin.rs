use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::StringForge;

#[macro_export]
macro_rules! bind_method {
    ($world:expr, $target:expr, $sf:expr, $sh:expr, $name:literal, $func:path, $nargs:literal) => {
        let _ = $world.bind_method($target, $sh, $sf, $name, $func as *const (), $nargs);
    };
}

#[macro_export]
macro_rules! bind_methods {
    ($world:expr, $target:expr, $sf:expr, $sh:expr,
     $(($name:literal, $func:path, $nargs:literal)),* $(,)?) => {
        $( bind_method!($world, $target, $sf, $sh, $name, $func, $nargs); )*
    };
}

pub struct ObjectMethods {
    pub keys: *const (),
    pub create: *const (),
    pub assign: *const (),
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
}

pub struct ArrayMethods {
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
}

pub struct FunctionMethods {
    pub call: *const (),
    pub apply: *const (),
    pub bind: *const (),
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
}

fn intern_label(string_forge: &StringForge, label: &str) -> u32 {
    string_forge.intern(label).0
}

fn make_pair(
    string_forge: &StringForge,
    shape_forge: &ShapeForge,
    name: &str,
    si_prototype: u32,
    si_constructor: u32,
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
    proto
        .ensure_hash_props()
        .push(Box::new(JsValue::undefined())); // slot 0: "constructor" placeholder
    ctor.ensure_hash_props()
        .push(Box::new(JsValue::undefined())); // slot 0: "prototype" placeholder
    ctor.ensure_hash_props()
        .push(Box::new(JsValue::undefined())); // slot 1: "name" placeholder

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

        let (object_proto, object_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Object",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (array_proto, array_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Array",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (function_proto, function_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Function",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (string_proto, string_constructor) = make_pair(
            string_forge,
            shape_forge,
            "String",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (number_proto, number_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Number",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (boolean_proto, boolean_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Boolean",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (error_proto, error_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Error",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (symbol_proto, symbol_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Symbol",
            si_prototype,
            si_constructor,
            si_name,
        );

        let error_proto_val = JsValue::from_js_object(error_proto.as_ptr() as *mut JsObject);
        let type_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let reference_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let range_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let syntax_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let uri_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let eval_error_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));

        let math_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let json_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let (date_proto, date_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Date",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (set_proto, set_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Set",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (map_proto, map_constructor) = make_pair(
            string_forge,
            shape_forge,
            "Map",
            si_prototype,
            si_constructor,
            si_name,
        );
        let (regexp_proto, regexp_constructor) = make_pair(
            string_forge,
            shape_forge,
            "RegExp",
            si_prototype,
            si_constructor,
            si_name,
        );

        let sym_match = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_replace = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_search = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_split = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        /// Overwrite placeholder slots (set up by make_pair) with real values.
        /// ctor.vec[0] = constructor.prototype -> proto
        /// proto.vec[0] = proto.constructor -> ctor
        fn wire_ctor_proto(ctor: &P<JsObject>, proto: &P<JsObject>) {
            let ctor_ptr = ctor.as_ptr() as *mut JsObject;
            let ctor = unsafe { &mut *ctor_ptr };
            let vec = ctor.ensure_hash_props();
            if !vec.is_empty() {
                *vec[0] = JsValue::from_js_object(proto.as_ptr() as *mut JsObject);
            }
            let proto_ptr = proto.as_ptr() as *mut JsObject;
            let proto = unsafe { &mut *proto_ptr };
            let pvec = proto.ensure_hash_props();
            if !pvec.is_empty() {
                *pvec[0] = JsValue::from_js_object(ctor_ptr);
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
        }
    }

    pub fn bind_object_methods(
        &self,
        methods: &ObjectMethods,
        string_forge: &StringForge,
        shape_forge: &ShapeForge,
    ) {
        let ctor_ptr = P::as_ptr(&self.object_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };

        let _ = self.bind_method(ctor, shape_forge, string_forge, "keys", methods.keys, 1);
        let _ = self.bind_method(ctor, shape_forge, string_forge, "create", methods.create, 2);
        let _ = self.bind_method(ctor, shape_forge, string_forge, "assign", methods.assign, 2);
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "defineProperty",
            methods.define_property,
            3,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "getOwnPropertyDescriptor",
            methods.get_own_property_descriptor,
            2,
        );
        let _ = self.bind_method(ctor, shape_forge, string_forge, "freeze", methods.freeze, 1);
        let _ = self.bind_method(ctor, shape_forge, string_forge, "seal", methods.seal, 1);
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "preventExtensions",
            methods.prevent_extensions,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "isFrozen",
            methods.is_frozen,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "isSealed",
            methods.is_sealed,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "isExtensible",
            methods.is_extensible,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "getOwnPropertyNames",
            methods.get_own_property_names,
            1,
        );
        let _ = self.bind_method(ctor, shape_forge, string_forge, "defineProperties", methods.define_properties, 2);
        let _ = self.bind_method(ctor, shape_forge, string_forge, "fromEntries", methods.from_entries, 1);
        let _ = self.bind_method(ctor, shape_forge, string_forge, "getPrototypeOf", methods.get_prototype_of, 1);
        let _ = self.bind_method(ctor, shape_forge, string_forge, "hasOwn", methods.has_own, 2);
    }

    pub fn bind_array_methods(
        &self,
        methods: &ArrayMethods,
        string_forge: &StringForge,
        shape_forge: &ShapeForge,
    ) {
        let proto_ptr = P::as_ptr(&self.array_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };

        let _ = self.bind_method(proto, shape_forge, string_forge, "push", methods.push, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "pop", methods.pop, 0);
        let _ = self.bind_method(proto, shape_forge, string_forge, "slice", methods.slice, 2);
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "splice",
            methods.splice,
            2,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "concat",
            methods.concat,
            1,
        );
        let _ = self.bind_method(proto, shape_forge, string_forge, "join", methods.join, 1);
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "indexOf",
            methods.index_of,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "includes",
            methods.includes,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "reverse",
            methods.reverse,
            0,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "forEach",
            methods.for_each,
            1,
        );
        let _ = self.bind_method(proto, shape_forge, string_forge, "map", methods.map, 1);
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "filter",
            methods.filter,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "reduce",
            methods.reduce,
            1,
        );
        let _ = self.bind_method(proto, shape_forge, string_forge, "find", methods.find, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "some", methods.some, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "every", methods.every, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "flat", methods.flat, 0);
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "flatMap",
            methods.flat_map,
            1,
        );
        let _ = self.bind_method(proto, shape_forge, string_forge, "shift", methods.shift, 0);
        let _ = self.bind_method(proto, shape_forge, string_forge, "unshift", methods.unshift, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "fill", methods.fill, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "copyWithin", methods.copy_within, 2);
        let _ = self.bind_method(proto, shape_forge, string_forge, "at", methods.at, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "lastIndexOf", methods.last_index_of, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "findIndex", methods.find_index, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "findLast", methods.find_last, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "reduceRight", methods.reduce_right, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "sort", methods.sort, 0);
    }

    pub fn bind_error_methods(
        &self,
        methods: &ErrorMethods,
        string_forge: &StringForge,
        shape_forge: &ShapeForge,
    ) {
        let ctor_ptr = P::as_ptr(&self.error_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };

        let _ = self.bind_method(ctor, shape_forge, string_forge, "Error", methods.error, 1);
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "TypeError",
            methods.type_error,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "ReferenceError",
            methods.reference_error,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "RangeError",
            methods.range_error,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "SyntaxError",
            methods.syntax_error,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "URIError",
            methods.uri_error,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "EvalError",
            methods.eval_error,
            1,
        );
    }

    pub fn bind_string_methods(
        &self,
        methods: &StringMethods,
        string_forge: &StringForge,
        shape_forge: &ShapeForge,
    ) {
        let proto_ptr = P::as_ptr(&self.string_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };

        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "indexOf",
            methods.index_of,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "includes",
            methods.includes,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "charAt",
            methods.char_at,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "charCodeAt",
            methods.char_code_at,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "concat",
            methods.concat,
            1,
        );
        let _ = self.bind_method(proto, shape_forge, string_forge, "slice", methods.slice, 2);
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "substring",
            methods.substring,
            2,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "toUpperCase",
            methods.to_upper_case,
            0,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "toLowerCase",
            methods.to_lower_case,
            0,
        );
        let _ = self.bind_method(proto, shape_forge, string_forge, "trim", methods.trim, 0);
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "repeat",
            methods.repeat,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "padStart",
            methods.pad_start,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "padEnd",
            methods.pad_end,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "startsWith",
            methods.starts_with,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "endsWith",
            methods.ends_with,
            1,
        );
        let _ = self.bind_method(proto, shape_forge, string_forge, "split", methods.split, 1);
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "replace",
            methods.replace,
            2,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "match",
            methods.match_fn,
            1,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "search",
            methods.search,
            1,
        );
        let _ = self.bind_method(proto, shape_forge, string_forge, "trimStart", methods.trim_start, 0);
        let _ = self.bind_method(proto, shape_forge, string_forge, "trimEnd", methods.trim_end, 0);
        let _ = self.bind_method(proto, shape_forge, string_forge, "codePointAt", methods.code_point_at, 1);
        let _ = self.bind_method(proto, shape_forge, string_forge, "normalize", methods.normalize, 0);
    }

    pub fn bind_number_methods(
        &self,
        methods: &NumberMethods,
        string_forge: &StringForge,
        shape_forge: &ShapeForge,
    ) {
        let ctor_ptr = P::as_ptr(&self.number_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        let proto_ptr = P::as_ptr(&self.number_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };

        let _ = self.bind_method(ctor, shape_forge, string_forge, "isNaN", methods.is_nan, 1);
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "isFinite",
            methods.is_finite,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "parseInt",
            methods.parse_int,
            1,
        );
        let _ = self.bind_method(
            ctor,
            shape_forge,
            string_forge,
            "parseFloat",
            methods.parse_float,
            1,
        );

        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "toString",
            methods.to_string,
            0,
        );
        let _ = self.bind_method(
            proto,
            shape_forge,
            string_forge,
            "toFixed",
            methods.to_fixed,
            0,
        );
    }

    pub fn bind_function_methods(
        &self,
        methods: &FunctionMethods,
        string_forge: &StringForge,
        shape_forge: &ShapeForge,
    ) {
        let proto_ptr = P::as_ptr(&self.function_proto) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        let fp = self.fn_proto_val();

        let _ = Self::bind_method_static(
            proto,
            shape_forge,
            string_forge,
            "call",
            methods.call,
            1,
            fp,
        );
        let _ = Self::bind_method_static(
            proto,
            shape_forge,
            string_forge,
            "apply",
            methods.apply,
            2,
            fp,
        );
        let _ = Self::bind_method_static(
            proto,
            shape_forge,
            string_forge,
            "bind",
            methods.bind,
            1,
            fp,
        );
    }

    pub fn bind_method(
        &self,
        proto: &mut JsObject,
        shape_forge: &ShapeForge,
        string_forge: &StringForge,
        method_name: &str,
        native_fn_ptr: *const (),
        arg_count: u8,
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
        proto: &mut JsObject,
        shape_forge: &ShapeForge,
        string_forge: &StringForge,
        method_name: &str,
        native_fn_ptr: *const (),
        arg_count: u8,
        wrapper_proto: JsValue,
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
        wrapper.set_native_fn(Some(native_fn_ptr));
        wrapper.set_native_arg_count(arg_count);
        let wrapper_val = JsValue::from_js_object(Box::into_raw(wrapper));
        let new_shape = shape_forge.make_shape(proto.shape_id(), si);
        proto.set_shape_id(new_shape);
        proto.ensure_hash_props().push(Box::new(wrapper_val));
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
            assert!(
                p.shape_id() > EMPTY_SHAPE_ID,
                "proto should have a non-empty shape"
            );
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
        assert!(w.object_proto.proto().is_null());
        assert!(w.array_proto.proto().is_null());
        assert!(w.function_proto.proto().is_null());
        assert!(w.string_proto.proto().is_null());
        assert!(w.number_proto.proto().is_null());
        assert!(w.boolean_proto.proto().is_null());
        assert!(w.error_proto.proto().is_null());
        assert!(w.symbol_proto.proto().is_null());
    }

    #[test]
    fn test_shapes_populated() {
        let w = make_world();
        assert!(
            w.object_constructor.shape_id() > EMPTY_SHAPE_ID,
            "constructor should have prototype + name shape"
        );
        assert!(
            w.object_proto.shape_id() > EMPTY_SHAPE_ID,
            "prototype should have constructor shape"
        );
    }
}
