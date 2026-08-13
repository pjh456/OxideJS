use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::P;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use crate::object::{delete_own_property, key_si_to_string, walk_own_keys};

use oxide_runtime_api::{NativeResult, VmHost};

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

/// `Reflect.construct(target, argumentsList, newTarget)`：以 newTarget 的 prototype
/// 分配 this 后调用 target；返回值非对象时回退到新建的 this。
pub fn reflect_construct<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target = arg(vm, args, 1);
    let arg_list = arg(vm, args, 2);
    let new_target = if args.len() > 3 { arg(vm, args, 3) } else { target };

    if !is_callable(target) {
        return type_error(vm, "Reflect.construct target is not callable");
    }
    let new_target_ptr = if new_target.is_object() {
        new_target.as_js_object_ptr()
    } else {
        std::ptr::null_mut()
    };
    if new_target_ptr.is_null() || !unsafe { &*new_target_ptr }.is_function() {
        return type_error(vm, "Reflect.construct newTarget is not a constructor");
    }
    // 不可构造：箭头函数、native 方法（非构造器，OBJ_TYPE_CONSTRUCTOR 标记的除外）。
    let nt = unsafe { &*new_target_ptr };
    if nt.is_arrow() || (nt.native_fn().is_some() && nt.type_tag != oxide_types::object::JsObject::OBJ_TYPE_CONSTRUCTOR)
    {
        return type_error(vm, "Reflect.construct newTarget is not a constructor");
    }

    let proto_si = vm.kernel_core().perm_interner().intern("prototype").0;
    let proto_val = match vm.resolve_property(unsafe { &*new_target_ptr }, proto_si) {
        Some(p) if p.is_object() => p,
        _ => JsValue::from_js_object(P::as_ptr(&vm.session().builtin_world().object_proto) as *mut JsObject),
    };
    let this_ptr = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    let this_val = JsValue::from_js_object(this_ptr);

    let call_args = array_like_elements(arg_list).unwrap_or_default();
    // ponytail: construct by binding the freshly allocated `this` to a plain call;
    // new.target is not propagated into target. Upgrade path: expose the VM's
    // [[Construct]] frame (constructed_this/new_target in vm.rs) through VmHost.
    match vm.call_function_sync(target, this_val, &call_args) {
        Ok(ret) if ret.is_object() => NativeResult::Ok(ret),
        Ok(_) => NativeResult::Ok(this_val),
        Err(err) => NativeResult::Err(crate::error::create_type_error(vm, &err)),
    }
}

/// `Reflect.defineProperty(target, key, descriptor)`：按 descriptor 定义属性，
/// 数据/访问器属性混用或非法 getter/setter 返回 false。
pub fn reflect_define_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.defineProperty target is not an object");
    };
    let desc_val = arg(vm, args, 3);
    let Some(_) = object_ptr(desc_val) else {
        return type_error(vm, "Reflect.defineProperty descriptor is not an object");
    };

    let key_si = vm.property_key_si(arg(vm, args, 2));
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let get_si = vm.kernel_core().perm_interner().intern("get").0;
    let set_si = vm.kernel_core().perm_interner().intern("set").0;
    let writable_si = vm.kernel_core().perm_interner().intern("writable").0;
    let enumerable_si = vm.kernel_core().perm_interner().intern("enumerable").0;
    let configurable_si = vm.kernel_core().perm_interner().intern("configurable").0;

    let value_field = own_field(vm, desc_val, value_si);
    let get_field = own_field(vm, desc_val, get_si);
    let set_field = own_field(vm, desc_val, set_si);
    let writable_field = own_field(vm, desc_val, writable_si);
    let enumerable = own_field(vm, desc_val, enumerable_si)
        .map(oxide_runtime_api::to_boolean)
        .unwrap_or(false);
    let configurable = own_field(vm, desc_val, configurable_si)
        .map(oxide_runtime_api::to_boolean)
        .unwrap_or(false);

    let has_data = value_field.is_some() || writable_field.is_some();
    let has_accessor = get_field.is_some() || set_field.is_some();
    if has_data && has_accessor {
        return NativeResult::Ok(JsValue::bool(false));
    }

    let target = unsafe { &mut *target_ptr };
    let result = if has_accessor {
        let get = get_field.unwrap_or(JsValue::undefined());
        let set = set_field.unwrap_or(JsValue::undefined());
        if (!get.is_undefined() && !is_callable(get)) || (!set.is_undefined() && !is_callable(set)) {
            return NativeResult::Ok(JsValue::bool(false));
        }
        vm.define_accessor_property(target, key_si, get, set, PropAttributes::new(false, enumerable, configurable))
    } else {
        let value = value_field.unwrap_or(JsValue::undefined());
        let writable = writable_field.map(oxide_runtime_api::to_boolean).unwrap_or(false);
        vm.define_data_property(target, key_si, value, PropAttributes::new(writable, enumerable, configurable))
    };
    NativeResult::Ok(JsValue::bool(result.is_ok()))
}

/// `Reflect.deleteProperty(target, key)`：删除自身属性；
/// 属性不可配置时返回 false，否则重构属性表并返回 true。
pub fn reflect_delete_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.deleteProperty target is not an object");
    };
    let key_si = vm.property_key_si(arg(vm, args, 2));
    let target = unsafe { &mut *target_ptr };
    NativeResult::Ok(JsValue::bool(delete_own_property(vm, target, key_si)))
}

/// `Reflect.get(target, key, receiver)`：读取属性（含原型链与 accessor）。
pub fn reflect_get<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.get target is not an object");
    };
    let key_si = vm.property_key_si(arg(vm, args, 2));
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

/// `Reflect.has(target, key)`：属性是否存在（含原型链）。
pub fn reflect_has<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.has target is not an object");
    };
    let key_si = vm.property_key_si(arg(vm, args, 2));
    NativeResult::Ok(JsValue::bool(vm.resolve_property(unsafe { &*target_ptr }, key_si).is_some()))
}

/// `Reflect.isExtensible(target)`：对象是否可扩展。
pub fn reflect_is_extensible<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.isExtensible target is not an object");
    };
    NativeResult::Ok(JsValue::bool(unsafe { &*target_ptr }.is_extensible()))
}

/// `Reflect.ownKeys(target)`：返回对象全部自身属性名（字符串数组）。
pub fn reflect_own_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.ownKeys target is not an object");
    };
    let target = unsafe { &*target_ptr };
    let key_names: Vec<String> = walk_own_keys(vm, target)
        .into_iter()
        .map(|(si, _)| key_si_to_string(vm, si))
        .collect();
    NativeResult::Ok(make_string_array(vm, &key_names))
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

/// `Reflect.set(target, key, value, receiver)`：写入属性；
/// 不可扩展且属性不存在时返回 false。
pub fn reflect_set<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.set target is not an object");
    };
    let key_si = vm.property_key_si(arg(vm, args, 2));
    let value = arg(vm, args, 3);
    let receiver = if args.len() > 4 { vm.reg(args[4]) } else { target_val };
    let target = unsafe { &mut *target_ptr };
    if !target.is_extensible() && vm.get_own_property_slot(target, key_si).is_none() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    NativeResult::Ok(JsValue::bool(vm.ordinary_set(target, key_si, value, receiver).is_ok()))
}

/// `Reflect.setPrototypeOf(target, proto)`：设置 prototype（对象或 null）。
pub fn reflect_set_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.setPrototypeOf target is not an object");
    };
    let proto = arg(vm, args, 2);
    if !proto.is_object() && !proto.is_null() {
        return type_error(vm, "Reflect.setPrototypeOf prototype must be an object or null");
    }
    NativeResult::Ok(JsValue::bool(unsafe { &mut *target_ptr }.set_proto(proto).is_ok()))
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

/// 按 ToPropertyDescriptor 语义取描述符字段：沿原型链判存在性，存在时经
/// ordinary_get 取值（触发 accessor getter，receiver 为描述符对象本身）。
fn own_field<H: VmHost>(vm: &mut H, desc: JsValue, prop_si: u32) -> Option<JsValue> {
    // ordinary_get 对缺失属性返回 undefined，无法区分"不存在"与"值为
    // undefined"，故先用 resolve_property 沿原型链判 HasProperty。
    let obj = unsafe { &*desc.as_js_object_ptr() };
    vm.resolve_property(obj, prop_si)?;
    vm.ordinary_get(obj, prop_si, desc).ok()
}

fn make_string_array<H: VmHost>(vm: &mut H, parts: &[String]) -> JsValue {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
        parts.len(),
        vm.epoch().bump(),
    ));
    for (idx, part) in parts.iter().enumerate() {
        let value = vm.new_string(part);
        unsafe { &mut *arr }.set_prop_at(idx, value);
    }
    unsafe { &mut *arr }.set_prop_count(parts.len());
    JsValue::from_js_object(arr)
}
