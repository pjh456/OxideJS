use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes, PropMetaEntry};
use oxide_types::value::JsValue;

use crate::object::walk_own_keys;

use oxide_runtime_api::{NativeResult, VmHost};

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

pub fn reflect_construct<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target = arg(vm, args, 1);
    let arg_list = arg(vm, args, 2);
    let new_target = if args.len() > 3 { arg(vm, args, 3) } else { target };

    if !is_callable(target) {
        return type_error(vm, "Reflect.construct target is not callable");
    }
    // newTarget must be constructable — check if it's an object with function flag
    if new_target.is_object() {
        let nt_ptr = new_target.as_js_object_ptr();
        if !nt_ptr.is_null() && unsafe { &*nt_ptr }.is_function() {
            // Accept: newTarget is a valid constructor
        } else {
            return type_error(vm, "Reflect.construct newTarget is not a constructor");
        }
    } else {
        return type_error(vm, "Reflect.construct newTarget is not a constructor");
    }
    let call_args = array_like_elements(arg_list).unwrap_or_default();
    match vm.call_function_sync(target, JsValue::undefined(), &call_args) {
        Ok(value) => NativeResult::Ok(value),
        Err(err) => NativeResult::Err(crate::error::create_type_error(vm, &err)),
    }
}

pub fn reflect_define_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.defineProperty target is not an object");
    };
    let desc_val = arg(vm, args, 3);
    let Some(desc_ptr) = object_ptr(desc_val) else {
        return type_error(vm, "Reflect.defineProperty descriptor is not an object");
    };

    let key_si = vm.property_key_si(arg(vm, args, 2));
    let desc = unsafe { &*desc_ptr };
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let get_si = vm.kernel_core().perm_interner().intern("get").0;
    let set_si = vm.kernel_core().perm_interner().intern("set").0;
    let writable_si = vm.kernel_core().perm_interner().intern("writable").0;
    let enumerable_si = vm.kernel_core().perm_interner().intern("enumerable").0;
    let configurable_si = vm.kernel_core().perm_interner().intern("configurable").0;

    let value_field = own_field(vm, desc, value_si);
    let get_field = own_field(vm, desc, get_si);
    let set_field = own_field(vm, desc, set_si);
    let writable_field = own_field(vm, desc, writable_si);
    let enumerable = own_field(vm, desc, enumerable_si)
        .map(oxide_runtime_api::to_boolean)
        .unwrap_or(false);
    let configurable = own_field(vm, desc, configurable_si)
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

pub fn reflect_delete_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.deleteProperty target is not an object");
    };
    let key_si = vm.property_key_si(arg(vm, args, 2));
    let target = unsafe { &mut *target_ptr };
    NativeResult::Ok(JsValue::bool(delete_own_property(vm, target, key_si)))
}

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

pub fn reflect_get_own_property_descriptor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(_) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.getOwnPropertyDescriptor target is not an object");
    };
    crate::object::object_get_own_property_descriptor(vm, args)
}

pub fn reflect_get_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.getPrototypeOf target is not an object");
    };
    NativeResult::Ok(unsafe { &*target_ptr }.proto())
}

pub fn reflect_has<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.has target is not an object");
    };
    let key_si = vm.property_key_si(arg(vm, args, 2));
    NativeResult::Ok(JsValue::bool(vm.resolve_property(unsafe { &*target_ptr }, key_si).is_some()))
}

pub fn reflect_is_extensible<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.isExtensible target is not an object");
    };
    NativeResult::Ok(JsValue::bool(unsafe { &*target_ptr }.is_extensible()))
}

pub fn reflect_own_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.ownKeys target is not an object");
    };
    let target = unsafe { &*target_ptr };
    let key_names: Vec<String> = walk_own_keys(vm, target)
        .into_iter()
        .map(|(si, _)| vm.kernel_core().perm_interner().lookup(si).unwrap_or("").to_string())
        .collect();
    NativeResult::Ok(make_string_array(vm, &key_names))
}

pub fn reflect_prevent_extensions<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let target_val = arg(vm, args, 1);
    let Some(target_ptr) = object_ptr(target_val) else {
        return type_error(vm, "Reflect.preventExtensions target is not an object");
    };
    unsafe { &mut *target_ptr }.set_extensible(false);
    NativeResult::Ok(JsValue::bool(true))
}

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

fn own_field<H: VmHost>(vm: &H, obj: &JsObject, prop_si: u32) -> Option<JsValue> {
    vm.kernel_core()
        .shape_forge()
        .lookup_position(obj.shape_id(), prop_si)
        .and_then(|pos| (obj.prop_vec_len() > pos as usize).then(|| obj.get_prop_at(pos)))
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

fn delete_own_property<H: VmHost>(vm: &mut H, obj: &mut JsObject, key_si: u32) -> bool {
    let keys = walk_own_keys(vm, obj);
    let Some((_, delete_pos)) = keys.iter().find(|(si, _)| *si == key_si).copied() else {
        return true;
    };
    if obj
        .prop_meta_at(delete_pos)
        .map(|meta| !meta.attributes.configurable())
        .unwrap_or(false)
    {
        return false;
    }

    let retained: Vec<(u32, JsValue, Option<PropMetaEntry>)> = keys
        .into_iter()
        .filter(|(_, pos)| *pos != delete_pos)
        .map(|(si, pos)| (si, obj.get_prop_at(pos), obj.prop_meta_at(pos)))
        .collect();

    obj.set_shape_id(EMPTY_SHAPE_ID);
    obj.set_prop_count(0usize);
    for (si, value, meta) in retained {
        let shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape);
        let pos = obj.push_prop(value);
        if let Some(meta) = meta {
            if meta.is_accessor {
                obj.set_accessor_meta(pos, meta.get, meta.set, meta.attributes);
            } else {
                obj.set_data_meta(pos, meta.attributes);
            }
        }
    }
    obj.bump_generation();
    true
}
