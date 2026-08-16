use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::make_well_known_symbol_key;
use oxide_types::value::JsValue;

use crate::kernel::{BuiltinDirtySet, BuiltinId};
use crate::kernel_info;
use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::PermInterner;

#[macro_export]
macro_rules! bind_method {
    ($world:expr, $target:expr, $sf:expr, $sh:expr, $name:literal, $func:expr, $nargs:expr) => {{
        let _raw: *const () = $func as *const ();
        // SAFETY: $func 是 NativeFn 函数项；函数项强转为 *const () 始终有效。
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
            // SAFETY: $func 是 NativeFn 函数项；强转并包装合法。
            let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
            let _ = $crate::builtin::BuiltinWorld::bind_method_static(
                $target, $sh, $sf, $name, _func_ptr, $nargs, $wrapper_proto,
            );
        })*
    };
}

/// Object 静态方法与原型方法的 native 函数指针集合，由 builtin 绑定层填充后交给
/// [`BuiltinWorld::bind_object_methods`] 安装到对象/原型上。
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
    pub get_own_property_symbols: *const (),
    pub define_properties: *const (),
    pub from_entries: *const (),
    pub get_prototype_of: *const (),
    pub has_own: *const (),
    pub entries: *const (),
    pub values: *const (),
    pub has_own_property: *const (),
    pub property_is_enumerable: *const (),
}

/// Array 静态方法与原型方法的 native 函数指针集合，由 [`BuiltinWorld::bind_array_methods`] 安装。
pub struct ArrayMethods {
    pub is_array: *const (),
    pub from: *const (),
    pub of: *const (),
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
    pub entries: *const (),
    pub keys: *const (),
    pub find_last_index: *const (),
    pub to_sorted: *const (),
    pub to_reversed: *const (),
    pub to_spliced: *const (),
    pub with_method: *const (),
}

/// Error 家族（含各子类型）构造器与原型方法的 native 函数指针集合，由 [`BuiltinWorld::bind_error_methods`] 安装。
pub struct ErrorMethods {
    pub error: *const (),
    pub type_error: *const (),
    pub reference_error: *const (),
    pub range_error: *const (),
    pub syntax_error: *const (),
    pub uri_error: *const (),
    pub eval_error: *const (),
    pub suppressed_error: *const (),
    pub to_string: *const (),
    pub stack: *const (),
}

/// String 静态方法与原型方法的 native 函数指针集合，由 [`BuiltinWorld::bind_string_methods`] 安装。
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
    pub from_code_point: *const (),
    pub is_well_formed: *const (),
    pub to_well_formed: *const (),
}

/// RegExp 原型方法的 native 函数指针集合。
pub struct RegExpMethods {
    pub exec: *const (),
    pub test: *const (),
    pub to_string: *const (),
}

/// Function 原型方法的 native 函数指针集合，由 [`BuiltinWorld::bind_function_methods`] 安装。
pub struct FunctionMethods {
    pub call: *const (),
    pub apply: *const (),
    pub bind: *const (),
    pub to_string: *const (),
    /// `@@hasInstance`（well-known symbol id 6）：instanceof 运算符的默认判定。
    pub has_instance: *const (),
}

/// 全部内置对象（原型、构造器、全局单例 Math/JSON、well-known symbol 与 stub 对象）的持有者。
///
/// 每个 session 独立持有自己的 `BuiltinWorld`，保证 session 间内置对象隔离；
/// 由 [`BuiltinWorld::new`] 全量构造，或 [`BuiltinWorld::rebuild_with_dirty`] 按脏标记部分重建。
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
    pub suppressed_error_proto: P<JsObject>,
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
    pub typed_array_constructor: P<JsObject>,
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
    pub sym_match_all: P<JsObject>,
    pub sym_async_iterator: P<JsObject>,
    pub sym_to_string_tag: P<JsObject>,
    pub sym_species: P<JsObject>,
    pub sym_async_dispose: P<JsObject>,
    pub sym_dispose: P<JsObject>,
    pub temporal_object: P<JsObject>,
    pub temporal_now_object: P<JsObject>,
    pub instant_constructor: P<JsObject>,
    pub instant_proto: P<JsObject>,
    pub plain_date_constructor: P<JsObject>,
    pub plain_date_proto: P<JsObject>,
    pub plain_time_constructor: P<JsObject>,
    pub plain_time_proto: P<JsObject>,
    pub duration_constructor: P<JsObject>,
    pub duration_proto: P<JsObject>,
    pub zoned_date_time_constructor: P<JsObject>,
    pub zoned_date_time_proto: P<JsObject>,
    pub plain_date_time_constructor: P<JsObject>,
    pub plain_date_time_proto: P<JsObject>,
    pub bigint_constructor: P<JsObject>,
    pub bigint_proto: P<JsObject>,
    /// `%IteratorPrototype%`：各集合迭代器原型的公共祖先，持有 `@@iterator`（返回自身）。
    pub iterator_proto: P<JsObject>,
    /// `%ArrayIteratorPrototype%`：Array 与 TypedArray 迭代器共享（`next` 挂其上）。
    pub array_iterator_proto: P<JsObject>,
    /// `%MapIteratorPrototype%`：Map 的 values/keys/entries 迭代器共享。
    pub map_iterator_proto: P<JsObject>,
    /// `%SetIteratorPrototype%`：Set 的 values/keys/entries 迭代器共享。
    pub set_iterator_proto: P<JsObject>,
    /// `%StringIteratorPrototype%`：String.prototype[@@iterator] 返回的迭代器。
    pub string_iterator_proto: P<JsObject>,
    /// `%RegExpStringIteratorPrototype%`：matchAll 返回的迭代器。
    pub regexp_string_iterator_proto: P<JsObject>,
    /// `%IteratorHelperPrototype%`：Iterator helpers 结果对象的共享原型，链到 %IteratorPrototype%。
    pub iterator_helper_proto: P<JsObject>,
    /// `DisposableStack.prototype`：同步资源栈原型（链到 Object.prototype），
    /// 方法/别名/@@toStringTag 由绑定层安装。
    pub disposable_stack_proto: P<JsObject>,
    /// `AsyncDisposableStack.prototype`：异步资源栈原型（形状与同步栈一致）。
    pub async_disposable_stack_proto: P<JsObject>,
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
    let name_si = string_forge.intern(name).0; // 同时 intern 构造器名作为属性值

    let mut proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());

    let proto_shape = shape_forge.make_shape(EMPTY_SHAPE_ID, si_constructor);
    proto.set_shape_id(proto_shape);

    let ctor_shape1 = shape_forge.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let ctor_shape2 = shape_forge.make_shape(ctor_shape1, si_name);
    ctor.set_shape_id(ctor_shape2);
    ctor.set_function(true);

    // 预分配槽位：proto[0]="constructor"、ctor[0]="prototype"、ctor[1]="name"。
    proto.ensure_hash_props().push(JsValue::undefined()); // 占位
    ctor.ensure_hash_props().push(JsValue::undefined()); // "prototype" 占位
                                                         // 立即写入实际 name 值（ctor vec[1]）。
    ctor.ensure_hash_props()
        .push(JsValue::perm_string(string_forge.string_ptr(name_si)));
    // 属性元数据：.prototype（ctor[0]）= writable:false, enumerable:false, configurable:false。
    ctor.set_data_meta(0u32, oxide_types::object::PropAttributes::new(false, false, false));
    // .name（ctor[1]）= writable:false, enumerable:false, configurable:true。
    ctor.set_data_meta(1u32, oxide_types::object::PropAttributes::new(false, false, true));
    // .constructor（proto[0]）= writable:true, enumerable:false, configurable:true。
    // （JS 规范：prototype.constructor 非枚举，避免泄漏进 for-in。）
    proto.set_data_meta(0u32, oxide_types::object::PropAttributes::new(true, false, true));

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
        suppressed_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val)),
    }
}

struct ErrorSubtypeProtos {
    type_error_proto: P<JsObject>,
    reference_error_proto: P<JsObject>,
    range_error_proto: P<JsObject>,
    syntax_error_proto: P<JsObject>,
    uri_error_proto: P<JsObject>,
    eval_error_proto: P<JsObject>,
    suppressed_error_proto: P<JsObject>,
}

struct TypedArrayFamily {
    typed_array_constructor: P<JsObject>,
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

/// 建 `%TypedArray%` 抽象构造器对象：name=`TypedArray`、`prototype` 指向共享原型，
/// 实际 native 实现由绑定层配置为恒抛 TypeError（不可 new 也不可调用）。
fn make_typed_array_abstract_ctor(
    string_forge: &PermInterner, shape_forge: &ShapeForge, labels: BuiltinLabels, typed_array_proto: &P<JsObject>,
) -> P<JsObject> {
    let name_si = string_forge.intern("TypedArray").0;
    let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    let ctor_shape1 = shape_forge.make_shape(EMPTY_SHAPE_ID, labels.prototype);
    let ctor_shape2 = shape_forge.make_shape(ctor_shape1, labels.name);
    ctor.set_shape_id(ctor_shape2);
    ctor.set_function(true);
    ctor.ensure_hash_props()
        .push(JsValue::from_js_object(typed_array_proto.as_ptr() as *mut JsObject));
    ctor.ensure_hash_props()
        .push(JsValue::perm_string(string_forge.string_ptr(name_si)));
    // .prototype 不可写不可枚举不可配置；.name 不可写不可枚举可配置。
    ctor.set_data_meta(0, PropAttributes::new(false, false, false));
    ctor.set_data_meta(1, PropAttributes::new(false, false, true));
    P::new(ctor)
}

fn make_typed_array_family(
    string_forge: &PermInterner, shape_forge: &ShapeForge, labels: BuiltinLabels, object_proto: &P<JsObject>,
) -> TypedArrayFamily {
    let obj_proto_val = JsValue::from_js_object(object_proto.as_ptr() as *mut JsObject);
    // 给共享原型开 "constructor" 槽位（占位值在 wire 时填抽象构造器）。
    let mut typed_array_proto_obj = JsObject::new_empty(EMPTY_SHAPE_ID, obj_proto_val);
    let ctor_si = labels.constructor;
    let proto_shape = shape_forge.make_shape(typed_array_proto_obj.shape_id(), ctor_si);
    typed_array_proto_obj.set_shape_id(proto_shape);
    typed_array_proto_obj.ensure_hash_props().push(JsValue::undefined());
    typed_array_proto_obj.set_data_meta(0, PropAttributes::new(true, false, true));
    let typed_array_proto = P::new(typed_array_proto_obj);
    let typed_array_constructor = make_typed_array_abstract_ctor(string_forge, shape_forge, labels, &typed_array_proto);
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
        typed_array_constructor,
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

/// 用真实值覆盖（`make_pair` 设置的）占位槽。
/// ctor.vec[0] = constructor.prototype → proto。
/// proto.vec[0] = proto.constructor → ctor。
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
    wire_ctor_proto(&world.instant_constructor, &world.instant_proto);
    wire_ctor_proto(&world.plain_date_constructor, &world.plain_date_proto);
    wire_ctor_proto(&world.plain_time_constructor, &world.plain_time_proto);
    wire_ctor_proto(&world.duration_constructor, &world.duration_proto);
    wire_ctor_proto(&world.zoned_date_time_constructor, &world.zoned_date_time_proto);
    wire_ctor_proto(&world.plain_date_time_constructor, &world.plain_date_time_proto);
    wire_ctor_proto(&world.bigint_constructor, &world.bigint_proto);

    // 所有内置构造器的 [[Prototype]] 指向 %FunctionPrototype%（ECMA-262 §17：
    // 标准内置函数对象均继承 Function.prototype；此前仅 TypedArray 构造器设置过）。
    let fn_proto_val = world.fn_proto_val();
    for ctor in [
        &world.object_constructor,
        &world.array_constructor,
        &world.function_constructor,
        &world.string_constructor,
        &world.number_constructor,
        &world.boolean_constructor,
        &world.error_constructor,
        &world.symbol_constructor,
        &world.date_constructor,
        &world.set_constructor,
        &world.map_constructor,
        &world.regexp_constructor,
        &world.array_buffer_constructor,
        &world.data_view_constructor,
        &world.bigint_constructor,
        &world.instant_constructor,
        &world.plain_date_constructor,
        &world.plain_time_constructor,
        &world.duration_constructor,
        &world.zoned_date_time_constructor,
        &world.plain_date_time_constructor,
    ] {
        set_proto_if_changed(ctor, fn_proto_val);
    }

    let obj_proto_val = JsValue::from_js_object(world.object_proto.as_ptr() as *mut JsObject);
    let non_object_protos: [&P<JsObject>; 23] = [
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
        &world.instant_proto,
        &world.plain_date_proto,
        &world.plain_time_proto,
        &world.duration_proto,
        &world.zoned_date_time_proto,
        &world.plain_date_time_proto,
        &world.bigint_proto,
        &world.disposable_stack_proto,
        &world.async_disposable_stack_proto,
    ];
    for proto in &non_object_protos {
        set_proto_if_changed(proto, obj_proto_val);
    }

    // Temporal 命名空间对象（非构造器）继承 Object.prototype。
    set_proto_if_changed(&world.temporal_object, obj_proto_val);
    set_proto_if_changed(&world.temporal_now_object, obj_proto_val);

    // 迭代器原型链：%IteratorPrototype% → Object.prototype；各集合迭代器原型
    // → %IteratorPrototype%（next/@@iterator 方法由绑定层安装到对应原型）。
    set_proto_if_changed(&world.iterator_proto, obj_proto_val);
    let iterator_proto_val = JsValue::from_js_object(world.iterator_proto.as_ptr() as *mut JsObject);
    let iterator_protos: [&P<JsObject>; 6] = [
        &world.array_iterator_proto,
        &world.map_iterator_proto,
        &world.set_iterator_proto,
        &world.string_iterator_proto,
        &world.regexp_string_iterator_proto,
        &world.iterator_helper_proto,
    ];
    for proto in &iterator_protos {
        set_proto_if_changed(proto, iterator_proto_val);
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

    // 共享原型与抽象构造器互指（prototype/constructor）；11 个具体构造器的
    // [[Prototype]] 指向 `%TypedArray%`，抽象构造器自身 [[Prototype]] 为 Function.prototype。
    wire_ctor_proto(&world.typed_array_constructor, &world.typed_array_proto);
    set_proto_if_changed(&world.typed_array_constructor, world.fn_proto_val());
    let typed_array_ctor_val = JsValue::from_js_object(world.typed_array_constructor.as_ptr() as *mut JsObject);
    let typed_array_ctors: [&P<JsObject>; 11] = [
        &world.int8array_constructor,
        &world.uint8array_constructor,
        &world.uint8clampedarray_constructor,
        &world.int16array_constructor,
        &world.uint16array_constructor,
        &world.int32array_constructor,
        &world.uint32array_constructor,
        &world.float32array_constructor,
        &world.float64array_constructor,
        &world.bigint64array_constructor,
        &world.biguint64array_constructor,
    ];
    for ctor in &typed_array_ctors {
        set_proto_if_changed(ctor, typed_array_ctor_val);
    }
}

impl BuiltinWorld {
    pub fn fn_proto_val(&self) -> JsValue {
        JsValue::from_js_object(self.function_proto.as_ptr() as *mut JsObject)
    }

    /// 按 [`BuiltinId`] 取对应内置对象的指针引用。
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
            BuiltinId::SuppressedErrorProto => &self.suppressed_error_proto,
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
            BuiltinId::SymMatchAll => &self.sym_match_all,
            BuiltinId::SymAsyncIterator => &self.sym_async_iterator,
            BuiltinId::SymToStringTag => &self.sym_to_string_tag,
            BuiltinId::SymSpecies => &self.sym_species,
            BuiltinId::SymAsyncDispose => &self.sym_async_dispose,
            BuiltinId::SymDispose => &self.sym_dispose,
            BuiltinId::TemporalObject => &self.temporal_object,
            BuiltinId::TemporalNowObject => &self.temporal_now_object,
            BuiltinId::InstantConstructor => &self.instant_constructor,
            BuiltinId::InstantProto => &self.instant_proto,
            BuiltinId::PlainDateConstructor => &self.plain_date_constructor,
            BuiltinId::PlainDateProto => &self.plain_date_proto,
            BuiltinId::PlainTimeConstructor => &self.plain_time_constructor,
            BuiltinId::PlainTimeProto => &self.plain_time_proto,
            BuiltinId::DurationConstructor => &self.duration_constructor,
            BuiltinId::DurationProto => &self.duration_proto,
            BuiltinId::ZonedDateTimeConstructor => &self.zoned_date_time_constructor,
            BuiltinId::ZonedDateTimeProto => &self.zoned_date_time_proto,
            BuiltinId::PlainDateTimeConstructor => &self.plain_date_time_constructor,
            BuiltinId::PlainDateTimeProto => &self.plain_date_time_proto,
            BuiltinId::BigIntConstructor => &self.bigint_constructor,
            BuiltinId::BigIntProto => &self.bigint_proto,
        }
    }

    /// 全量构造一个全新的 builtin world：创建所有原型/构造器对、Error 子类型、
    /// TypedArray 家族与 well-known symbol 对象，并建立原型链链接。
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
        let sym_match_all = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_async_iterator = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_to_string_tag = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_species = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_async_dispose = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let sym_dispose = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let temporal_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let temporal_now_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let (instant_proto, instant_constructor) = make_named_pair(string_forge, shape_forge, labels, "Instant");
        let (plain_date_proto, plain_date_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "PlainDate");
        let (plain_time_proto, plain_time_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "PlainTime");
        let (duration_proto, duration_constructor) = make_named_pair(string_forge, shape_forge, labels, "Duration");
        let (zoned_date_time_proto, zoned_date_time_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "ZonedDateTime");
        let (plain_date_time_proto, plain_date_time_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "PlainDateTime");
        let (bigint_proto, bigint_constructor) = make_named_pair(string_forge, shape_forge, labels, "BigInt");
        let stub_objects = Vec::new();

        // 迭代器原型家族：链关系（→ %IteratorPrototype% → Object.prototype）在
        // wire_builtin_world_links 中建立，next/@@iterator 方法由绑定层安装。
        let iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let array_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let map_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let set_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let string_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let regexp_string_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let iterator_helper_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let disposable_stack_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let async_disposable_stack_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

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
            suppressed_error_proto: error_subtypes.suppressed_error_proto,
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
            typed_array_constructor: typed_arrays.typed_array_constructor,
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
            sym_match_all,
            sym_async_iterator,
            sym_to_string_tag,
            sym_species,
            sym_async_dispose,
            sym_dispose,
            temporal_object,
            temporal_now_object,
            instant_constructor,
            instant_proto,
            plain_date_constructor,
            plain_date_proto,
            plain_time_constructor,
            plain_time_proto,
            duration_constructor,
            duration_proto,
            zoned_date_time_constructor,
            zoned_date_time_proto,
            plain_date_time_constructor,
            plain_date_time_proto,
            bigint_constructor,
            bigint_proto,
            iterator_proto,
            array_iterator_proto,
            map_iterator_proto,
            set_iterator_proto,
            string_iterator_proto,
            regexp_string_iterator_proto,
            iterator_helper_proto,
            disposable_stack_proto,
            async_disposable_stack_proto,
            stub_objects,
        };
        wire_builtin_world_links(&world);
        kernel_info!("BuiltinWorld initialized");
        world
    }

    /// 按脏标记选择性重建 builtin world：仅重建被污染的对象家族，未污染的保留原指针。
    pub fn rebuild_with_dirty(
        current: &BuiltinWorld, string_forge: &PermInterner, shape_forge: &ShapeForge, dirty: &BuiltinDirtySet,
    ) -> BuiltinWorld {
        let labels = builtin_labels(string_forge);

        // 方法 wrapper（Box::into_raw 永久泄漏）的 proto 持有旧 function_proto 裸指针，
        // 旧 function_proto 又经 proto/constructor 引用旧 object 家族。function/object
        // 重建时这 4 个对象若随 Arc 归零释放，保留 wrapper 会沿悬垂原型链
        // use-after-free：泄漏保活旧家族对，与 wrapper 的永久泄漏同一约定
        // （每次 dirty rebuild 至多泄漏 4 个对象，低频可接受）。
        if dirty.function || dirty.object {
            std::mem::forget(current.function_proto.clone());
            std::mem::forget(current.function_constructor.clone());
            std::mem::forget(current.object_proto.clone());
            std::mem::forget(current.object_constructor.clone());
        }

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
                    suppressed_error_proto: current.suppressed_error_proto.clone(),
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
            sym_match_all,
            sym_async_iterator,
            sym_to_string_tag,
            sym_species,
            sym_async_dispose,
            sym_dispose,
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
                current.sym_match_all.clone(),
                current.sym_async_iterator.clone(),
                current.sym_to_string_tag.clone(),
                current.sym_species.clone(),
                current.sym_async_dispose.clone(),
                current.sym_dispose.clone(),
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
                typed_array_constructor: current.typed_array_constructor.clone(),
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
        let (
            temporal_object,
            temporal_now_object,
            instant_proto,
            instant_constructor,
            plain_date_proto,
            plain_date_constructor,
            plain_time_proto,
            plain_time_constructor,
            duration_proto,
            duration_constructor,
            zoned_date_time_proto,
            zoned_date_time_constructor,
            plain_date_time_proto,
            plain_date_time_constructor,
        ) = if dirty.temporal {
            let (instant_proto, instant_constructor) = make_named_pair(string_forge, shape_forge, labels, "Instant");
            let (plain_date_proto, plain_date_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainDate");
            let (plain_time_proto, plain_time_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainTime");
            let (duration_proto, duration_constructor) = make_named_pair(string_forge, shape_forge, labels, "Duration");
            let (zoned_date_time_proto, zoned_date_time_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "ZonedDateTime");
            let (plain_date_time_proto, plain_date_time_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainDateTime");
            (
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                instant_proto,
                instant_constructor,
                plain_date_proto,
                plain_date_constructor,
                plain_time_proto,
                plain_time_constructor,
                duration_proto,
                duration_constructor,
                zoned_date_time_proto,
                zoned_date_time_constructor,
                plain_date_time_proto,
                plain_date_time_constructor,
            )
        } else {
            (
                current.temporal_object.clone(),
                current.temporal_now_object.clone(),
                current.instant_proto.clone(),
                current.instant_constructor.clone(),
                current.plain_date_proto.clone(),
                current.plain_date_constructor.clone(),
                current.plain_time_proto.clone(),
                current.plain_time_constructor.clone(),
                current.duration_proto.clone(),
                current.duration_constructor.clone(),
                current.zoned_date_time_proto.clone(),
                current.zoned_date_time_constructor.clone(),
                current.plain_date_time_proto.clone(),
                current.plain_date_time_constructor.clone(),
            )
        };
        let (bigint_proto, bigint_constructor) = if dirty.stubs {
            make_named_pair(string_forge, shape_forge, labels, "BigInt")
        } else {
            (current.bigint_proto.clone(), current.bigint_constructor.clone())
        };
        let stub_objects = if dirty.stubs { Vec::new() } else { current.stub_objects.clone() };

        // 迭代器原型与资源栈原型依赖 Object.prototype（链到其上）：object 家族重建时
        // 一并重建，否则旧原型链指向已释放的 object_proto。
        let (
            iterator_proto,
            array_iterator_proto,
            map_iterator_proto,
            set_iterator_proto,
            string_iterator_proto,
            regexp_string_iterator_proto,
            iterator_helper_proto,
            disposable_stack_proto,
            async_disposable_stack_proto,
        ) = if dirty.object {
            (
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
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
                current.iterator_proto.clone(),
                current.array_iterator_proto.clone(),
                current.map_iterator_proto.clone(),
                current.set_iterator_proto.clone(),
                current.string_iterator_proto.clone(),
                current.regexp_string_iterator_proto.clone(),
                current.iterator_helper_proto.clone(),
                current.disposable_stack_proto.clone(),
                current.async_disposable_stack_proto.clone(),
            )
        };

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
            suppressed_error_proto: error_subtypes.suppressed_error_proto,
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
            typed_array_constructor: typed_arrays.typed_array_constructor,
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
            sym_match_all,
            sym_async_iterator,
            sym_to_string_tag,
            sym_species,
            sym_async_dispose,
            sym_dispose,
            temporal_object,
            temporal_now_object,
            instant_constructor,
            instant_proto,
            plain_date_constructor,
            plain_date_proto,
            plain_time_constructor,
            plain_time_proto,
            duration_constructor,
            duration_proto,
            zoned_date_time_constructor,
            zoned_date_time_proto,
            plain_date_time_constructor,
            plain_date_time_proto,
            bigint_constructor,
            bigint_proto,
            iterator_proto,
            array_iterator_proto,
            map_iterator_proto,
            set_iterator_proto,
            string_iterator_proto,
            regexp_string_iterator_proto,
            iterator_helper_proto,
            disposable_stack_proto,
            async_disposable_stack_proto,
            stub_objects,
        };
        wire_builtin_world_links(&world);
        world
    }

    /// 把 Object 家族方法安装到 Object 构造器与原型上（含 `hasOwnProperty` 等非枚举元数据修正）。
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
            ("getOwnPropertySymbols", methods.get_own_property_symbols, 1),
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

    /// 把 Array 家族方法安装到 Array 构造器与原型上。
    pub fn bind_array_methods(&self, methods: &ArrayMethods, string_forge: &PermInterner, shape_forge: &ShapeForge) {
        let ctor_ptr = P::as_ptr(&self.array_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(
            self,
            ctor,
            string_forge,
            shape_forge,
            ("isArray", methods.is_array, 1),
            ("from", methods.from, 1),
            ("of", methods.of, 0),
        );

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
            ("entries", methods.entries, 0),
            ("keys", methods.keys, 0),
            ("findLastIndex", methods.find_last_index, 1),
            ("toSorted", methods.to_sorted, 1),
            ("toReversed", methods.to_reversed, 0),
            ("toSpliced", methods.to_spliced, 2),
            ("with", methods.with_method, 2),
        );

        let iterator_key = make_well_known_symbol_key(0);
        let raw = methods.values;
        // SAFETY: methods.values 是 VM 绑定层传入的 NativeFn 函数项。
        let func_ptr = unsafe { NativeFnPtr::from_raw(raw) };
        let _ = Self::bind_method_key_static(
            proto,
            shape_forge,
            string_forge,
            iterator_key,
            "@@iterator",
            func_ptr,
            0,
            self.fn_proto_val(),
        );
        debug_assert!(shape_forge.lookup_position(proto.shape_id(), iterator_key).is_some());
    }

    /// 把 Error 家族方法安装到 Error 及各子类型构造器与原型上。
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
            ("SuppressedError", methods.suppressed_error, 3),
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

        self.set_subtype_proto_name(string_forge, shape_forge, &self.type_error_proto, "TypeError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.reference_error_proto, "ReferenceError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.range_error_proto, "RangeError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.syntax_error_proto, "SyntaxError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.uri_error_proto, "URIError", si_name);
        self.set_subtype_proto_name(string_forge, shape_forge, &self.eval_error_proto, "EvalError", si_name);
        self.set_subtype_proto_name(
            string_forge,
            shape_forge,
            &self.suppressed_error_proto,
            "SuppressedError",
            si_name,
        );
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
        let name_pos = proto.push_prop(name_val);
        // 子类型原型上的 name 同 Error.prototype.name：非枚举数据属性。
        proto.set_data_meta(name_pos, oxide_types::object::PropAttributes::new(true, false, true));
    }

    /// 把 String 家族方法安装到 String 构造器与原型上。
    pub fn bind_string_methods(&self, methods: &StringMethods, string_forge: &PermInterner, shape_forge: &ShapeForge) {
        let ctor_ptr = P::as_ptr(&self.string_constructor) as *mut JsObject;
        let ctor = unsafe { &mut *ctor_ptr };
        bind_methods!(
            self,
            ctor,
            string_forge,
            shape_forge,
            ("fromCharCode", methods.from_char_code, 1),
            ("fromCodePoint", methods.from_code_point, 1),
        );

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
            ("isWellFormed", methods.is_well_formed, 0),
            ("toWellFormed", methods.to_well_formed, 0),
        );
    }

    /// 把 Function 原型方法安装到 Function.prototype 上。
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
        // @@hasInstance 走 well-known symbol 键（id 6）：instanceof 运算符经
        // dispatch_instanceof 读该属性调用，绑定后属性存在性测试与全局改写生效。
        let _ = Self::bind_method_key_static(
            proto,
            shape_forge,
            string_forge,
            make_well_known_symbol_key(6),
            "[Symbol.hasInstance]",
            unsafe { NativeFnPtr::from_raw(methods.has_instance) },
            1,
            fp,
        );
    }

    /// 在指定原型上安装一个 native 方法，wrapper 函数以本 world 的 Function 原型为原型。
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

    /// 构造并安装一个 native 方法 wrapper 函数对象（设置 `length`/`name` 属性与参数元数据）。
    ///
    /// 无状态版本，不依赖 `BuiltinWorld` 实例，供静态绑定宏在初始化阶段直接调用。
    pub fn bind_method_static(
        proto: &mut JsObject, shape_forge: &ShapeForge, string_forge: &PermInterner, method_name: &str,
        native_fn_ptr: NativeFnPtr, arg_count: u8, wrapper_proto: JsValue,
    ) -> Result<(), String> {
        let si = string_forge.intern(method_name).0;
        Self::bind_method_key_static(
            proto,
            shape_forge,
            string_forge,
            si,
            method_name,
            native_fn_ptr,
            arg_count,
            wrapper_proto,
        )
    }

    /// 按指定属性键安装方法 wrapper（键不要求字符串 intern，well-known symbol 键用此路径）。
    ///
    /// `method_name` 只用于 wrapper 的 `name` 属性；`key` 是属性的实际存储键。
    #[expect(clippy::too_many_arguments)]
    pub fn bind_method_key_static(
        proto: &mut JsObject, shape_forge: &ShapeForge, string_forge: &PermInterner, key: u32, method_name: &str,
        native_fn_ptr: NativeFnPtr, arg_count: u8, wrapper_proto: JsValue,
    ) -> Result<(), String> {
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
        // NativeFnPtr 不变量由调用方维护（见 bind_method / bind_method_static
        // 的调用方，均使用函数项表达式）。
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
            .push(JsValue::perm_string(string_forge.string_ptr(string_forge.intern(method_name).0)));
        let name_pos = wrapper.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
        wrapper.set_data_meta(name_pos, oxide_types::object::PropAttributes::new(false, false, true));
        let wrapper_val = JsValue::from_js_object(Box::into_raw(wrapper));
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
        // Object.prototype 是根——其 __proto__ 为 null。
        assert!(w.object_proto.proto().is_null());
        // 其余构造器原型均继承自 Object.prototype。
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
