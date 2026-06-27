use oxide_kernel::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use oxide_kernel::string_forge::PermInterner;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::is_private_name_key;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

pub(crate) fn walk_own_keys<H: VmHost>(vm: &H, obj: &JsObject) -> Vec<(u32, u32)> {
    let mut keys: Vec<(u32, u32)> = Vec::new();
    let shape_id = obj.shape_id();
    let mut pos: u32 = 0;
    let mut shape_ids = Vec::new();
    let mut cursor = Some(shape_id);
    while let Some(id) = cursor {
        if id == EMPTY_SHAPE_ID {
            break;
        }
        if let Some(shape) = vm.kernel_core().shape_forge().get_shape(id) {
            cursor = shape.parent;
            if shape.property_name != u32::MAX && !is_private_name_key(shape.property_name) {
                shape_ids.push(id);
            }
        } else {
            break;
        }
    }
    for id in shape_ids.iter().rev() {
        if let Some(shape) = vm.kernel_core().shape_forge().get_shape(*id) {
            if shape.property_name != 0 && !is_private_name_key(shape.property_name) {
                keys.push((shape.property_name, pos));
            }
        }
        pos += 1;
    }
    keys
}

pub fn object_constructor<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    NativeResult::Ok(JsValue::from_js_object(obj))
}

pub fn object_keys<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = match require_obj_arg(vm, args, "keys") {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };

    let key_names: Vec<String>;
    {
        let obj = unsafe { &*obj_ptr };
        let keys = walk_own_keys(vm, obj);
        key_names = keys
            .iter()
            .map(|(si, _offset)| vm.kernel_core().perm_interner().lookup(*si).unwrap_or("").to_string())
            .collect();
    }

    let n = key_names.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, k) in key_names.iter().enumerate() {
        let str_val = vm.new_string(k);
        unsafe {
            (*arr).set_prop_at(i, str_val);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

pub fn object_create<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.create: at least 1 argument required"));
    }
    let proto_val = vm.reg(args[1]);
    if proto_val.is_null() {
        let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
        return NativeResult::Ok(JsValue::from_js_object(obj));
    }
    if !proto_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.create: prototype must be an object or null",
        ));
    }
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    NativeResult::Ok(JsValue::from_js_object(obj))
}

pub fn object_assign<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.assign requires a target"));
    }
    let target_val = vm.reg(args[1]);
    let target_val = match oxide_runtime_api::to_object(target_val, vm) {
        Ok(val) => val,
        Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
    };
    let target_ptr = target_val.as_js_object_ptr();
    if target_ptr.is_null() {
        return NativeResult::Ok(target_val);
    }

    let mut all_assignments: Vec<(u32, JsValue, JsValue)> = Vec::new();
    for &arg_reg in args.iter().skip(2) {
        let source_val = vm.reg(arg_reg);
        if !source_val.is_object() {
            continue;
        }
        let source_ptr = source_val.as_js_object_ptr();
        if source_ptr.is_null() {
            continue;
        }
        let source_keys: Vec<(u32, u32)> = {
            let source = unsafe { &*source_ptr };
            walk_own_keys(vm, source)
        };
        for (si, offset) in source_keys {
            let source = unsafe { &*source_ptr };
            let val = source.get_prop_at(offset);
            all_assignments.push((si, source_val, val));
        }
    }

    let target = unsafe { &mut *target_ptr };
    for (si, source_val, val) in all_assignments {
        let promoted = vm.promote_if_needed_for_write_ptr(target_ptr, val);
        let _ = vm.ordinary_set(target, si, promoted, source_val);
    }
    NativeResult::Ok(target_val)
}

pub fn object_is<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.is called with insufficient arguments"));
    }
    let lhs = vm.reg(args[1]);
    let rhs = vm.reg(args[2]);
    NativeResult::Ok(JsValue::bool(oxide_runtime_api::same_value(lhs, rhs)))
}

pub fn object_define_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 4 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.defineProperty: expected at least 3 arguments",
        ));
    }
    let obj_val = vm.reg(args[1]);
    if !obj_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.defineProperty called on non-object"));
    }
    let obj_ptr = obj_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.defineProperty called on non-object"));
    }
    let desc_val = vm.reg(args[3]);
    if !desc_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Property description must be an object"));
    }
    let desc_ptr = desc_val.as_js_object_ptr();
    if desc_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Property description must be an object"));
    }
    let prop_name_str = oxide_runtime_api::to_string(vm.reg(args[2]));
    let si = vm.kernel_core().perm_interner().intern(&prop_name_str).0;

    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let get_si = vm.kernel_core().perm_interner().intern("get").0;
    let set_si = vm.kernel_core().perm_interner().intern("set").0;
    let writable_si = vm.kernel_core().perm_interner().intern("writable").0;
    let enumerable_si = vm.kernel_core().perm_interner().intern("enumerable").0;
    let configurable_si = vm.kernel_core().perm_interner().intern("configurable").0;

    let desc = unsafe { &*desc_ptr };
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
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Invalid property descriptor: cannot mix data and accessor fields",
        ));
    }

    let obj = unsafe { &mut *obj_ptr };
    if has_accessor {
        let get = get_field.unwrap_or(JsValue::undefined());
        let set = set_field.unwrap_or(JsValue::undefined());
        if (!get.is_undefined() && !is_callable(get)) || (!set.is_undefined() && !is_callable(set)) {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "accessor descriptor get/set must be callable or undefined",
            ));
        }
        if vm
            .define_accessor_property(obj, si, get, set, PropAttributes::new(false, enumerable, configurable))
            .is_err()
        {
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot define property"));
        }
    } else {
        let value = value_field.unwrap_or(JsValue::undefined());
        let writable = writable_field.map(oxide_runtime_api::to_boolean).unwrap_or(false);
        if vm
            .define_data_property(obj, si, value, PropAttributes::new(writable, enumerable, configurable))
            .is_err()
        {
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot define property"));
        }
    }
    NativeResult::Ok(obj_val)
}

pub fn object_get_own_property_descriptor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.getOwnPropertyDescriptor called on non-object",
        ));
    }
    let obj_val = vm.reg(args[1]);
    if !obj_val.is_object() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let obj_ptr = obj_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let prop_name_str = oxide_runtime_api::to_string(vm.reg(args[2]));
    let si = vm.kernel_core().perm_interner().intern(&prop_name_str).0;

    let (found_value, found_meta, found) = {
        let obj = unsafe { &*obj_ptr };
        let keys = walk_own_keys(vm, obj);
        let mut found_value = JsValue::undefined();
        let mut found_meta = None;
        let mut found = false;
        for (prop_si, offset) in keys {
            if prop_si == si {
                found_value = obj.get_prop_at(offset);
                found_meta = obj.prop_meta_at(offset);
                found = true;
                break;
            }
        }
        (found_value, found_meta, found)
    };

    if !found {
        return NativeResult::Ok(JsValue::undefined());
    }

    let sf_ptr = vm.kernel_core().perm_interner().as_ref() as *const PermInterner;
    let sh_ptr = vm.kernel_core().shape_forge().as_ref() as *const ShapeForge;
    let desc_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let desc = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(desc_proto)));
    let sf = unsafe { &*sf_ptr };
    let sh = unsafe { &*sh_ptr };

    let d: &mut JsObject = unsafe { &mut *desc };
    let meta = found_meta.unwrap_or_else(|| oxide_types::object::PropMetaEntry::data(PropAttributes::DEFAULT_DATA));
    if meta.is_accessor {
        push_desc_prop(d, sh, sf.intern("get").0, meta.get);
        push_desc_prop(d, sh, sf.intern("set").0, meta.set);
        push_desc_prop(d, sh, sf.intern("enumerable").0, JsValue::bool(meta.attributes.enumerable()));
        push_desc_prop(d, sh, sf.intern("configurable").0, JsValue::bool(meta.attributes.configurable()));
    } else {
        push_desc_prop(d, sh, sf.intern("value").0, found_value);
        push_desc_prop(d, sh, sf.intern("writable").0, JsValue::bool(meta.attributes.writable()));
        push_desc_prop(d, sh, sf.intern("enumerable").0, JsValue::bool(meta.attributes.enumerable()));
        push_desc_prop(d, sh, sf.intern("configurable").0, JsValue::bool(meta.attributes.configurable()));
    }

    NativeResult::Ok(JsValue::from_js_object(desc))
}

fn own_field<H: VmHost>(vm: &H, obj: &JsObject, prop_si: u32) -> Option<JsValue> {
    vm.kernel_core()
        .shape_forge()
        .lookup_position(obj.shape_id(), prop_si)
        .and_then(|pos| {
            if obj.prop_vec_len() > pos as usize {
                Some(obj.get_prop_at(pos))
            } else {
                None
            }
        })
}

fn is_callable(value: JsValue) -> bool {
    value.is_object() && unsafe { &*value.as_js_object_ptr() }.is_function()
}

fn push_desc_prop(obj: &mut JsObject, shape_forge: &ShapeForge, prop_si: u32, val: JsValue) {
    let shape_id = shape_forge.make_shape(obj.shape_id(), prop_si);
    obj.set_shape_id(shape_id);
    obj.push_prop(val);
}

fn require_obj_arg<H: VmHost>(vm: &mut H, args: &[u8], fn_name: &str) -> Result<*mut JsObject, JsValue> {
    if args.len() < 2 {
        return Err(crate::error::create_type_error(vm, &format!("Object.{fn_name} called on non-object")));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return Err(crate::error::create_type_error(vm, &format!("Object.{fn_name} called on non-object")));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, &format!("Object.{fn_name} called on non-object")));
    }
    Ok(ptr)
}

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

pub fn object_freeze<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.freeze called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    let obj_ptr = val.as_js_object_ptr();
    unsafe {
        (*obj_ptr).set_frozen(true);
        (*obj_ptr).set_extensible(false);
    }
    NativeResult::Ok(val)
}

pub fn object_seal<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.seal called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    let obj_ptr = val.as_js_object_ptr();
    unsafe {
        (*obj_ptr).set_sealed(true);
        (*obj_ptr).set_extensible(false);
    }
    NativeResult::Ok(val)
}

pub fn object_prevent_extensions<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.preventExtensions called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    let obj_ptr = val.as_js_object_ptr();
    unsafe {
        (*obj_ptr).set_extensible(false);
    }
    NativeResult::Ok(val)
}

pub fn object_is_frozen<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.isFrozen called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let obj_ptr = val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let obj = unsafe { &*obj_ptr };
    if obj.is_frozen() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    if obj.is_extensible() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let keys = walk_own_keys(vm, obj);
    for (_si, offset) in keys {
        if let Some(meta) = obj.prop_meta_at(offset) {
            if meta.attributes.configurable() {
                return NativeResult::Ok(JsValue::bool(false));
            }
            if !meta.is_accessor && meta.attributes.writable() {
                return NativeResult::Ok(JsValue::bool(false));
            }
        } else {
            return NativeResult::Ok(JsValue::bool(false));
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

pub fn object_is_sealed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.isSealed called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let obj_ptr = val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let obj = unsafe { &*obj_ptr };
    if obj.is_sealed() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    if obj.is_extensible() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let keys = walk_own_keys(vm, obj);
    for (_si, offset) in keys {
        if let Some(meta) = obj.prop_meta_at(offset) {
            if meta.attributes.configurable() {
                return NativeResult::Ok(JsValue::bool(false));
            }
        } else {
            return NativeResult::Ok(JsValue::bool(false));
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

pub fn object_is_extensible<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.isExtensible called on non-object"));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj_ptr = val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj = unsafe { &*obj_ptr };
    NativeResult::Ok(JsValue::bool(obj.is_extensible()))
}

pub fn object_get_own_property_names<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "getOwnPropertyNames"));

    let key_names: Vec<String> = {
        let obj = unsafe { &*obj_ptr };
        let keys = walk_own_keys(vm, obj);
        keys.iter()
            .map(|(si, _)| vm.kernel_core().perm_interner().lookup(*si).unwrap_or("").to_string())
            .collect()
    };

    let n = key_names.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, k) in key_names.iter().enumerate() {
        let str_val = vm.new_string(k);
        unsafe {
            (*arr).set_prop_at(i, str_val);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

pub fn object_define_properties<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.defineProperties: expected at least 2 arguments",
        ));
    }
    let target_val = vm.reg(args[1]);
    let target_ptr = match require_obj_arg(vm, args, "defineProperties") {
        Ok(ptr) => ptr,
        Err(err) => return NativeResult::Err(err),
    };
    let desc_val = vm.reg(args[2]);
    if !desc_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.defineProperties: descriptor must be an object",
        ));
    }
    let desc_ptr = desc_val.as_js_object_ptr();
    if desc_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.defineProperties: descriptor must be an object",
        ));
    }
    let prop_keys: Vec<(u32, u32)> = {
        let desc = unsafe { &*desc_ptr };
        walk_own_keys(vm, desc)
    };
    for (key_si, offset) in prop_keys {
        let desc = unsafe { &*desc_ptr };
        let prop_desc_val = desc.get_prop_at(offset);
        if !prop_desc_val.is_object() {
            continue;
        }
        let value_si = vm.kernel_core().perm_interner().intern("value").0;
        let desc_obj_ptr = prop_desc_val.as_js_object_ptr();
        if desc_obj_ptr.is_null() {
            continue;
        }
        let desc_obj = unsafe { &*desc_obj_ptr };
        let prop_val = if let Some(pos) = vm.kernel_core().shape_forge().lookup_position(desc_obj.shape_id(), value_si)
        {
            desc_obj.get_prop_at(pos)
        } else {
            JsValue::undefined()
        };
        let target = unsafe { &mut *target_ptr };
        let _ = vm.define_data_property(target, key_si, prop_val, PropAttributes::DEFAULT_DATA);
    }
    NativeResult::Ok(target_val)
}

pub fn object_from_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.fromEntries: expected 1 argument"));
    }
    let entries = vm.reg(args[1]);
    if !entries.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.fromEntries: argument must be iterable"));
    }
    let entries_ptr = entries.as_js_object_ptr();
    if entries_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Object.fromEntries: argument must be iterable"));
    }
    let obj = vm.alloc_object(JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject),
    ));
    let target_val = JsValue::from_js_object(obj);
    let n: usize = unsafe { (*entries_ptr).prop_vec_len() };
    for i in 0..n {
        let pair_val = unsafe { (*entries_ptr).get_prop_at(i) };
        if !pair_val.is_object() {
            continue;
        }
        let pair_ptr = pair_val.as_js_object_ptr();
        if pair_ptr.is_null() {
            continue;
        }
        let pair = unsafe { &*pair_ptr };
        let key_val = pair.get_prop_at(0);
        let value_val = pair.get_prop_at(1);
        let key_str = oxide_runtime_api::to_string(key_val);
        let si = vm.kernel_core().perm_interner().intern(&key_str).0;
        let promoted = vm.promote_if_needed_for_write_ptr(obj, value_val);
        let _ = vm.ordinary_set(unsafe { &mut *obj }, si, promoted, target_val);
    }
    NativeResult::Ok(target_val)
}

pub fn object_get_prototype_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "getPrototypeOf"));
    NativeResult::Ok(unsafe { (*obj_ptr).proto() })
}

pub fn object_has_own<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let obj_ptr = native_try!(require_obj_arg(vm, args, "hasOwn"));
    let key_si = vm.property_key_si(vm.reg(args[2]));
    let obj = unsafe { &*obj_ptr };
    NativeResult::Ok(JsValue::bool(vm.get_own_property_slot(obj, key_si).is_some()))
}

pub fn object_proto_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    // Object.prototype.valueOf returns the `this` object unchanged; OrdinaryToPrimitive
    // then falls through to toString since the result is not primitive.
    NativeResult::Ok(vm.reg(args[0]))
}

pub fn object_proto_to_string<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    // ponytail: minimal [[Class]] string — always "[object Object]". Type-specific
    // tags ("[object Array]" etc.) and Symbol.toStringTag are a later refinement.
    NativeResult::Ok(vm.new_string("[object Object]"))
}

pub fn object_proto_has_own_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() || this_val.as_js_object_ptr().is_null() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.prototype.hasOwnProperty called on non-object",
        ));
    }
    let key_si = vm.property_key_si(vm.reg(args[1]));
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    NativeResult::Ok(JsValue::bool(vm.get_own_property_slot(obj, key_si).is_some()))
}

pub fn object_proto_property_is_enumerable<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() || this_val.as_js_object_ptr().is_null() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Object.prototype.propertyIsEnumerable called on non-object",
        ));
    }
    let key_si = vm.property_key_si(vm.reg(args[1]));
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    let Some(pos) = vm.get_own_property_slot(obj, key_si) else {
        return NativeResult::Ok(JsValue::bool(false));
    };
    let enumerable = obj
        .prop_meta_at(pos)
        .map(|meta| meta.attributes.enumerable())
        .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable());
    NativeResult::Ok(JsValue::bool(enumerable))
}

pub fn object_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "entries"));
    let obj = unsafe { &*obj_ptr };
    let keys = walk_own_keys(vm, obj);
    let n = keys.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, (si, offset)) in keys.iter().enumerate() {
        let key_str = vm.kernel_core().perm_interner().lookup(*si).unwrap_or_default();
        let key_val = vm.new_string(key_str);
        let val = obj.get_prop_at(*offset);
        let pair = vm.alloc_object(JsObject::new_array(
            EMPTY_SHAPE_ID,
            JsValue::from_js_object(array_proto),
            2,
            vm.epoch().bump(),
        ));
        unsafe {
            (*pair).set_prop_at(0, key_val);
            (*pair).set_prop_at(1, val);
            (*pair).set_prop_count(2);
            (*arr).set_prop_at(i, JsValue::from_js_object(pair));
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

pub fn object_values<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj_ptr = native_try!(require_obj_arg(vm, args, "values"));
    let obj = unsafe { &*obj_ptr };
    let keys = walk_own_keys(vm, obj);
    let n = keys.len();
    let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.alloc_object(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for (i, (_si, offset)) in keys.iter().enumerate() {
        let val = obj.get_prop_at(*offset);
        unsafe {
            (*arr).set_prop_at(i, val);
        }
    }
    unsafe {
        (*arr).set_prop_count(n);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}
