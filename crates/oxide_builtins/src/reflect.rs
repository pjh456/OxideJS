use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropMetaEntry};
use oxide_types::value::JsValue;

use crate::array::{arraylike_get, from_engine_error, is_constructor_value};
use crate::object::{delete_own_property, key_si_to_js_value, own_symbol_key_values, walk_own_keys};

use oxide_runtime_api::{to_length, NativeResult, VmHost};

/// 原型链深度上限（与引擎原型链深度上限同值）。
const MAX_PROTO_CHAIN_DEPTH: usize = 1024;

/// `Reflect.apply(target, thisArgument, argumentsList)`：以指定 this 与参数数组调用函数。
pub fn reflect_apply<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target = arg(vm, args, 1);
    let this_arg = arg(vm, args, 2);
    let arg_list = arg(vm, args, 3);
    if !is_callable(target) {
        return type_error(vm, "Reflect.apply target is not callable");
    }
    let Some(call_args) = array_like_elements(arg_list) else {
        return type_error(vm, "Reflect.apply argumentsList must be an array-like object");
    };
    match vm.call_function_sync(target, this_arg, &call_args) {
        Ok(value) => NativeResult::Ok(value),
        Err(err) => type_error(vm, &err),
    }
}

/// `Reflect.construct(target, argumentsList, newTarget)`：真 `[[Construct]]`——
/// 按 newTarget 的 prototype 分配 this、new.target = newTarget 压构造帧执行；
/// 返回值非对象时回退到新建的 this。
///
/// # 步骤
/// 1. IsConstructor(target) 为 false → TypeError。
/// 2. newTarget 缺省 = target；IsConstructor 为 false → TypeError。
/// 3. argumentsList 非对象 → TypeError；CreateListFromArrayLike：规范读 length
///    （访问器异常原值传播）、ToLength、逐索引规范 Get。
///
/// # 边界与前提
/// - 构造失败（derived 未调 super 等）的 `Err` 携带原始异常值，原值直传重抛。
pub fn reflect_construct<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target = arg(vm, args, 1);
    let arg_list = arg(vm, args, 2);
    let new_target = if args.len() > 3 { arg(vm, args, 3) } else { target };

    if !is_constructor_value(target) {
        return type_error(vm, "Reflect.construct target is not a constructor");
    }
    if !is_constructor_value(new_target) {
        return type_error(vm, "Reflect.construct newTarget is not a constructor");
    }
    let Some(arg_list_ptr) = object_ptr(arg_list) else {
        return type_error(vm, "Reflect.construct argumentsList is not an object");
    };
    // SAFETY: object_ptr 保证指针非空且指向存活对象。
    let arg_list_obj = unsafe { &*arg_list_ptr };

    // CreateListFromArrayLike：规范读 length（访问器异常原值传播），ToLength 纯函数不抛。
    let length_si = vm.kernel_core().perm_interner().intern("length").0;
    let len_val = match vm.ordinary_get(arg_list_obj, length_si, arg_list) {
        Ok(v) => v,
        Err(msg) => return NativeResult::Err(from_engine_error(vm, &msg)),
    };
    let len = to_length(len_val) as usize;
    let mut call_args = Vec::with_capacity(len);
    for i in 0..len {
        match arraylike_get(vm, arg_list_ptr, i) {
            Ok(v) => call_args.push(v),
            Err(exc) => return NativeResult::Err(exc),
        }
    }

    match vm.construct_ctor_nt(target, new_target, &call_args) {
        Ok(value) => NativeResult::Ok(value),
        Err(exc) => NativeResult::Err(exc),
    }
}

/// `Reflect.defineProperty(target, key, descriptor)`：按 descriptor 定义属性，
/// 数据/访问器属性混用或非法 getter/setter 返回 false。
///
/// # 注意事项
/// - 与 `Object.defineProperty` 共用 `define_from_descriptor`：缺失字段回填现有
///   属性、模块命名空间 exotic 收窄语义一处生效；普通失败投影为布尔 false。
/// - 数组 length 的非法值（`"RangeError: "` 前缀）与强转失败（`"TypeError: "`
///   前缀）按规范抛对应错误，不投影为 false。
/// - 强转期用户代码抛出的原始异常经 VM 专用槽原值重抛。
pub fn reflect_define_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.defineProperty target is not an object");
    };
    // 规范步序：键转换（ToPropertyKey）先于 descriptor 类型检；键转换异常原值传播。
    let key_si = match vm.to_property_key_si(arg(vm, args, 2)) {
        Ok(si) => si,
        Err(e) => return NativeResult::Err(from_engine_error(vm, &e)),
    };
    let desc_val = arg(vm, args, 3);
    let Some(_) = object_ptr(desc_val) else {
        return type_error(vm, "Reflect.defineProperty descriptor is not an object");
    };

    match crate::object::define_from_descriptor(vm, target_ptr, key_si, desc_val) {
        Ok(()) => NativeResult::Ok(JsValue::bool(true)),
        Err(msg) => {
            // 强转期用户代码（valueOf / Symbol.toPrimitive）抛出的异常值转存专用槽，
            // 须原值重抛，不得投影为 false 或改写成引擎错误。
            if let Some(exc) = vm.take_pending_length_exception() {
                return NativeResult::Err(exc);
            }
            match crate::error::split_kinded(&msg) {
                Some((kind, rest)) => NativeResult::Err(crate::error::create_kind_error(vm, kind, rest)),
                None => NativeResult::Ok(JsValue::bool(false)),
            }
        }
    }
}

/// `Reflect.deleteProperty(target, key)`：删除自身属性；
/// 属性不可配置时返回 false，否则重构属性表并返回 true。
pub fn reflect_delete_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.deleteProperty target is not an object");
    };
    // 键转换异常原值传播（ToPropertyKey 步先于 DeleteOwnProperty）。
    let key_si = match vm.to_property_key_si(arg(vm, args, 2)) {
        Ok(si) => si,
        Err(e) => return NativeResult::Err(from_engine_error(vm, &e)),
    };
    let target = unsafe { &mut *target_ptr };
    NativeResult::Ok(JsValue::bool(delete_own_property(vm, target, key_si)))
}

/// `Reflect.get(target, key, receiver)`：读取属性（含原型链与 accessor）。
pub fn reflect_get<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.get target is not an object");
    };
    // 键转换异常原值传播（ToPropertyKey 步先于 Get）。
    let key_si = match vm.to_property_key_si(arg(vm, args, 2)) {
        Ok(si) => si,
        Err(e) => return NativeResult::Err(from_engine_error(vm, &e)),
    };
    let receiver = if args.len() > 3 { vm.reg(args[3]) } else { target_val };
    match vm.ordinary_get(unsafe { &*target_ptr }, key_si, receiver) {
        Ok(value) => NativeResult::Ok(value),
        Err(err) => type_error(vm, &err),
    }
}

/// `Reflect.getOwnPropertyDescriptor(target, key)`：复用 Object 同名实现。
pub fn reflect_get_own_property_descriptor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(_) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.getOwnPropertyDescriptor target is not an object");
    };
    crate::object::object_get_own_property_descriptor(vm, args)
}

/// `Reflect.getPrototypeOf(target)`：返回对象的 prototype。
pub fn reflect_get_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.getPrototypeOf target is not an object");
    };
    NativeResult::Ok(unsafe { &*target_ptr }.proto())
}

/// `Reflect.has(target, key)`：属性存在性判定（规范 HasProperty，含原型链）。
///
/// TypedArray 经统一数值键门（exotic [[HasProperty]]）：界内整数索引判存在；
/// 数字无效键（负/分数/±Infinity/NaN/越界，含 "-0" 特例）立即 false，不查
/// 自身命名属性也不走原型链；非规范数字串落普通路径。原型链上的 TA 按同口径
/// 经门判定。
pub fn reflect_has<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.has target is not an object");
    };
    // 键转换异常原值传播（ToPropertyKey 步先于 HasProperty）。
    let key_si = match vm.to_property_key_si(arg(vm, args, 2)) {
        Ok(si) => si,
        Err(e) => return NativeResult::Err(from_engine_error(vm, &e)),
    };
    NativeResult::Ok(JsValue::bool(vm.has_property(unsafe { &*target_ptr }, key_si)))
}

/// `Reflect.isExtensible(target)`：对象是否可扩展。
pub fn reflect_is_extensible<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.isExtensible target is not an object");
    };
    NativeResult::Ok(JsValue::bool(unsafe { &*target_ptr }.is_extensible()))
}

/// `Reflect.ownKeys(target)`：返回对象全部自身属性键，字符串键在前、Symbol 键在后。
///
/// # 步骤
/// 1. 收集字符串键与整数键（整数升序，其余插入序）。
/// 2. 追加自身 Symbol 键（保持插入序），符合 OrdinaryOwnPropertyKeys 的排序。
///
/// # 副作用
/// - 结果数组分配在当前 epoch。
pub fn reflect_own_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.ownKeys target is not an object");
    };
    let target = unsafe { &*target_ptr };

    // Symbol 键分流在消费端追加：底层字符串枚举源（Object.keys / JSON / for-in）
    // 语义不变，不泄漏 Symbol 键。
    let keys = walk_own_keys(vm, target);
    let symbols = own_symbol_key_values(vm, target);
    let n = keys.len() + symbols.len();

    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, (si, _)) in keys.iter().enumerate() {
        let key_val = key_si_to_js_value(vm, *si);
        unsafe {
            (*arr).set_prop_at(i, key_val);
        }
    }
    let base = keys.len();
    for (j, sym) in symbols.iter().enumerate() {
        unsafe {
            (*arr).set_prop_at(base + j, *sym);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

/// `Reflect.preventExtensions(target)`：阻止扩展，返回 true。
pub fn reflect_prevent_extensions<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.preventExtensions target is not an object");
    };
    unsafe { &mut *target_ptr }.set_extensible(false);
    NativeResult::Ok(JsValue::bool(true))
}

/// `Reflect.set(target, key, value, receiver)`：按 OrdinarySetWithReceiver 语义写入属性。
///
/// # 步骤
/// 1. target 非对象 → TypeError；键转换（ToPropertyKey）异常原值传播。
/// 2. receiver 缺省为 target；receiver == target（指针判等）或 target 为
///    TypedArray → 普通写路径（可扩展性 / setter / 只读判定内部完成）。
/// 3. receiver ≠ target 且 target 为普通对象：target 自身 accessor → 调 setter
///    （无 setter → false）；自身数据不可写 → false；自身数据可写 → 写 receiver；
///    无自身 → 原型链首命中同口径，链无命中 → receiver 上新建数据属性。
///
/// # 副作用
/// - 可能写 target/receiver 属性存储；可能执行用户 setter 代码。
///
/// # 注意事项
/// - setter 抛出原值重抛（专用槽）；纯写失败（只读 / 不可扩展 / 非对象
///   receiver）投影为 false。
/// - TypedArray target 走 exotic [[Set]]（数值键门已含 receiver≠target 臂），
///   普通 receiver 语义仅适用于普通对象。
pub fn reflect_set<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.set target is not an object");
    };
    // 入口清空 uncaught 槽：先前操作的残留值不得被误消费为本次原值
    // （与 define 通道同纪律）。
    vm.clear_uncaught_value();
    // 键转换异常原值传播（ToPropertyKey 步先于 Set）。
    let key_si = match vm.to_property_key_si(arg(vm, args, 2)) {
        Ok(si) => si,
        Err(e) => return NativeResult::Err(from_engine_error(vm, &e)),
    };
    let value = arg(vm, args, 3);
    let receiver = if args.len() > 4 { vm.reg(args[4]) } else { target_val };

    // SAFETY: object_ptr 保证指针非空且指向存活对象。
    let target = unsafe { &*target_ptr };
    // receiver == target（指针判等）走普通写路径：可扩展性检查、setter 与
    // 只读判定内部完成；TypedArray target 走 exotic [[Set]]（数值键门已含
    // receiver≠target 臂）。
    if receiver == target_val || target.is_typed_array_obj() {
        match vm.ordinary_set(unsafe { &mut *target_ptr }, key_si, value, receiver, true) {
            Ok(()) => NativeResult::Ok(JsValue::bool(true)),
            Err(_) => ordinary_set_failure(vm),
        }
    } else {
        set_with_receiver(vm, target, key_si, value, receiver)
    }
}

/// 普通写路径失败投影：setter 抛出的原值存于 uncaught 槽（原值重抛）；
/// 纯写失败（只读 / 不可扩展 / 无 setter）无原值，投影为 false。
fn ordinary_set_failure<H: VmHost>(vm: &mut H) -> NativeResult {
    match vm.take_uncaught_value() {
        Some(exc) => NativeResult::Err(exc),
        None => NativeResult::Ok(JsValue::bool(false)),
    }
}

/// OrdinarySetWithReceiver：target 为普通对象且 receiver ≠ target。
///
/// # 步骤
/// 1. target 自身臂：accessor（无 setter → false / 有 setter → 调用）、
///    数据不可写 → false、数据可写 → 写 receiver。
/// 2. 无自身：原型链首命中——accessor（无 setter → false / 有 setter →
///    调用）、数据不可写 → false、其余 → 写 receiver。
/// 3. 链无命中（链尾 null）：receiver 上新建数据属性。
///
/// # 副作用
/// - 可能写 receiver 属性存储；可能执行用户 setter 代码。
fn set_with_receiver<H: VmHost>(
    vm: &mut H, target: &JsObject, key_si: u32, value: JsValue, receiver: JsValue,
) -> NativeResult {
    // target 自身臂。
    if let Some(pos) = vm.get_own_property_slot(target, key_si) {
        if let Some(meta) = target.prop_meta_at(pos) {
            if meta.is_accessor {
                if meta.set.is_undefined() {
                    return NativeResult::Ok(JsValue::bool(false));
                }
                return call_setter(vm, meta.set, receiver, value);
            }
            if !meta.attributes.writable() {
                return NativeResult::Ok(JsValue::bool(false));
            }
        }
        return set_on_receiver(vm, receiver, key_si, value);
    }
    // 无自身：原型链首命中。
    if let Some(meta) = inherited_meta(vm, target, key_si) {
        if meta.is_accessor {
            if meta.set.is_undefined() {
                return NativeResult::Ok(JsValue::bool(false));
            }
            return call_setter(vm, meta.set, receiver, value);
        }
        if !meta.attributes.writable() {
            return NativeResult::Ok(JsValue::bool(false));
        }
    }
    // 链无命中（链尾 null）：receiver 上新建数据属性。
    set_on_receiver(vm, receiver, key_si, value)
}

/// Call(setter, Receiver, « V »）：以 receiver 为 this、单参调用 setter。
///
/// # 副作用
/// - 执行用户代码；setter 抛出原值重抛（uncaught 槽）。
fn call_setter<H: VmHost>(vm: &mut H, setter: JsValue, receiver: JsValue, value: JsValue) -> NativeResult {
    match vm.call_function_sync(setter, receiver, &[value]) {
        Ok(_) => NativeResult::Ok(JsValue::bool(true)),
        Err(err) => NativeResult::Err(from_engine_error(vm, &err)),
    }
}

/// 写 receiver：自身 accessor / 不可写数据 → false；无自身且不可扩展 →
/// false；其余直写（既有槽保留原属性，新属性 w/e/c 全开）。
///
/// # 副作用
/// - 可能写 receiver 属性存储 / 形状。
fn set_on_receiver<H: VmHost>(vm: &mut H, receiver: JsValue, key_si: u32, value: JsValue) -> NativeResult {
    let receiver_ptr = receiver.as_js_object_ptr();
    if receiver_ptr.is_null() {
        // 非对象 receiver：数据臂写失败。
        return NativeResult::Ok(JsValue::bool(false));
    }
    // SAFETY: receiver_ptr 为 receiver 值携带的非空对象指针，对象在会话内
    // 存活；写路径不移动对象。
    let receiver_obj = unsafe { &*receiver_ptr };
    if let Some(pos) = vm.get_own_property_slot(receiver_obj, key_si) {
        if let Some(meta) = receiver_obj.prop_meta_at(pos) {
            if meta.is_accessor || !meta.attributes.writable() {
                return NativeResult::Ok(JsValue::bool(false));
            }
        }
        let val = vm.promote_if_needed_for_write_ptr(receiver_ptr, value);
        vm.set_or_create_prop_value(unsafe { &mut *receiver_ptr }, key_si, val);
        return NativeResult::Ok(JsValue::bool(true));
    }
    if !receiver_obj.is_extensible() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let val = vm.promote_if_needed_for_write_ptr(receiver_ptr, value);
    vm.set_or_create_prop_value(unsafe { &mut *receiver_ptr }, key_si, val);
    NativeResult::Ok(JsValue::bool(true))
}

/// 原型链首命中：逐级取自身属性元数据，深度封顶；无命中返回 `None`。
fn inherited_meta<H: VmHost>(vm: &H, target: &JsObject, key_si: u32) -> Option<PropMetaEntry> {
    let mut proto = target.proto();
    let mut depth = 0usize;
    while proto.is_object() && depth < MAX_PROTO_CHAIN_DEPTH {
        depth += 1;
        // SAFETY: proto 为链上值携带的非空对象指针，对象在会话内存活。
        let proto_obj = unsafe { &*proto.as_js_object_ptr() };
        if let Some(pos) = vm.get_own_property_slot(proto_obj, key_si) {
            return proto_obj.prop_meta_at(pos);
        }
        proto = proto_obj.proto();
    }
    None
}

/// `Reflect.setPrototypeOf(target, proto)`：设置 prototype（对象或 null）。
/// 不可扩展且新旧原型不同时返回 false；新旧相同返回 true。
pub fn reflect_set_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.setPrototypeOf target is not an object");
    };
    let proto = arg(vm, args, 2);
    if !proto.is_object() && !proto.is_null() {
        return type_error(vm, "Reflect.setPrototypeOf prototype must be an object or null");
    }
    let target = unsafe { &mut *target_ptr };
    // OrdinarySetPrototypeOf 步 2-4：新旧相同先于可扩展判定，直接成功。
    if !target.is_extensible() && target.proto() != proto {
        return NativeResult::Ok(JsValue::bool(false));
    }
    NativeResult::Ok(JsValue::bool(target.set_proto(proto).is_ok()))
}

fn arg<H: VmHost>(vm: &H, args: &[u8], idx: usize) -> JsValue {
    args.get(idx).map(|reg| vm.reg(*reg)).unwrap_or_else(JsValue::undefined)
}

fn object_ptr(value: JsValue) -> Option<*mut JsObject> {
    if !value.is_object() {
        return None;
    }
    let ptr = value.as_js_object_ptr();
    (!ptr.is_null()).then_some(ptr)
}

fn is_callable(value: JsValue) -> bool {
    object_ptr(value).is_some_and(|ptr| unsafe { &*ptr }.is_function())
}

fn type_error<H: VmHost>(vm: &mut H, message: &str) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, message))
}

fn array_like_elements(value: JsValue) -> Option<Vec<JsValue>> {
    let ptr = object_ptr(value)?;
    let obj = unsafe { &*ptr };
    Some((0..obj.prop_count() as usize).map(|idx| obj.get_prop_at(idx)).collect())
}
