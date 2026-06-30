use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::kernel::{BuiltinDirtySet, BuiltinId};
use crate::kernel_info;
use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::PermInterner;

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
    pub values: *const (),
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
    pub from_char_code: *const (),
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
    pub substr: *const (),
    pub at: *const (),
    pub last_index_of: *const (),
}

pub struct RegExpMethods {
    pub exec: *const (),
    pub test: *const (),
    pub to_string: *const (),
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
    pub array_buffer_constructor: P<JsObject>,
    pub array_buffer_proto: P<JsObject>,
    pub data_view_constructor: P<JsObject>,
    pub data_view_proto: P<JsObject>,
    pub typed_array_proto: P<JsObject>,
    pub int8array_constructor: P<JsObject>,
    pub int8array_proto: P<JsObject>,
    pub uint8array_constructor: P<JsObject>,
    pub uint8array_proto: P<JsObject>,
    pub uint8clampedarray_constructor: P<JsObject>,
    pub uint8clampedarray_proto: P<JsObject>,
    pub int16array_constructor: P<JsObject>,
    pub int16array_proto: P<JsObject>,
    pub uint16array_constructor: P<JsObject>,
    pub uint16array_proto: P<JsObject>,
    pub int32array_constructor: P<JsObject>,
    pub int32array_proto: P<JsObject>,
    pub uint32array_constructor: P<JsObject>,
    pub uint32array_proto: P<JsObject>,
    pub float32array_constructor: P<JsObject>,
    pub float32array_proto: P<JsObject>,
    pub float64array_constructor: P<JsObject>,
    pub float64array_proto: P<JsObject>,
    pub bigint64array_constructor: P<JsObject>,
    pub bigint64array_proto: P<JsObject>,
    pub biguint64array_constructor: P<JsObject>,
    pub biguint64array_proto: P<JsObject>,
    pub sym_match: P<JsObject>,
    pub sym_replace: P<JsObject>,
    pub sym_search: P<JsObject>,
    pub sym_split: P<JsObject>,
    pub sym_iterator: P<JsObject>,
    pub sym_to_primitive: P<JsObject>,
    pub sym_has_instance: P<JsObject>,
    pub stub_objects: Vec<P<JsObject>>,
}

fn intern_label(string_forge: &PermInterner, label: &str) -> u32 {
    string_forge.intern(label).0
}

fn make_pair(
    string_forge: &PermInterner, shape_forge: &ShapeForge, name: &str, si_prototype: u32, si_constructor: u32,
    si_name: u32,
) -> (P<JsObject>, P<JsObject>) {
    intern_label(string_forge, name);
    let name_si = string_forge.intern(name).0; // intern the constructor's name value too

    let mut proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());

    let proto_shape = shape_forge.make_shape(EMPTY_SHAPE_ID, si_constructor);
    proto.set_shape_id(proto_shape);

    let ctor_shape1 = shape_forge.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let ctor_shape2 = shape_forge.make_shape(ctor_shape1, si_name);
    ctor.set_shape_id(ctor_shape2);
    ctor.set_function(true);

    // Pre-allocate slots: proto[0]="constructor", ctor[0]="prototype", ctor[1]="name"
    proto.ensure_hash_props().push(JsValue::undefined()); // placeholder
    ctor.ensure_hash_props().push(JsValue::undefined()); // placeholder for "prototype"
    // Set the actual name value immediately (ctor vec[1])
    ctor.ensure_hash_props().push(JsValue::perm_string(string_forge.string_ptr(name_si)));
    // Set attribute metadata: .prototype (ctor[0]) = writable:false, enumerable:false, configurable:false
    ctor.set_data_meta(0u32, oxide_types::object::PropAttributes::new(false, false, false));
    // .name (ctor[1]) = writable:false, enumerable:false, configurable:true
    ctor.set_data_meta(1u32, oxide_types::object::PropAttributes::new(false, false, true));

    (P::new(proto), P::new(ctor))
}

#[derive(Clone, Copy)]
struct BuiltinLabels {
    prototype: u32,
    constructor: u32,
    name: u32,
}

fn builtin_labels(string_forge: &PermInterner) -> BuiltinLabels {
    let labels = BuiltinLabels {
        prototype: intern_label(string_forge, "prototype"),
        constructor: intern_label(string_forge, "constructor"),
        name: intern_label(string_forge, "name"),
    };
    intern_label(string_forge, "length");
    intern_label(string_forge, "toString");
    intern_label(string_forge, "valueOf");
    labels
}

fn make_named_pair(
    string_forge: &PermInterner, shape_forge: &ShapeForge, labels: BuiltinLabels, name: &str,
) -> (P<JsObject>, P<JsObject>) {
    make_pair(string_forge, shape_forge, name, labels.prototype, labels.constructor, labels.name)
}

fn make_error_subtypes(error_proto: &P<JsObject>) -> ErrorSubtypeProtos {
    let error_proto_val = JsValue::from_js_object(error_proto.as_ptr() as *mut JsObject);
    ErrorSubtypeProtos {
        type_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val)),
        reference_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val)),
        range_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val)),
        syntax_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val)),
        uri_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val)),
        eval_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val)),
    }
}

struct ErrorSubtypeProtos {
    type_error_proto: P<JsObject>,
    reference_error_proto: P<JsObject>,
    range_error_proto: P<JsObject>,
    syntax_error_proto: P<JsObject>,
    uri_error_proto: P<JsObject>,
    eval_error_proto: P<JsObject>,
}

struct TypedArrayFamily {
    typed_array_proto: P<JsObject>,
    int8array_constructor: P<JsObject>,
    int8array_proto: P<JsObject>,
    uint8array_constructor: P<JsObject>,
    uint8array_proto: P<JsObject>,
    uint8clampedarray_constructor: P<JsObject>,
    uint8clampedarray_proto: P<JsObject>,
    int16array_constructor: P<JsObject>,
    int16array_proto: P<JsObject>,
    uint16array_constructor: P<JsObject>,
    uint16array_proto: P<JsObject>,
    int32array_constructor: P<JsObject>,
    int32array_proto: P<JsObject>,
    uint32array_constructor: P<JsObject>,
    uint32array_proto: P<JsObject>,
    float32array_constructor: P<JsObject>,
    float32array_proto: P<JsObject>,
    float64array_constructor: P<JsObject>,
    float64array_proto: P<JsObject>,
    bigint64array_constructor: P<JsObject>,
    bigint64array_proto: P<JsObject>,
    biguint64array_constructor: P<JsObject>,
    biguint64array_proto: P<JsObject>,
}

fn make_typed_array_family(
    string_forge: &PermInterner, shape_forge: &ShapeForge, labels: BuiltinLabels, object_proto: &P<JsObject>,
) -> TypedArrayFamily {
    let obj_proto_val = JsValue::from_js_object(object_proto.as_ptr() as *mut JsObject);
    let typed_array_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, obj_proto_val));
    let (int8array_proto, int8array_constructor) = make_named_pair(string_forge, shape_forge, labels, "Int8Array");
    let (uint8array_proto, uint8array_constructor) = make_named_pair(string_forge, shape_forge, labels, "Uint8Array");
    let (uint8clampedarray_proto, uint8clampedarray_constructor) =
        make_named_pair(string_forge, shape_forge, labels, "Uint8ClampedArray");
    let (int16array_proto, int16array_constructor) = make_named_pair(string_forge, shape_forge, labels, "Int16Array");
    let (uint16array_proto, uint16array_constructor) =
        make_named_pair(string_forge, shape_forge, labels, "Uint16Array");
    let (int32array_proto, int32array_constructor) = make_named_pair(string_forge, shape_forge, labels, "Int32Array");
    let (uint32array_proto, uint32array_constructor) =
        make_named_pair(string_forge, shape_forge, labels, "Uint32Array");
    let (float32array_proto, float32array_constructor) =
        make_named_pair(string_forge, shape_forge, labels, "Float32Array");
    let (float64array_proto, float64array_constructor) =
        make_named_pair(string_forge, shape_forge, labels, "Float64Array");
    let (bigint64array_proto, bigint64array_constructor) =
        make_named_pair(string_forge, shape_forge, labels, "BigInt64Array");
    let (biguint64array_proto, biguint64array_constructor) =
        make_named_pair(string_forge, shape_forge, labels, "BigUint64Array");

    TypedArrayFamily {
        typed_array_proto,
        int8array_constructor,
        int8array_proto,
        uint8array_constructor,
        uint8array_proto,
        uint8clampedarray_constructor,
        uint8clampedarray_proto,
        int16array_constructor,
        int16array_proto,
        uint16array_constructor,
        uint16array_proto,
        int32array_constructor,
        int32array_proto,
        uint32array_constructor,
        uint32array_proto,
        float32array_constructor,
        float32array_proto,
        float64array_constructor,
        float64array_proto,
        bigint64array_constructor,
        bigint64array_proto,
        biguint64array_constructor,
        biguint64array_proto,
    }
}

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

fn set_proto_if_changed(obj: &P<JsObject>, proto: JsValue) {
    let ptr = obj.as_ptr() as *mut JsObject;
    let obj = unsafe { &mut *ptr };
    if obj.proto() != proto {
        obj.set_proto(proto).ok();
    }
}

fn wire_builtin_world_links(world: &BuiltinWorld) {
    wire_ctor_proto(&world.object_constructor, &world.object_proto);
    wire_ctor_proto(&world.array_constructor, &world.array_proto);
    wire_ctor_proto(&world.function_constructor, &world.function_proto);
    wire_ctor_proto(&world.string_constructor, &world.string_proto);
    wire_ctor_proto(&world.number_constructor, &world.number_proto);
    wire_ctor_proto(&world.boolean_constructor, &world.boolean_proto);
    wire_ctor_proto(&world.error_constructor, &world.error_proto);
    wire_ctor_proto(&world.symbol_constructor, &world.symbol_proto);
    wire_ctor_proto(&world.date_constructor, &world.date_proto);
    wire_ctor_proto(&world.set_constructor, &world.set_proto);
    wire_ctor_proto(&world.map_constructor, &world.map_proto);
    wire_ctor_proto(&world.regexp_constructor, &world.regexp_proto);
    wire_ctor_proto(&world.array_buffer_constructor, &world.array_buffer_proto);
    wire_ctor_proto(&world.data_view_constructor, &world.data_view_proto);
    wire_ctor_proto(&world.int8array_constructor, &world.int8array_proto);
    wire_ctor_proto(&world.uint8array_constructor, &world.uint8array_proto);
    wire_ctor_proto(&world.uint8clampedarray_constructor, &world.uint8clampedarray_proto);
    wire_ctor_proto(&world.int16array_constructor, &world.int16array_proto);
    wire_ctor_proto(&world.uint16array_constructor, &world.uint16array_proto);
    wire_ctor_proto(&world.int32array_constructor, &world.int32array_proto);
    wire_ctor_proto(&world.uint32array_constructor, &world.uint32array_proto);
    wire_ctor_proto(&world.float32array_constructor, &world.float32array_proto);
    wire_ctor_proto(&world.float64array_constructor, &world.float64array_proto);
    wire_ctor_proto(&world.bigint64array_constructor, &world.bigint64array_proto);
    wire_ctor_proto(&world.biguint64array_constructor, &world.biguint64array_proto);

    let obj_proto_val = JsValue::from_js_object(world.object_proto.as_ptr() as *mut JsObject);
    let non_object_protos: [&P<JsObject>; 14] = [
        &world.array_proto,
        &world.function_proto,
        &world.string_proto,
        &world.number_proto,
        &world.boolean_proto,
        &world.error_proto,
        &world.symbol_proto,
        &world.date_proto,
        &world.set_proto,
        &world.map_proto,
        &world.regexp_proto,
        &world.array_buffer_proto,
        &world.data_view_proto,
        &world.typed_array_proto,
    ];
    for proto in &non_object_protos {
        set_proto_if_changed(proto, obj_proto_val);
    }

    let typed_array_proto_val = JsValue::from_js_object(world.typed_array_proto.as_ptr() as *mut JsObject);
    let typed_array_protos: [&P<JsObject>; 11] = [
        &world.int8array_proto,
        &world.uint8array_proto,
        &world.uint8clampedarray_proto,
        &world.int16array_proto,
        &world.uint16array_proto,
        &world.int32array_proto,
        &world.uint32array_proto,
        &world.float32array_proto,
        &world.float64array_proto,
        &world.bigint64array_proto,
        &world.biguint64array_proto,
    ];
    for proto in &typed_array_protos {
        set_proto_if_changed(proto, typed_array_proto_val);
    }
}

impl BuiltinWorld {
    fn fn_proto_val(&self) -> JsValue {
        JsValue::from_js_object(self.function_proto.as_ptr() as *mut JsObject)
    }

    pub fn get_by_id(&self, id: BuiltinId) -> &P<JsObject> {
        match id {
            BuiltinId::ObjectProto => &self.object_proto,
            BuiltinId::ArrayProto => &self.array_proto,
            BuiltinId::FunctionProto => &self.function_proto,
            BuiltinId::StringProto => &self.string_proto,
            BuiltinId::NumberProto => &self.number_proto,
            BuiltinId::BooleanProto => &self.boolean_proto,
            BuiltinId::ErrorProto => &self.error_proto,
            BuiltinId::SymbolProto => &self.symbol_proto,
            BuiltinId::ObjectConstructor => &self.object_constructor,
            BuiltinId::ArrayConstructor => &self.array_constructor,
            BuiltinId::FunctionConstructor => &self.function_constructor,
            BuiltinId::StringConstructor => &self.string_constructor,
            BuiltinId::NumberConstructor => &self.number_constructor,
            BuiltinId::BooleanConstructor => &self.boolean_constructor,
            BuiltinId::ErrorConstructor => &self.error_constructor,
            BuiltinId::SymbolConstructor => &self.symbol_constructor,
            BuiltinId::TypeErrorProto => &self.type_error_proto,
            BuiltinId::ReferenceErrorProto => &self.reference_error_proto,
            BuiltinId::RangeErrorProto => &self.range_error_proto,
            BuiltinId::SyntaxErrorProto => &self.syntax_error_proto,
            BuiltinId::UriErrorProto => &self.uri_error_proto,
            BuiltinId::EvalErrorProto => &self.eval_error_proto,
            BuiltinId::MathObject => &self.math_object,
            BuiltinId::JsonObject => &self.json_object,
            BuiltinId::DateConstructor => &self.date_constructor,
            BuiltinId::DateProto => &self.date_proto,
            BuiltinId::SetConstructor => &self.set_constructor,
            BuiltinId::SetProto => &self.set_proto,
            BuiltinId::MapConstructor => &self.map_constructor,
            BuiltinId::MapProto => &self.map_proto,
            BuiltinId::RegExpConstructor => &self.regexp_constructor,
            BuiltinId::RegExpProto => &self.regexp_proto,
            BuiltinId::ArrayBufferConstructor => &self.array_buffer_constructor,
            BuiltinId::ArrayBufferProto => &self.array_buffer_proto,
            BuiltinId::DataViewConstructor => &self.data_view_constructor,
            BuiltinId::DataViewProto => &self.data_view_proto,
            BuiltinId::TypedArrayProto => &self.typed_array_proto,
            BuiltinId::Int8ArrayConstructor => &self.int8array_constructor,
            BuiltinId::Int8ArrayProto => &self.int8array_proto,
            BuiltinId::Uint8ArrayConstructor => &self.uint8array_constructor,
            BuiltinId::Uint8ArrayProto => &self.uint8array_proto,
            BuiltinId::Uint8ClampedArrayConstructor => &self.uint8clampedarray_constructor,
            BuiltinId::Uint8ClampedArrayProto => &self.uint8clampedarray_proto,
            BuiltinId::Int16ArrayConstructor => &self.int16array_constructor,
            BuiltinId::Int16ArrayProto => &self.int16array_proto,
            BuiltinId::Uint16ArrayConstructor => &self.uint16array_constructor,
            BuiltinId::Uint16ArrayProto => &self.uint16array_proto,
            BuiltinId::Int32ArrayConstructor => &self.int32array_constructor,
            BuiltinId::Int32ArrayProto => &self.int32array_proto,
            BuiltinId::Uint32ArrayConstructor => &self.uint32array_constructor,
            BuiltinId::Uint32ArrayProto => &self.uint32array_proto,
            BuiltinId::Float32ArrayConstructor => &self.float32array_constructor,
            BuiltinId::Float32ArrayProto => &self.float32array_proto,
            BuiltinId::Float64ArrayConstructor => &self.float64array_constructor,
            BuiltinId::Float64ArrayProto => &self.float64array_proto,
            BuiltinId::BigInt64ArrayConstructor => &self.bigint64array_constructor,
            BuiltinId::BigInt64ArrayProto => &self.bigint64array_proto,
            BuiltinId::BigUint64ArrayConstructor => &self.biguint64array_constructor,
            BuiltinId::BigUint64ArrayProto => &self.biguint64array_proto,
            BuiltinId::SymMatch => &self.sym_match,
            BuiltinId::SymReplace => &self.sym_replace,
            BuiltinId::SymSearch => &self.sym_search,
            BuiltinId::SymSplit => &self.sym_split,
            BuiltinId::SymIterator => &self.sym_iterator,
            BuiltinId::SymToPrimitive => &self.sym_to_primitive,
            BuiltinId::SymHasInstance => &self.sym_has_instance,
        }
    }

    pub fn new(string_forge: &PermInterner, shape_forge: &ShapeForge) -> Self {
        let labels = builtin_labels(string_forge);

        let (object_proto, object_constructor) = make_named_pair(string_forge, shape_forge, labels, "Object");
        let (array_proto, array_constructor) = make_named_pair(string_forge, shape_forge, labels, "Array");
        let (function_proto, function_constructor) = make_named_pair(string_forge, shape_forge, labels, "Function");
        let (string_proto, string_constructor) = make_named_pair(string_forge, shape_forge, labels, "String");
        let (number_proto, number_constructor) = make_named_pair(string_forge, shape_forge, labels, "Number");
        let (boolean_proto, boolean_constructor) = make_named_pair(string_forge, shape_forge, labels, "Boolean");
        let (error_proto, error_constructor) = make_named_pair(string_forge, shape_forge, labels, "Error");
        let (symbol_proto, symbol_constructor) = make_named_pair(string_forge, shape_forge, labels, "Symbol");

        let error_subtypes = make_error_subtypes(&error_proto);

        let math_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let json_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let (date_proto, date_constructor) = make_named_pair(string_forge, shape_forge, labels, "Date");
        let (set_proto, set_constructor) = make_named_pair(string_forge, shape_forge, labels, "Set");
        let (map_proto, map_constructor) = make_named_pair(string_forge, shape_forge, labels, "Map");
        let (regexp_proto, regexp_constructor) = make_named_pair(string_forge, shape_forge, labels, "RegExp");
        let (array_buffer_proto, array_buffer_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "ArrayBuffer");
        let (data_view_proto, data_view_constructor) = make_named_pair(string_forge, shape_forge, labels, "DataView");
        let typed_arrays = make_typed_array_family(string_forge, shape_forge, labels, &object_proto);

        let sym_match = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_replace = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_search = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_split = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_iterator = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_to_primitive = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_has_instance = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let stub_objects = Vec::new();

        let world = Self {
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
            type_error_proto: error_subtypes.type_error_proto,
            reference_error_proto: error_subtypes.reference_error_proto,
            range_error_proto: error_subtypes.range_error_proto,
            syntax_error_proto: error_subtypes.syntax_error_proto,
            uri_error_proto: error_subtypes.uri_error_proto,
            eval_error_proto: error_subtypes.eval_error_proto,
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
            array_buffer_constructor,
            array_buffer_proto,
            data_view_constructor,
            data_view_proto,
            typed_array_proto: typed_arrays.typed_array_proto,
            int8array_constructor: typed_arrays.int8array_constructor,
            int8array_proto: typed_arrays.int8array_proto,
            uint8array_constructor: typed_arrays.uint8array_constructor,
            uint8array_proto: typed_arrays.uint8array_proto,
            uint8clampedarray_constructor: typed_arrays.uint8clampedarray_constructor,
            uint8clampedarray_proto: typed_arrays.uint8clampedarray_proto,
            int16array_constructor: typed_arrays.int16array_constructor,
            int16array_proto: typed_arrays.int16array_proto,
            uint16array_constructor: typed_arrays.uint16array_constructor,
            uint16array_proto: typed_arrays.uint16array_proto,
            int32array_constructor: typed_arrays.int32array_constructor,
            int32array_proto: typed_arrays.int32array_proto,
            uint32array_constructor: typed_arrays.uint32array_constructor,
            uint32array_proto: typed_arrays.uint32array_proto,
            float32array_constructor: typed_arrays.float32array_constructor,
            float32array_proto: typed_arrays.float32array_proto,
            float64array_constructor: typed_arrays.float64array_constructor,
            float64array_proto: typed_arrays.float64array_proto,
            bigint64array_constructor: typed_arrays.bigint64array_constructor,
            bigint64array_proto: typed_arrays.bigint64array_proto,
            biguint64array_constructor: typed_arrays.biguint64array_constructor,
            biguint64array_proto: typed_arrays.biguint64array_proto,
            sym_match,
            sym_replace,
            sym_search,
            sym_split,
            sym_iterator,
            sym_to_primitive,
            sym_has_instance,
            stub_objects,
        };
        wire_builtin_world_links(&world);
        kernel_info!("BuiltinWorld initialized");
        world
    }

    pub fn rebuild_with_dirty(
        current: &BuiltinWorld, string_forge: &PermInterner, shape_forge: &ShapeForge, dirty: &BuiltinDirtySet,
    ) -> BuiltinWorld {
        let labels = builtin_labels(string_forge);

        let (object_proto, object_constructor) = if dirty.object {
            make_named_pair(string_forge, shape_forge, labels, "Object")
        } else {
            (current.object_proto.clone(), current.object_constructor.clone())
        };
        let (array_proto, array_constructor) = if dirty.array {
            make_named_pair(string_forge, shape_forge, labels, "Array")
        } else {
            (current.array_proto.clone(), current.array_constructor.clone())
        };
        let (function_proto, function_constructor) = if dirty.function {
            make_named_pair(string_forge, shape_forge, labels, "Function")
        } else {
            (current.function_proto.clone(), current.function_constructor.clone())
        };
        let (string_proto, string_constructor) = if dirty.string {
            make_named_pair(string_forge, shape_forge, labels, "String")
        } else {
            (current.string_proto.clone(), current.string_constructor.clone())
        };
        let (number_proto, number_constructor) = if dirty.number {
            make_named_pair(string_forge, shape_forge, labels, "Number")
        } else {
            (current.number_proto.clone(), current.number_constructor.clone())
        };
        let (boolean_proto, boolean_constructor) = if dirty.boolean {
            make_named_pair(string_forge, shape_forge, labels, "Boolean")
        } else {
            (current.boolean_proto.clone(), current.boolean_constructor.clone())
        };
        let (error_proto, error_constructor, error_subtypes) = if dirty.error_family {
            let (error_proto, error_constructor) = make_named_pair(string_forge, shape_forge, labels, "Error");
            let error_subtypes = make_error_subtypes(&error_proto);
            (error_proto, error_constructor, error_subtypes)
        } else {
            (
                current.error_proto.clone(),
                current.error_constructor.clone(),
                ErrorSubtypeProtos {
                    type_error_proto: current.type_error_proto.clone(),
                    reference_error_proto: current.reference_error_proto.clone(),
                    range_error_proto: current.range_error_proto.clone(),
                    syntax_error_proto: current.syntax_error_proto.clone(),
                    uri_error_proto: current.uri_error_proto.clone(),
                    eval_error_proto: current.eval_error_proto.clone(),
                },
            )
        };
        let (
            symbol_proto,
            symbol_constructor,
            sym_match,
            sym_replace,
            sym_search,
            sym_split,
            sym_iterator,
            sym_to_primitive,
            sym_has_instance,
        ) = if dirty.symbol_family {
            let (symbol_proto, symbol_constructor) = make_named_pair(string_forge, shape_forge, labels, "Symbol");
            (
                symbol_proto,
                symbol_constructor,
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            )
        } else {
            (
                current.symbol_proto.clone(),
                current.symbol_constructor.clone(),
                current.sym_match.clone(),
                current.sym_replace.clone(),
                current.sym_search.clone(),
                current.sym_split.clone(),
                current.sym_iterator.clone(),
                current.sym_to_primitive.clone(),
                current.sym_has_instance.clone(),
            )
        };

        let math_object = if dirty.math {
            P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))
        } else {
            current.math_object.clone()
        };
        let json_object = if dirty.json {
            P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))
        } else {
            current.json_object.clone()
        };
        let (date_proto, date_constructor) = if dirty.date {
            make_named_pair(string_forge, shape_forge, labels, "Date")
        } else {
            (current.date_proto.clone(), current.date_constructor.clone())
        };
        let (set_proto, set_constructor) = if dirty.set {
            make_named_pair(string_forge, shape_forge, labels, "Set")
        } else {
            (current.set_proto.clone(), current.set_constructor.clone())
        };
        let (map_proto, map_constructor) = if dirty.map {
            make_named_pair(string_forge, shape_forge, labels, "Map")
        } else {
            (current.map_proto.clone(), current.map_constructor.clone())
        };
        let (regexp_proto, regexp_constructor) = if dirty.regexp {
            make_named_pair(string_forge, shape_forge, labels, "RegExp")
        } else {
            (current.regexp_proto.clone(), current.regexp_constructor.clone())
        };
        let (array_buffer_proto, array_buffer_constructor) = if dirty.array_buffer {
            make_named_pair(string_forge, shape_forge, labels, "ArrayBuffer")
        } else {
            (current.array_buffer_proto.clone(), current.array_buffer_constructor.clone())
        };
        let (data_view_proto, data_view_constructor) = if dirty.data_view {
            make_named_pair(string_forge, shape_forge, labels, "DataView")
        } else {
            (current.data_view_proto.clone(), current.data_view_constructor.clone())
        };
        let typed_arrays = if dirty.typed_array_family {
            make_typed_array_family(string_forge, shape_forge, labels, &object_proto)
        } else {
            TypedArrayFamily {
                typed_array_proto: current.typed_array_proto.clone(),
                int8array_constructor: current.int8array_constructor.clone(),
                int8array_proto: current.int8array_proto.clone(),
                uint8array_constructor: current.uint8array_constructor.clone(),
                uint8array_proto: current.uint8array_proto.clone(),
                uint8clampedarray_constructor: current.uint8clampedarray_constructor.clone(),
                uint8clampedarray_proto: current.uint8clampedarray_proto.clone(),
                int16array_constructor: current.int16array_constructor.clone(),
                int16array_proto: current.int16array_proto.clone(),
                uint16array_constructor: current.uint16array_constructor.clone(),
                uint16array_proto: current.uint16array_proto.clone(),
                int32array_constructor: current.int32array_constructor.clone(),
                int32array_proto: current.int32array_proto.clone(),
                uint32array_constructor: current.uint32array_constructor.clone(),
                uint32array_proto: current.uint32array_proto.clone(),
                float32array_constructor: current.float32array_constructor.clone(),
                float32array_proto: current.float32array_proto.clone(),
                float64array_constructor: current.float64array_constructor.clone(),
                float64array_proto: current.float64array_proto.clone(),
                bigint64array_constructor: current.bigint64array_constructor.clone(),
                bigint64array_proto: current.bigint64array_proto.clone(),
                biguint64array_constructor: current.biguint64array_constructor.clone(),
                biguint64array_proto: current.biguint64array_proto.clone(),
            }
        };
        let stub_objects = if dirty.stubs { Vec::new() } else { current.stub_objects.clone() };

        let world = BuiltinWorld {
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
            type_error_proto: error_subtypes.type_error_proto,
            reference_error_proto: error_subtypes.reference_error_proto,
            range_error_proto: error_subtypes.range_error_proto,
            syntax_error_proto: error_subtypes.syntax_error_proto,
            uri_error_proto: error_subtypes.uri_error_proto,
            eval_error_proto: error_subtypes.eval_error_proto,
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
            array_buffer_constructor,
            array_buffer_proto,
            data_view_constructor,
            data_view_proto,
            typed_array_proto: typed_arrays.typed_array_proto,
            int8array_constructor: typed_arrays.int8array_constructor,
            int8array_proto: typed_arrays.int8array_proto,
            uint8array_constructor: typed_arrays.uint8array_constructor,
            uint8array_proto: typed_arrays.uint8array_proto,
            uint8clampedarray_constructor: typed_arrays.uint8clampedarray_constructor,
            uint8clampedarray_proto: typed_arrays.uint8clampedarray_proto,
            int16array_constructor: typed_arrays.int16array_constructor,
            int16array_proto: typed_arrays.int16array_proto,
            uint16array_constructor: typed_arrays.uint16array_constructor,
            uint16array_proto: typed_arrays.uint16array_proto,
            int32array_constructor: typed_arrays.int32array_constructor,
            int32array_proto: typed_arrays.int32array_proto,
            uint32array_constructor: typed_arrays.uint32array_constructor,
            uint32array_proto: typed_arrays.uint32array_proto,
            float32array_constructor: typed_arrays.float32array_constructor,
            float32array_proto: typed_arrays.float32array_proto,
            float64array_constructor: typed_arrays.float64array_constructor,
            float64array_proto: typed_arrays.float64array_proto,
            bigint64array_constructor: typed_arrays.bigint64array_constructor,
            bigint64array_proto: typed_arrays.bigint64array_proto,
            biguint64array_constructor: typed_arrays.biguint64array_constructor,
            biguint64array_proto: typed_arrays.biguint64array_proto,
            sym_match,
            sym_replace,
            sym_search,
            sym_split,
            sym_iterator,
            sym_to_primitive,
            sym_has_instance,
            stub_objects,
        };
        wire_builtin_world_links(&world);
        world
    }

    pub fn bind_object_methods(&self, methods: &ObjectMethods, string_forge: &PermInterner, shape_forge: &ShapeForge) {
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

    pub fn bind_array_methods(&self, methods: &ArrayMethods, string_forge: &PermInterner, shape_forge: &ShapeForge) {
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
            ("values", methods.values, 0),
        );

        let si = string_forge.intern("@@iterator").0;
        let raw = methods.values;
        // SAFETY: methods.values is a NativeFn fn-item passed by the VM binding layer.
        let func_ptr = unsafe { NativeFnPtr::from_raw(raw) };
        let _ =
            Self::bind_method_static(proto, shape_forge, string_forge, "@@iterator", func_ptr, 0, self.fn_proto_val());
        debug_assert!(shape_forge.lookup_position(proto.shape_id(), si).is_some());
    }

    pub fn bind_error_methods(&self, methods: &ErrorMethods, string_forge: &PermInterner, shape_forge: &ShapeForge) {
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

        let si_name = string_forge.intern("name").0;
        let error_si = string_forge.intern("Error").0;
        let error_name_val = JsValue::perm_string(string_forge.string_ptr(error_si));
        let name_shape = shape_forge.make_shape(proto.shape_id(), si_name);
        proto.set_shape_id(name_shape);
        proto.ensure_hash_props().push(error_name_val);

        let si_message = string_forge.intern("message").0;
        let empty_si = string_forge.intern("").0;
        let empty_val = JsValue::perm_string(string_forge.string_ptr(empty_si));
        let msg_shape = shape_forge.make_shape(proto.shape_id(), si_message);
        proto.set_shape_id(msg_shape);
        proto.ensure_hash_props().push(empty_val);

        self.set_subtype_proto_name(string_forge, shape_forge, &self.type_error_proto, "TypeError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.reference_error_proto, "ReferenceError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.range_error_proto, "RangeError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.syntax_error_proto, "SyntaxError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.uri_error_proto, "URIError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.eval_error_proto, "EvalError", si_name);
    }

    fn set_subtype_proto_name(
        &self, string_forge: &PermInterner, shape_forge: &ShapeForge, proto_p: &P<JsObject>, name: &str, si_name: u32,
    ) {
        let proto_ptr = P::as_ptr(proto_p) as *mut JsObject;
        let proto = unsafe { &mut *proto_ptr };
        let name_si = string_forge.intern(name).0;
        let name_val = JsValue::perm_string(string_forge.string_ptr(name_si));
        let name_shape = shape_forge.make_shape(proto.shape_id(), si_name);
        proto.set_shape_id(name_shape);
        proto.ensure_hash_props().push(name_val);
    }

    pub fn bind_string_methods(&self, methods: &StringMethods, string_forge: &PermInterner, shape_forge: &ShapeForge) {
        let ctor_ptr = P::as_ptr(&self.string_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(self, ctor, string_forge, shape_forge, ("fromCharCode", methods.from_char_code, 1),);

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
            ("substr", methods.substr, 2),
            ("at", methods.at, 1),
            ("lastIndexOf", methods.last_index_of, 1),
        );
    }

    pub fn bind_function_methods(
        &self, methods: &FunctionMethods, string_forge: &PermInterner, shape_forge: &ShapeForge,
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
        &self, proto: &mut JsObject, shape_forge: &ShapeForge, string_forge: &PermInterner, method_name: &str,
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
        proto: &mut JsObject, shape_forge: &ShapeForge, string_forge: &PermInterner, method_name: &str,
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
        // 设置 .length (ES spec: Function.length = formal parameter count,
        // {[[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: true})
        let si_length = string_forge.intern("length").0;
        let length_shape = shape_forge.make_shape(wrapper.shape_id(), si_length);
        wrapper.set_shape_id(length_shape);
        wrapper.ensure_hash_props().push(JsValue::int(arg_count as i32));
        let length_pos = wrapper.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        wrapper.set_data_meta(length_pos, oxide_types::object::PropAttributes::new(false, false, true));
        // 设置 .name ({[[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: true})
        let si_name = string_forge.intern("name").0;
        let name_shape = shape_forge.make_shape(wrapper.shape_id(), si_name);
        wrapper.set_shape_id(name_shape);
        wrapper
            .ensure_hash_props()
            .push(JsValue::perm_string(string_forge.string_ptr(si)));
        let name_pos = wrapper.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        wrapper.set_data_meta(name_pos, oxide_types::object::PropAttributes::new(false, false, true));
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
    use crate::string_forge::PermInterner;

    fn make_world() -> BuiltinWorld {
        let sf = PermInterner::new();
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

    #[test]
    fn builtin_rebuild_with_dirty_reuses_clean_fields() {
        let sf = PermInterner::new();
        let sh = ShapeForge::new();
        let w = BuiltinWorld::new(&sf, &sh);
        let rebuilt = BuiltinWorld::rebuild_with_dirty(&w, &sf, &sh, &crate::kernel::BuiltinDirtySet::default());

        assert!(std::ptr::eq(w.object_proto.as_ptr(), rebuilt.object_proto.as_ptr()));
        assert!(std::ptr::eq(w.array_proto.as_ptr(), rebuilt.array_proto.as_ptr()));
        assert!(std::ptr::eq(w.function_proto.as_ptr(), rebuilt.function_proto.as_ptr()));
    }

    #[test]
    fn builtin_rebuild_with_dirty_replaces_only_dirty_group() {
        let sf = PermInterner::new();
        let sh = ShapeForge::new();
        let w = BuiltinWorld::new(&sf, &sh);
        let dirty = crate::kernel::BuiltinDirtySet {
            array: true,
            ..Default::default()
        };
        let rebuilt = BuiltinWorld::rebuild_with_dirty(&w, &sf, &sh, &dirty);

        assert!(!std::ptr::eq(w.array_proto.as_ptr(), rebuilt.array_proto.as_ptr()));
        assert!(!std::ptr::eq(w.array_constructor.as_ptr(), rebuilt.array_constructor.as_ptr()));
        assert!(std::ptr::eq(w.object_proto.as_ptr(), rebuilt.object_proto.as_ptr()));
        assert!(std::ptr::eq(w.function_proto.as_ptr(), rebuilt.function_proto.as_ptr()));
    }

    #[test]
    fn builtin_rebuild_with_dirty_repairs_ctor_proto_links() {
        let sf = PermInterner::new();
        let sh = ShapeForge::new();
        let w = BuiltinWorld::new(&sf, &sh);
        let dirty = crate::kernel::BuiltinDirtySet {
            array: true,
            ..Default::default()
        };
        let rebuilt = BuiltinWorld::rebuild_with_dirty(&w, &sf, &sh, &dirty);

        let ctor_proto = rebuilt.array_constructor.get_prop_at(0).as_js_object_ptr();
        let proto_ctor = rebuilt.array_proto.get_prop_at(0).as_js_object_ptr();
        assert!(std::ptr::eq(ctor_proto, rebuilt.array_proto.as_ptr() as *mut JsObject));
        assert!(std::ptr::eq(proto_ctor, rebuilt.array_constructor.as_ptr() as *mut JsObject));
    }
}
