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
    if args.len() < 2 {
        return NativeResult::Err(JsValue::undefined());
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return NativeResult::Err(JsValue::undefined());
    }
    let obj_ptr = val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Err(JsValue::undefined());
    }

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
        return NativeResult::Ok(JsValue::undefined());
    }
    let proto_val = vm.reg(args[1]);
    if !proto_val.is_null() && !proto_val.is_object() {
        return NativeResult::Ok(JsValue::undefined());
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
    {
        let target = unsafe { &mut *target_ptr };
        for &arg_reg in args.iter().skip(2) {
            let source_val = vm.reg(arg_reg);
            if !source_val.is_object() {
                continue;
            }
            let source_ptr = source_val.as_js_object_ptr();
            if source_ptr.is_null() {
                continue;
            }
            let source = unsafe { &*source_ptr };
            let source_keys = walk_own_keys(vm, source);
            for (_si, offset) in source_keys {
                let prop_val = source.get_prop_at(offset);
                target.push_prop(prop_val);
            }
        }
    }
    NativeResult::Ok(target_val)
}

pub fn object_is<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let lhs = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let rhs = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    NativeResult::Ok(JsValue::bool(oxide_runtime_api::same_value(lhs, rhs)))
}

pub fn object_define_property<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 4 {
        return NativeResult::Err(JsValue::undefined());
    }
    let obj_val = vm.reg(args[1]);
    if !obj_val.is_object() {
        return NativeResult::Err(JsValue::undefined());
    }
    let obj_ptr = obj_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Err(JsValue::undefined());
    }
    let desc_val = vm.reg(args[3]);
    if !desc_val.is_object() {
        return NativeResult::Err(JsValue::undefined());
    }
    let desc_ptr = desc_val.as_js_object_ptr();
    if desc_ptr.is_null() {
        return NativeResult::Err(JsValue::undefined());
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
        return NativeResult::Err(JsValue::undefined());
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
        return NativeResult::Ok(JsValue::undefined());
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    NativeResult::Ok(val)
}

pub fn object_seal<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    NativeResult::Ok(val)
}

pub fn object_prevent_extensions<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(val);
    }
    NativeResult::Ok(val)
}

pub fn object_is_frozen<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    NativeResult::Ok(JsValue::bool(false))
}

pub fn object_is_sealed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(true));
    }
    let val = vm.reg(args[1]);
    if !val.is_object() || val.as_js_object_ptr().is_null() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    NativeResult::Ok(JsValue::bool(false))
}

pub fn object_is_extensible<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    native_try!(require_obj_arg(vm, args, "isExtensible"));
    NativeResult::Ok(JsValue::bool(true))
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
        return NativeResult::Ok(vm.reg(args[1]));
    }
    let _obj_ptr = native_try!(require_obj_arg(vm, args, "defineProperties"));
    let desc_val = vm.reg(args[2]);
    if !desc_val.is_object() {
        return NativeResult::Ok(vm.reg(args[1]));
    }
    NativeResult::Ok(vm.reg(args[1]))
}

pub fn object_from_entries<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::null());
    }
    let entries = vm.reg(args[1]);
    if !entries.is_object() {
        return NativeResult::Ok(JsValue::null());
    }
    let obj_ptr = vm.alloc_object(JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject),
    ));
    NativeResult::Ok(JsValue::from_js_object(obj_ptr))
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
