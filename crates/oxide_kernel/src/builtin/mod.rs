//! 内置对象世界：持有并构造全部内置原型、构造器与全局单例（Math/JSON），
//! 支持按脏标记重建，并提供绑定层的 native 方法指针表。

use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::kernel::{BuiltinDirtySet, BuiltinId};
use crate::kernel_info;
use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::PermInterner;

mod bind;
mod methods;
pub use methods::{ArrayMethods, ErrorMethods, FunctionMethods, ObjectMethods, RegExpMethods, StringMethods};

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
    ($target:expr, $sf:expr, $sh:expr, $world:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $({
            let _raw: *const () = $func as *const ();
            // SAFETY: $func 是 NativeFn 函数项；强转并包装合法。
            let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
            let _ = $crate::builtin::BuiltinWorld::bind_method_static(
                $target, $sh, $sf, $name, _func_ptr, $nargs, $world,
            );
        })*
    };
    ($target:expr, $sf:expr, $sh:expr, $world:expr, $label:expr,
     $(($name:literal, $func:expr, $nargs:expr)),* $(,)?) => {
        $({
            let _raw: *const () = $func as *const ();
            // SAFETY: $func 是 NativeFn 函数项；强转并包装合法。
            let _func_ptr = unsafe { oxide_types::object::NativeFnPtr::from_raw(_raw) };
            let _ = $crate::builtin::BuiltinWorld::bind_method_labeled_static(
                $target, $sh, $sf, $name, _func_ptr, $nargs, $world, $label,
            );
        })*
    };
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
    pub plain_month_day_constructor: P<JsObject>,
    pub plain_month_day_proto: P<JsObject>,
    pub plain_year_month_constructor: P<JsObject>,
    pub plain_year_month_proto: P<JsObject>,
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
    /// String.prototype[@@iterator] 默认迭代器函数对象指针（绑定层捕获，原始值指针、
    /// 所有权归 wrapper 释放表）。String 臂覆盖判定以此做指针比较：默认迭代器即
    /// @@iterator 槽本身（自别名），集合式锚点槽比较不可复用，须独立存指针。
    /// `Cell` 供绑定层经 `&Arc<BuiltinWorld>` 共享引用写入。
    pub string_default_iterator: std::cell::Cell<*const JsObject>,
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
    pub console_object: P<JsObject>,
    /// 绑定层经 `Box::into_raw` 持有的函数/宿主对象登记表（方法 wrapper、
    /// 访问器、错误构造器、Reflect/Iterator、内建原型构造器、`$262` 宿主等）。
    /// 这些对象本体在堆上、不属任何 arena，`session` 收尾时按表统一释放
    /// （属性区 + 本体）；选择性重建换 world 时本表整体并入新 world
    /// （`inherit_leaked_objects`），仍由 session 收尾统一释放，不悬垂、不双放。
    ///
    /// 可复用 native 函数 wrapper 带复用键（[`FnWrapperKey`]）：选择性重建
    /// 重绑按键命中前轮旧 wrapper，迁移到重建 P 对象槽位，登记表跨轮不增长。
    leaked_objects: std::cell::RefCell<Vec<LeakedSlot>>,
}

/// native 函数 wrapper 的复用键：（目标家族，目标站点标签，属性槽位键，wrapper 名）。
///
/// 家族目标是 [`BuiltinWorld::all_p_fields`] 枚举的稳定下标（重建跨轮不变，
/// 1..=N），此时标签恒 0；非 P 目标（global 对象、Box 自建构造器、宿主对象、
/// VM 内建原型）家族为 0，以绑定站点标签（站点名的 perm intern 键）区分
/// 同名方法槽位——如 Generator/AsyncGenerator 原型同名的 next/return/throw。
/// 选择性重建重绑按键查找前轮旧 wrapper 并迁移，避免每轮新建导致
/// 登记表无界累积；同键重复登记意味着复用键设计缺陷（debug 断言守约）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FnWrapperKey {
    family: u16,
    label: u32,
    slot: u32,
    name: u32,
}

impl FnWrapperKey {
    pub const fn new(family: u16, label: u32, slot: u32, name: u32) -> Self {
        Self { family, label, slot, name }
    }
}

/// 登记表条目：对象指针 + 可选复用键（`None` = 不可复用对象）。
struct LeakedSlot {
    ptr: *mut JsObject,
    key: Option<FnWrapperKey>,
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
    wire_ctor_proto(&world.plain_month_day_constructor, &world.plain_month_day_proto);
    wire_ctor_proto(&world.plain_year_month_constructor, &world.plain_year_month_proto);
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
        &world.plain_month_day_constructor,
        &world.plain_year_month_constructor,
    ] {
        set_proto_if_changed(ctor, fn_proto_val);
    }

    let obj_proto_val = JsValue::from_js_object(world.object_proto.as_ptr() as *mut JsObject);
    let non_object_protos: [&P<JsObject>; 25] = [
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
        &world.plain_month_day_proto,
        &world.plain_year_month_proto,
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

    // Console 对象（单例命名空间）继承 Object.prototype。
    set_proto_if_changed(&world.console_object, obj_proto_val);

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

    /// 登记一个绑定层经 `Box::into_raw` 持有的函数/宿主对象（不可复用对象），
    /// 供 [`Self::teardown_heap_data`] 在 session 收尾时统一释放；可复用
    /// native 函数 wrapper 走 [`Self::track_fn_wrapper`]。
    pub fn track_leaked_object(&self, obj_ptr: *mut JsObject) {
        self.leaked_objects.borrow_mut().push(LeakedSlot { ptr: obj_ptr, key: None });
    }

    /// 登记一个可复用 native 函数 wrapper（带复用键），随登记表在 session
    /// 收尾统一释放。
    ///
    /// # 注意事项
    /// 同键重复登记意味着复用键设计缺陷（同家族槽位对应两个不同 wrapper
    /// 对象）——debug 断言立即失败。
    pub fn track_fn_wrapper(&self, obj_ptr: *mut JsObject, key: FnWrapperKey) {
        debug_assert!(
            !self.leaked_objects.borrow().iter().any(|s| s.key == Some(key)),
            "同键 native 函数 wrapper 重复登记"
        );
        self.leaked_objects
            .borrow_mut()
            .push(LeakedSlot { ptr: obj_ptr, key: Some(key) });
    }

    /// 查找复用键相同且 native 函数/参数个数匹配的既有 wrapper（选择性重建
    /// 重绑的复用入口）。
    ///
    /// # 边界与前提
    /// 登记表指针 session 存活期内有效（full_reset 安全点无并发读者）；
    /// native 函数与参数个数一并校验，防绑定表漂移时误换旧实现。
    pub fn find_fn_wrapper(
        &self, key: FnWrapperKey, native_fn_ptr: NativeFnPtr, arg_count: u8,
    ) -> Option<*mut JsObject> {
        self.leaked_objects
            .borrow()
            .iter()
            .find(|s| {
                s.key == Some(key) && {
                    // SAFETY: 登记表指针 session 存活期内有效。
                    let obj = unsafe { &*s.ptr };
                    obj.native_fn().map(|p| p.0) == Some(native_fn_ptr.0) && obj.native_arg_count() == arg_count
                }
            })
            .map(|s| s.ptr)
    }

    /// wrapper 复用键的目标家族标签：目标对象是本 world 固定 P 字段时返回
    /// 其枚举下标 + 1（`all_p_fields` 顺序跨重建轮不变），非 P 目标返回 0。
    pub fn wrapper_family_of(&self, obj: *const JsObject) -> u16 {
        for (i, p) in self.all_p_fields().iter().enumerate() {
            if std::ptr::eq(p.as_ptr(), obj) {
                return (i + 1) as u16;
            }
        }
        0
    }

    /// 登记表对象数（泄漏校准的跨轮继承采样锚点）。
    pub fn leaked_object_count(&self) -> usize {
        self.leaked_objects.borrow().len()
    }

    /// 选择性重建时把旧 world 的登记表整体并入新 world（见
    /// [`crate::kernel::KernelSession::selective_reset`]）。
    pub fn inherit_leaked_objects(&self, from: &BuiltinWorld) {
        self.leaked_objects.borrow_mut().append(&mut from.leaked_objects.borrow_mut());
    }

    /// 选择性重建收尾：把保留对象（登记表 wrapper + 新旧 world 共用的保留 P
    /// 字段）的 proto 槽从被替换旧指针重指到新指针，随后逐一释放被替换旧 P
    /// 对象的属性区（含保活钉住的 Function/Object 四件；本体钉保留，见
    /// `rebuild_with_dirty` 的保活注记）。
    ///
    /// # 边界与前提
    /// - 须在 `inherit_leaked_objects` 之后、旧 world 换出前调用（登记表完整；
    ///   full_reset 安全点无并发读者）；重指必须先于释放完成，否则保留对象
    ///   的原型链读到已释放属性区。
    /// - 替换集由逐字段新旧指针比较（stubs 按指针集合）判定，与保留集天然
    ///   不相交：未替换字段新旧 world 沿用同一 Arc，其属性区归 session 收尾
    ///   （`teardown_heap_data`）释放，此处不碰，不双放。
    /// - 保活钉住的 4 件本体不经任何路径释放（Arc 计数已被 `mem::forget`
    ///   抬升）；此处只释放其属性区，漏重指时读者降级为属性静默缺失
    ///   （zombie 本体）而非 UAF。
    /// - 只重指 proto 槽；属性值链接由同家族同批替换与绑定层 sync_* 覆盖，
    ///   不在此重指。
    ///
    /// # 副作用
    /// 保留对象 proto 槽重指（generation 递增）；被替换旧 P 对象属性区四区
    /// 释放并置空（幂等，重入为 no-op）。
    pub fn retire_replaced(&self, old: &BuiltinWorld) {
        // 替换映射：逐字段新旧指针比较（stubs 按指针集合），记录被换出的
        // 旧指针 → 新指针；stub 无继任者记空指针，只进释放集。
        let old_fields = old.all_p_fields();
        let new_fields = self.all_p_fields();
        let mut remap: std::collections::HashMap<*mut JsObject, *mut JsObject> = std::collections::HashMap::new();
        for (o, n) in old_fields.iter().zip(new_fields.iter()) {
            let (op, np) = (o.as_ptr() as *mut JsObject, n.as_ptr() as *mut JsObject);
            if !std::ptr::eq(op, np) {
                remap.insert(op, np);
            }
        }
        for p in &old.stub_objects {
            let op = p.as_ptr() as *mut JsObject;
            if !self.stub_objects.iter().any(|q| std::ptr::eq(q.as_ptr() as *mut JsObject, op)) {
                remap.insert(op, std::ptr::null_mut());
            }
        }
        if remap.is_empty() {
            return;
        }
        // 重指：保留对象（新旧 world 同一指针）proto 槽仍指被替换旧指针的，
        // 换新指针——与 `wire_builtin_world_links` 的 set_proto_if_changed 同模式。
        let repoint = |obj: &mut JsObject| {
            let cur = obj.proto();
            if !cur.is_object() {
                return;
            }
            let cur_ptr = cur.as_js_object_ptr();
            let Some(np) = remap.get(&cur_ptr).copied() else {
                return;
            };
            if np.is_null() {
                return;
            }
            // SAFETY: np 是本 world 的 P 对象；full_reset 安全点无并发读者，
            // 成环检查由 set_proto 内部完成。
            obj.set_proto(JsValue::from_js_object(np)).ok();
        };
        for (o, n) in old_fields.iter().zip(new_fields.iter()) {
            let op = o.as_ptr() as *mut JsObject;
            let np = n.as_ptr() as *mut JsObject;
            if std::ptr::eq(op, np) {
                // SAFETY: op/np 指向同一保留 P 对象，重指安全点无并发读者。
                unsafe {
                    repoint(&mut *np);
                }
            }
        }
        for slot in self.leaked_objects.borrow().iter() {
            let ptr = slot.ptr;
            // SAFETY: 登记表指针 session 存活期内有效，重指安全点无并发读者。
            unsafe {
                repoint(&mut *ptr);
            }
        }
        // 守约：重指后保留对象 proto 槽不得残留任何被替换旧指针。
        for (o, n) in old_fields.iter().zip(new_fields.iter()) {
            let op = o.as_ptr() as *mut JsObject;
            let np = n.as_ptr() as *mut JsObject;
            if std::ptr::eq(op, np) {
                let cur = unsafe { (*np).proto() };
                debug_assert!(
                    !cur.is_object() || !remap.contains_key(&cur.as_js_object_ptr()),
                    "保留字段 proto 槽不得残留被替换旧指针"
                );
            }
        }
        for slot in self.leaked_objects.borrow().iter() {
            let cur = unsafe { (*slot.ptr).proto() };
            debug_assert!(
                !cur.is_object() || !remap.contains_key(&cur.as_js_object_ptr()),
                "登记表 wrapper proto 槽不得残留被替换旧指针"
            );
        }
        // 释放：被替换旧 P 对象属性区逐一恰好释放一次（本体不释放；保活钉住
        // 的 4 件保留本体钉、属性区同样释放）。
        for &op in remap.keys() {
            // SAFETY: op 是旧 world 被换出的 P 对象，属性区仅此一处释放并置空
            // （幂等）；full_reset 安全点无并发读者。
            unsafe {
                (&mut *op).release_raw_heap();
            }
        }
    }

    /// 枚举本 world 全部固定 P 对象字段（按结构体字段序，含迭代器原型族、
    /// stub 之外的全部命名空间对象与 console）。
    ///
    /// # 注意事项
    /// session 收尾（`teardown_heap_data`）与选择性重建收尾（`retire_replaced`）
    /// 的 P 字段枚举唯一入口：`BuiltinWorld` 新增 P 字段须在此同步补一行，否则
    /// 收尾时该字段属性区永久泄漏、重建重指/释放漏掉该字段。
    pub(crate) fn all_p_fields(&self) -> [&P<JsObject>; 104] {
        [
            &self.object_proto,
            &self.array_proto,
            &self.function_proto,
            &self.string_proto,
            &self.number_proto,
            &self.boolean_proto,
            &self.error_proto,
            &self.symbol_proto,
            &self.object_constructor,
            &self.array_constructor,
            &self.function_constructor,
            &self.string_constructor,
            &self.number_constructor,
            &self.boolean_constructor,
            &self.error_constructor,
            &self.symbol_constructor,
            &self.type_error_proto,
            &self.reference_error_proto,
            &self.range_error_proto,
            &self.syntax_error_proto,
            &self.uri_error_proto,
            &self.eval_error_proto,
            &self.suppressed_error_proto,
            &self.math_object,
            &self.json_object,
            &self.date_constructor,
            &self.date_proto,
            &self.set_constructor,
            &self.set_proto,
            &self.map_constructor,
            &self.map_proto,
            &self.regexp_constructor,
            &self.regexp_proto,
            &self.array_buffer_constructor,
            &self.array_buffer_proto,
            &self.data_view_constructor,
            &self.data_view_proto,
            &self.typed_array_proto,
            &self.typed_array_constructor,
            &self.int8array_constructor,
            &self.int8array_proto,
            &self.uint8array_constructor,
            &self.uint8array_proto,
            &self.uint8clampedarray_constructor,
            &self.uint8clampedarray_proto,
            &self.int16array_constructor,
            &self.int16array_proto,
            &self.uint16array_constructor,
            &self.uint16array_proto,
            &self.int32array_constructor,
            &self.int32array_proto,
            &self.uint32array_constructor,
            &self.uint32array_proto,
            &self.float32array_constructor,
            &self.float32array_proto,
            &self.float64array_constructor,
            &self.float64array_proto,
            &self.bigint64array_constructor,
            &self.bigint64array_proto,
            &self.biguint64array_constructor,
            &self.biguint64array_proto,
            &self.sym_match,
            &self.sym_replace,
            &self.sym_search,
            &self.sym_split,
            &self.sym_iterator,
            &self.sym_to_primitive,
            &self.sym_has_instance,
            &self.sym_match_all,
            &self.sym_async_iterator,
            &self.sym_to_string_tag,
            &self.sym_species,
            &self.sym_async_dispose,
            &self.sym_dispose,
            &self.temporal_object,
            &self.temporal_now_object,
            &self.instant_constructor,
            &self.instant_proto,
            &self.plain_date_constructor,
            &self.plain_date_proto,
            &self.plain_time_constructor,
            &self.plain_time_proto,
            &self.duration_constructor,
            &self.duration_proto,
            &self.zoned_date_time_constructor,
            &self.zoned_date_time_proto,
            &self.plain_date_time_constructor,
            &self.plain_date_time_proto,
            &self.plain_month_day_constructor,
            &self.plain_month_day_proto,
            &self.plain_year_month_constructor,
            &self.plain_year_month_proto,
            &self.bigint_constructor,
            &self.bigint_proto,
            &self.iterator_proto,
            &self.array_iterator_proto,
            &self.map_iterator_proto,
            &self.set_iterator_proto,
            &self.string_iterator_proto,
            &self.regexp_string_iterator_proto,
            &self.iterator_helper_proto,
            &self.disposable_stack_proto,
            &self.async_disposable_stack_proto,
            &self.console_object,
        ]
    }

    /// 释放本 world 拥有的全部手工堆数据。
    ///
    /// # 口径
    /// 1. `Box::into_raw` 持有的函数/宿主对象（登记表）：先释放其堆外属性区，
    ///    再释放对象本体；
    /// 2. 全部 P 对象字段（`all_p_fields` 枚举 + stub 族）的堆外属性区——
    ///    对象本体随 Arc 引用归零释放。
    ///
    /// # 注意事项
    /// 仅由 session 收尾调用（`KernelSession` 的 `Drop` 与 session 替换前），
    /// 幂等：登记表按值取走，属性区释放后置空。选择性重建（dirty rebuild）
    /// 不走本路径：登记表整体并入新 world（`inherit_leaked_objects`），仍由
    /// session 收尾统一释放；被替换家族的旧 P 字段属性区在重建收尾
    /// （`retire_replaced`）恰好释放一次，与本路径对象集不相交，不双放。
    pub fn teardown_heap_data(&self) {
        for slot in self.leaked_objects.borrow_mut().drain(..) {
            let ptr = slot.ptr;
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 是绑定层 Box::into_raw 产物，session 存活期内有效，
            // 此处恰好释放一次（登记表按值取走，重入时表已空）。
            unsafe {
                let obj = &mut *ptr;
                obj.release_raw_heap();
                drop(Box::from_raw(ptr));
            }
        }
        for p in self.all_p_fields() {
            // SAFETY: p 是本 world 的 P 对象，属性区仅在此释放并置空（幂等）。
            unsafe {
                (&mut *p.as_mut_ptr()).release_raw_heap();
            }
        }
        for p in &self.stub_objects {
            // SAFETY: 同上，stub 对象归本 world 所有。
            unsafe {
                (&mut *p.as_mut_ptr()).release_raw_heap();
            }
        }
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
            BuiltinId::PlainMonthDayConstructor => &self.plain_month_day_constructor,
            BuiltinId::PlainMonthDayProto => &self.plain_month_day_proto,
            BuiltinId::PlainYearMonthConstructor => &self.plain_year_month_constructor,
            BuiltinId::PlainYearMonthProto => &self.plain_year_month_proto,
            BuiltinId::BigIntConstructor => &self.bigint_constructor,
            BuiltinId::BigIntProto => &self.bigint_proto,
            BuiltinId::Console => &self.console_object,
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

        let console_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

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
        let (plain_month_day_proto, plain_month_day_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "PlainMonthDay");
        let (plain_year_month_proto, plain_year_month_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "PlainYearMonth");
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
        // 默认迭代器指针初值空（绑定层随后经 bind_string 捕获写入）。
        let string_default_iterator = std::cell::Cell::new(std::ptr::null());

        let world = Self {
            object_proto,
            string_default_iterator,
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
            plain_month_day_constructor,
            plain_month_day_proto,
            plain_year_month_constructor,
            plain_year_month_proto,
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
            console_object,
            leaked_objects: std::cell::RefCell::new(Vec::new()),
        };
        wire_builtin_world_links(&world);
        kernel_info!("BuiltinWorld initialized");
        world
    }

    /// 按脏标记选择性重建 builtin world：仅重建被污染的对象家族，未污染的保留原指针。
    ///
    /// # 注意事项
    /// - Function/Object 家族脏时，旧 fn_proto/object_proto 对须先保活再重建：
    ///   释放表统一持有的方法 wrapper 永久泄漏（`Box::into_raw`），其 proto 裸指针
    ///   指向绑定时的 function_proto——执行期原型链查找（如 `push.call` 沿 wrapper
    ///   原型链取 `call`）仍走这些对象，reset 清空执行状态不阻断该路径，旧对
    ///   Arc 归零即悬空。`retire_replaced` 重指完成后旧对无读者；本体钉保留
    ///   （每次 dirty rebuild 至多 4 个对象本体永久泄漏）作重指遗漏兜底——漏
    ///   重指时读者降级为属性静默缺失（zombie 本体）而非 UAF，属性区于重指
    ///   完成时由 `retire_replaced` 释放。
    pub fn rebuild_with_dirty(
        current: &BuiltinWorld, string_forge: &PermInterner, shape_forge: &ShapeForge, dirty: &BuiltinDirtySet,
    ) -> BuiltinWorld {
        let labels = builtin_labels(string_forge);

        // 保活：钉住旧 Function/Object 对的 Arc 计数使其永不归零——保留对象的
        // proto 槽重指在 `retire_replaced`（本函数返回后、旧 world 换出前）
        // 完成，钉住的本体是重指遗漏的兜底，不经任何路径释放。
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
            plain_month_day_proto,
            plain_month_day_constructor,
            plain_year_month_proto,
            plain_year_month_constructor,
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
            let (plain_month_day_proto, plain_month_day_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainMonthDay");
            let (plain_year_month_proto, plain_year_month_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainYearMonth");
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
                plain_month_day_proto,
                plain_month_day_constructor,
                plain_year_month_proto,
                plain_year_month_constructor,
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
                current.plain_month_day_proto.clone(),
                current.plain_month_day_constructor.clone(),
                current.plain_year_month_proto.clone(),
                current.plain_year_month_constructor.clone(),
            )
        };
        let (bigint_proto, bigint_constructor) = if dirty.stubs {
            make_named_pair(string_forge, shape_forge, labels, "BigInt")
        } else {
            (current.bigint_proto.clone(), current.bigint_constructor.clone())
        };
        let stub_objects = if dirty.stubs { Vec::new() } else { current.stub_objects.clone() };
        let console_object = if dirty.console {
            P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))
        } else {
            current.console_object.clone()
        };

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
            string_default_iterator: std::cell::Cell::new(if dirty.string {
                std::ptr::null()
            } else {
                current.string_default_iterator.get()
            }),
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
            plain_month_day_constructor,
            plain_month_day_proto,
            plain_year_month_constructor,
            plain_year_month_proto,
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
            console_object,
            leaked_objects: std::cell::RefCell::new(Vec::new()),
        };
        wire_builtin_world_links(&world);
        world
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 选择性重建的释放 + 重指面动态自测：被替换旧 P 对象属性区恰好释放一次
    /// （置空可断言，含保活钉住对的属性区、本体钉保留），保留字段指针不变且
    /// proto 槽重指新指针，登记表并入新 world。
    #[test]
    fn selective_reset_releases_replaced_family_heap() {
        use crate::kernel::{KernelConfig, KernelCore, KernelSession};
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        // 持有旧 world Arc：被替换对象本体在断言期仍可读（属性区指针可检查）。
        let old_world = std::sync::Arc::clone(&session.builtin_world);
        let array_proto = old_world.array_proto.as_ptr() as *mut JsObject;
        let object_proto = old_world.object_proto.as_ptr() as *mut JsObject;
        let fn_proto = old_world.function_proto.as_ptr() as *mut JsObject;

        // 旧原型各造一个命名属性区（绑定后的驻留态）；登记表放一个 proto 槽
        // 指向旧 fn_proto 的泄漏 wrapper（绑定时固化形态）。
        let wrapper = Box::into_raw(Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())));
        unsafe {
            (*wrapper).set_proto(JsValue::from_js_object(fn_proto)).ok();
            (&mut *array_proto).ensure_hash_props().push(JsValue::int(1));
            (&mut *object_proto).ensure_hash_props().push(JsValue::int(2));
            (&mut *fn_proto).ensure_hash_props().push(JsValue::int(3));
        }
        old_world.track_leaked_object(wrapper);

        unsafe {
            (&mut *array_proto).bump_generation();
            (&mut *fn_proto).bump_generation();
        }
        let dirty = session.selective_reset(&core);
        assert!(dirty.array);
        assert!(dirty.function);

        // 被替换家族（array）：属性区四区已释放置空。
        let old_array = unsafe { &*array_proto };
        assert!(old_array.hash_props_raw().is_null());
        assert!(old_array.array_elements_raw().is_null());
        assert!(old_array.array_elements_meta_raw().is_null());
        assert!(old_array.prop_meta_raw().is_null());
        // 保活钉住对（function）：本体钉保留（仍可读），属性区于重指完成后
        // 同样释放——重指后旧对无读者。
        assert!(unsafe { &*fn_proto }.hash_props_raw().is_null());
        // 未脏家族（object）：沿用同一对象，字段指针与属性区均不受影响。
        assert!(!unsafe { &*object_proto }.hash_props_raw().is_null());
        assert!(std::ptr::eq(object_proto, session.builtin_world.object_proto.as_ptr() as *mut JsObject));
        // 重指：保留字段（object_constructor）与保留 wrapper 的 proto 槽均换
        // 到新 fn_proto，无残留旧指针。
        let new_fn_proto = session.builtin_world.function_proto.as_ptr() as *mut JsObject;
        let object_ctor = old_world.object_constructor.as_ptr() as *mut JsObject;
        assert!(std::ptr::eq(
            object_ctor,
            session.builtin_world.object_constructor.as_ptr() as *mut JsObject
        ));
        assert!(std::ptr::eq(unsafe { (*object_ctor).proto().as_js_object_ptr() }, new_fn_proto));
        assert!(std::ptr::eq(unsafe { (*wrapper).proto().as_js_object_ptr() }, new_fn_proto));
        // 登记表并入新 world：保留 wrapper 仍须由 session 收尾统一释放。
        assert!(session.builtin_world.leaked_objects.borrow().iter().any(|s| s.ptr == wrapper));
    }
}
