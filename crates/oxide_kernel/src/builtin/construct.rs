//! 实例化职责：空槽构造原型/构造器对、Error 子类型、TypedArray 家族与全局
//! 单例对象，并建立原型链链接（wire_ctor_proto / set_proto_if_changed /
//! wire_builtin_world_links）。

use oxide_types::mem::P;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use super::BuiltinWorld;
use crate::kernel_info;
use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::PermInterner;

fn intern_label(string_forge: &PermInterner, label: &str) -> u32 {
    string_forge.intern(label).0
}

fn make_pair(
    string_forge: &PermInterner, shape_forge: &ShapeForge, name: &str, si_prototype: u32, si_constructor: u32,
    si_name: u32,
) -> (P<JsObject>, P<JsObject>) {
    intern_label(string_forge, name);
    let name_si = string_forge.intern(name).0;

    let mut proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    let mut ctor = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());

    let proto_shape = shape_forge.make_shape(EMPTY_SHAPE_ID, si_constructor);
    proto.set_shape_id(proto_shape);

    let ctor_shape1 = shape_forge.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let ctor_shape2 = shape_forge.make_shape(ctor_shape1, si_name);
    ctor.set_shape_id(ctor_shape2);
    ctor.set_function(true);

    // 预分配槽位：proto[0]="constructor"、ctor[0]="prototype"、ctor[1]="name"。
    proto.ensure_hash_props().push(JsValue::undefined());
    ctor.ensure_hash_props().push(JsValue::undefined());
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
pub(crate) struct BuiltinLabels {
    prototype: u32,
    constructor: u32,
    name: u32,
}

pub(crate) fn builtin_labels(string_forge: &PermInterner) -> BuiltinLabels {
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

pub(crate) fn make_named_pair(
    string_forge: &PermInterner, shape_forge: &ShapeForge, labels: BuiltinLabels, name: &str,
) -> (P<JsObject>, P<JsObject>) {
    make_pair(string_forge, shape_forge, name, labels.prototype, labels.constructor, labels.name)
}

/// 把 Number.prototype 标成 Number 对象本体：规范 21.7.3 规定其是
/// [[NumberData]] = +0 的 Number object。type_tag 是 Object.prototype.toString
/// 品牌表的判据（置位后报 "[object Number]"），boxed 值供 thisNumberValue/
/// valueOf 的通用分支读取。
pub(crate) fn tag_number_proto(proto: &P<JsObject>) {
    let ptr = proto.as_ptr() as *mut JsObject;
    // SAFETY: proto 是 make_named_pair 刚建的本进程对象，P 引用与裸指针同址。
    let obj = unsafe { &mut *ptr };
    obj.type_tag = JsObject::OBJ_TYPE_NUMBER_OBJ;
    obj.set_boxed_value(JsValue::int(0));
}

/// 把 Boolean.prototype 标成 Boolean 对象本体：规范 20.7.3 规定其是
/// [[BooleanData]] = false 的 Boolean object。type_tag 是品牌表判据
/// （置位后报 "[object Boolean]"），boxed 值供 valueOf/toString 的通用分支读取。
pub(crate) fn tag_boolean_proto(proto: &P<JsObject>) {
    let ptr = proto.as_ptr() as *mut JsObject;
    // SAFETY: proto 是 make_named_pair 刚建的本进程对象，P 引用与裸指针同址。
    let obj = unsafe { &mut *ptr };
    obj.type_tag = JsObject::OBJ_TYPE_BOOLEAN_OBJ;
    obj.set_boxed_value(JsValue::bool(false));
}

/// 把 String.prototype 标成 String 对象本体：规范 22.7.3 规定其是
/// [[StringData]] = 空串的 String object。type_tag 是品牌表判据，boxed 值
/// 供 thisStringValue/valueOf 的通用分支读取；length 自身属性按构造期
/// 物化约定落地（writable:false / enumerable:false / configurable:false）。
pub(crate) fn tag_string_proto(proto: &P<JsObject>, string_forge: &PermInterner, shape_forge: &ShapeForge) {
    let ptr = proto.as_ptr() as *mut JsObject;
    // SAFETY: proto 是 make_named_pair 刚建的本进程对象，P 引用与裸指针同址。
    let obj = unsafe { &mut *ptr };
    obj.type_tag = JsObject::OBJ_TYPE_STRING_OBJ;
    obj.set_boxed_value(JsValue::perm_string(crate::string_forge::empty_string_ptr()));
    let length_si = string_forge.intern("length").0;
    let shape_id = shape_forge.make_shape(obj.shape_id(), length_si);
    obj.set_shape_id(shape_id);
    // push_prop 返回绝对存储下标（尾部槽），meta 按同下标落位，
    // 避免写死槽号覆盖既有属性（如 constructor）的元数据。
    let length_pos = obj.push_prop(JsValue::int(0));
    obj.set_data_meta(length_pos, PropAttributes::new(false, false, false));
    obj.bump_generation();
}

pub(crate) fn make_error_subtypes(error_proto: &P<JsObject>) -> ErrorSubtypeProtos {
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

pub(crate) struct ErrorSubtypeProtos {
    pub(crate) type_error_proto: P<JsObject>,
    pub(crate) reference_error_proto: P<JsObject>,
    pub(crate) range_error_proto: P<JsObject>,
    pub(crate) syntax_error_proto: P<JsObject>,
    pub(crate) uri_error_proto: P<JsObject>,
    pub(crate) eval_error_proto: P<JsObject>,
    pub(crate) suppressed_error_proto: P<JsObject>,
}

pub(crate) struct TypedArrayFamily {
    pub(crate) typed_array_constructor: P<JsObject>,
    pub(crate) typed_array_proto: P<JsObject>,
    pub(crate) int8array_constructor: P<JsObject>,
    pub(crate) int8array_proto: P<JsObject>,
    pub(crate) uint8array_constructor: P<JsObject>,
    pub(crate) uint8array_proto: P<JsObject>,
    pub(crate) uint8clampedarray_constructor: P<JsObject>,
    pub(crate) uint8clampedarray_proto: P<JsObject>,
    pub(crate) int16array_constructor: P<JsObject>,
    pub(crate) int16array_proto: P<JsObject>,
    pub(crate) uint16array_constructor: P<JsObject>,
    pub(crate) uint16array_proto: P<JsObject>,
    pub(crate) int32array_constructor: P<JsObject>,
    pub(crate) int32array_proto: P<JsObject>,
    pub(crate) uint32array_constructor: P<JsObject>,
    pub(crate) uint32array_proto: P<JsObject>,
    pub(crate) float32array_constructor: P<JsObject>,
    pub(crate) float32array_proto: P<JsObject>,
    pub(crate) float64array_constructor: P<JsObject>,
    pub(crate) float64array_proto: P<JsObject>,
    pub(crate) bigint64array_constructor: P<JsObject>,
    pub(crate) bigint64array_proto: P<JsObject>,
    pub(crate) biguint64array_constructor: P<JsObject>,
    pub(crate) biguint64array_proto: P<JsObject>,
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

pub(crate) fn make_typed_array_family(
    string_forge: &PermInterner, shape_forge: &ShapeForge, labels: BuiltinLabels, object_proto: &P<JsObject>,
) -> TypedArrayFamily {
    let obj_proto_val = JsValue::from_js_object(object_proto.as_ptr() as *mut JsObject);
    // 给共享原型开 "constructor" 槽位（占位值在 wire 时填抽象构造器）；
    // [[Prototype]] 接 Object.prototype（own toString 由绑定层安装，不经 Array.prototype 继承）。
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

/// 建立全部内置对象的原型链连线：构造器/原型对互指与各原型的
/// `[[Prototype]]` 赋值。
///
/// # 步骤
/// 1. 36 组构造器/原型对互指（`.prototype` / `.constructor` 槽，含
///    TypedArray 家族共享对），经 `wire_ctor_proto` 覆盖占位槽；
/// 2. 24 个非 TypedArray 构造器的 `[[Prototype]]` → Function.prototype
///    （标准内置函数对象均继承 Function.prototype）；
/// 3. 26 个非 Object 原型的 `[[Prototype]]` → Object.prototype，另含
///    Temporal 命名空间对象、Temporal.now、Console 单例与 Math/JSON
///    命名空间对象；
/// 4. %IteratorPrototype% → Object.prototype，6 个集合迭代器原型
///    → %IteratorPrototype%；
/// 5. TypedArray 家族：11 个具体原型 → %TypedArray% 共享原型，11 个具体
///    构造器 → %TypedArray% 抽象构造器，抽象构造器 `[[Prototype]]`
///    → Function.prototype。
///
/// # 边界与前提
/// - 在 `BuiltinWorld::new` 全量构造末尾与选择性重建（`rebuild_with_dirty`）
///   构造出新 world 后调用，各对对象均已构造完毕，本函数只填槽不改形状；
/// - `set_proto_if_changed` 仅当槽值不同才改写，重复调用幂等。
///
/// # 副作用
/// - 改写上述对象的 `.prototype` / `.constructor` 槽值与 `[[Prototype]]`
///   槽，每次 `[[Prototype]]` 改写递增该对象 generation。
pub(crate) fn wire_builtin_world_links(world: &BuiltinWorld) {
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
    wire_ctor_proto(&world.shared_array_buffer_constructor, &world.shared_array_buffer_proto);
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

    // 非 TypedArray 构造器的 [[Prototype]] 指向 Function.prototype：
    // 标准内置函数对象均继承 Function.prototype。
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
        &world.shared_array_buffer_constructor,
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
    let non_object_protos: [&P<JsObject>; 28] = [
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
        &world.shared_array_buffer_proto,
        &world.atomics_object,
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
        &world.async_iterator_proto,
    ];
    for proto in &non_object_protos {
        set_proto_if_changed(proto, obj_proto_val);
    }

    // Temporal 命名空间对象（非构造器）继承 Object.prototype。
    set_proto_if_changed(&world.temporal_object, obj_proto_val);
    set_proto_if_changed(&world.temporal_now_object, obj_proto_val);

    // Console 对象（单例命名空间）继承 Object.prototype。
    set_proto_if_changed(&world.console_object, obj_proto_val);

    // Math / JSON 命名空间对象（非构造器）继承 Object.prototype。
    set_proto_if_changed(&world.math_object, obj_proto_val);
    set_proto_if_changed(&world.json_object, obj_proto_val);

    // 迭代器原型链：%IteratorPrototype% → Object.prototype；各集合迭代器原型
    // → %IteratorPrototype%（next/@@iterator 方法由绑定层——oxide_builtins
    // 中安装方法 wrapper 的代码——安装到对应原型）。
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
    /// 全量构造一个全新的 builtin world：创建所有原型/构造器对、Error 子类型、
    /// TypedArray 家族与 well-known symbol 对象，并建立原型链链接。
    pub fn new(string_forge: &PermInterner, shape_forge: &ShapeForge) -> Self {
        let labels = builtin_labels(string_forge);

        let (object_proto, object_constructor) = make_named_pair(string_forge, shape_forge, labels, "Object");
        let (array_proto, array_constructor) = make_named_pair(string_forge, shape_forge, labels, "Array");
        let (function_proto, function_constructor) = make_named_pair(string_forge, shape_forge, labels, "Function");
        let (string_proto, string_constructor) = make_named_pair(string_forge, shape_forge, labels, "String");
        tag_string_proto(&string_proto, string_forge, shape_forge);
        let (number_proto, number_constructor) = make_named_pair(string_forge, shape_forge, labels, "Number");
        tag_number_proto(&number_proto);
        let (boolean_proto, boolean_constructor) = make_named_pair(string_forge, shape_forge, labels, "Boolean");
        tag_boolean_proto(&boolean_proto);
        let (error_proto, error_constructor) = make_named_pair(string_forge, shape_forge, labels, "Error");
        let (symbol_proto, symbol_constructor) = make_named_pair(string_forge, shape_forge, labels, "Symbol");

        let error_subtypes = make_error_subtypes(&error_proto);

        let math_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let json_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let atomics_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let console_object = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));

        let (date_proto, date_constructor) = make_named_pair(string_forge, shape_forge, labels, "Date");
        let (set_proto, set_constructor) = make_named_pair(string_forge, shape_forge, labels, "Set");
        let (map_proto, map_constructor) = make_named_pair(string_forge, shape_forge, labels, "Map");
        let (regexp_proto, regexp_constructor) = make_named_pair(string_forge, shape_forge, labels, "RegExp");
        let (array_buffer_proto, array_buffer_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "ArrayBuffer");
        let (shared_array_buffer_proto, shared_array_buffer_constructor) =
            make_named_pair(string_forge, shape_forge, labels, "SharedArrayBuffer");
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
        // wire_builtin_world_links 中建立，next/@@iterator 方法由绑定层
        // （oxide_builtins 中安装方法 wrapper 的代码）安装。
        let iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let array_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let map_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let set_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let string_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let regexp_string_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        let async_iterator_proto = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
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
            shared_array_buffer_proto,
            shared_array_buffer_constructor,
            atomics_object,
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
            async_iterator_proto,
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
}
