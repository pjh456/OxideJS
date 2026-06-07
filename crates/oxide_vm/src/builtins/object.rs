use oxide_kernel::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use oxide_kernel::string_forge::StringForge;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;

fn walk_own_keys(vm: &Vm, obj: &JsObject) -> Vec<(u32, u8)> {
    let mut keys: Vec<(u32, u8)> = Vec::new();
    let shape_id = obj.shape_id();
    let mut pos: u8 = 0;
    let mut shape_ids = Vec::new();
    let mut cursor = Some(shape_id);
    while let Some(id) = cursor {
        if id == EMPTY_SHAPE_ID {
            break;
        }
        if let Some(shape) = vm.kernel().shape_forge().get_shape(id) {
            cursor = shape.parent;
            if shape.property_name != u32::MAX {
                shape_ids.push(id);
            }
        } else {
            break;
        }
    }
    for id in shape_ids.iter().rev() {
        if let Some(shape) = vm.kernel().shape_forge().get_shape(*id) {
            if shape.property_name != 0 {
                keys.push((shape.property_name, pos));
            }
        }
        pos += 1;
    }
    keys
}

pub fn object_constructor(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let obj = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    Ok(JsValue::from_js_object(obj))
}

pub fn object_keys(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return Err(JsValue::undefined());
    }
    let val = vm.reg(args[1]);
    if !val.is_object() {
        return Err(JsValue::undefined());
    }
    let obj_ptr = val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(JsValue::undefined());
    }

    let key_names: Vec<String>;
    {
        let obj = unsafe { &*obj_ptr };
        let keys = walk_own_keys(vm, obj);
        key_names = keys
            .iter()
            .map(|(si, _offset)| vm.kernel().string_forge().lookup(*si).unwrap_or_default())
            .collect();
    }

    let n = key_names.len().min(255);
    let array_proto = vm.kernel().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let arr = vm.epoch().alloc(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(array_proto),
        n,
        vm.epoch().bump(),
    ));
    for k in key_names.iter().take(n) {
        let str_val = vm.intern(k);
        unsafe {
            (*arr).ensure_hash_props().push(Box::new(str_val));
        }
    }
    Ok(JsValue::from_js_object(arr))
}

pub fn object_create(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return Ok(JsValue::undefined());
    }
    let proto_val = vm.reg(args[1]);
    if !proto_val.is_null() && !proto_val.is_object() {
        return Ok(JsValue::undefined());
    }
    let obj = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    Ok(JsValue::from_js_object(obj))
}

pub fn object_assign(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return Err(JsValue::undefined());
    }
    let target_val = vm.reg(args[1]);
    if !target_val.is_object() {
        return Ok(target_val);
    }
    let target_ptr = target_val.as_js_object_ptr();
    if target_ptr.is_null() {
        return Ok(target_val);
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
    Ok(target_val)
}

pub fn object_define_property(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 4 {
        return Err(JsValue::undefined());
    }
    let obj_val = vm.reg(args[1]);
    if !obj_val.is_object() {
        return Err(JsValue::undefined());
    }
    let obj_ptr = obj_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(JsValue::undefined());
    }
    let desc_val = vm.reg(args[3]);
    if !desc_val.is_object() {
        return Err(JsValue::undefined());
    }
    let desc_ptr = desc_val.as_js_object_ptr();
    if desc_ptr.is_null() {
        return Err(JsValue::undefined());
    }
    let prop_name_str = coercion::to_string(vm.kernel().string_forge().as_ref(), vm.reg(args[2]));
    let si = vm.kernel().string_forge().intern(&prop_name_str).0;
    let shape_forge = vm.kernel().shape_forge().as_ref();
    let new_shape = shape_forge.make_shape(unsafe { (&*obj_ptr).shape_id() }, si);
    let value = {
        let desc = unsafe { &*desc_ptr };
        desc.hash_props_vec()
            .and_then(|v| v.first().map(|b| **b))
            .unwrap_or(JsValue::undefined())
    };
    {
        let obj = unsafe { &mut *obj_ptr };
        obj.set_shape_id(new_shape);
        obj.ensure_hash_props().push(Box::new(value));
    }
    Ok(obj_val)
}

pub fn object_get_own_property_descriptor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 3 {
        return Err(JsValue::undefined());
    }
    let obj_val = vm.reg(args[1]);
    if !obj_val.is_object() {
        return Ok(JsValue::undefined());
    }
    let obj_ptr = obj_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Ok(JsValue::undefined());
    }
    let prop_name_str = coercion::to_string(vm.kernel().string_forge().as_ref(), vm.reg(args[2]));
    let si = vm.kernel().string_forge().intern(&prop_name_str).0;

    let (found_value, found) = {
        let obj = unsafe { &*obj_ptr };
        let keys = walk_own_keys(vm, obj);
        let mut found_value = JsValue::undefined();
        let mut found = false;
        for (prop_si, offset) in keys {
            if prop_si == si {
                found_value = obj.get_prop_at(offset);
                found = true;
                break;
            }
        }
        (found_value, found)
    };

    if !found {
        return Ok(JsValue::undefined());
    }

    let sf_ptr = vm.kernel().string_forge().as_ref() as *const StringForge;
    let sh_ptr = vm.kernel().shape_forge().as_ref() as *const ShapeForge;
    let desc_proto = vm.kernel().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let desc = vm.epoch().alloc(JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(desc_proto),
    ));
    let true_val = JsValue::bool(true);
    let sf = unsafe { &*sf_ptr };
    let sh = unsafe { &*sh_ptr };

    let d: &mut JsObject = unsafe { &mut *desc };
    let si_val = sf.intern("value").0;
    let sh_val = sh.make_shape(EMPTY_SHAPE_ID, si_val);
    d.set_shape_id(sh_val);
    d.ensure_hash_props().push(Box::new(found_value));

    let si_wr = sf.intern("writable").0;
    let sh_wr = sh.make_shape(sh_val, si_wr);
    d.set_shape_id(sh_wr);
    d.ensure_hash_props().push(Box::new(true_val));

    let si_en = sf.intern("enumerable").0;
    let sh_en = sh.make_shape(sh_wr, si_en);
    d.set_shape_id(sh_en);
    d.ensure_hash_props().push(Box::new(true_val));

    let si_cf = sf.intern("configurable").0;
    let sh_cf = sh.make_shape(sh_en, si_cf);
    d.set_shape_id(sh_cf);
    d.ensure_hash_props().push(Box::new(true_val));

    Ok(JsValue::from_js_object(desc))
}
