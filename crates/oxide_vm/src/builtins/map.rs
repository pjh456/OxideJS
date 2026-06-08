use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::builtins::set::SetKey;
use crate::native::NativeResult;
use crate::vm::Vm;

const MAP_PROP_INDEX: u8 = 0;

fn get_map_inner(vm: &mut Vm, this_val: JsValue) -> Result<*mut indexmap::IndexMap<SetKey, JsValue>, JsValue> {
    if !this_val.is_object() {
        return Err(crate::builtins::error::create_type_error(vm, "called on non-Map object"));
    }
    let map_ptr = this_val.as_js_object_ptr();
    if map_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "Map internal state invalid"));
    }
    let map_obj = unsafe { &*map_ptr };
    let inner_val = map_obj.get_prop_at(MAP_PROP_INDEX);
    if !inner_val.is_object() {
        return Err(crate::builtins::error::create_type_error(vm, "Map internal state invalid"));
    }
    let inner_ptr = inner_val.as_js_object_ptr() as *mut indexmap::IndexMap<SetKey, JsValue>;
    if inner_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "Map internal state invalid"));
    }
    Ok(inner_ptr)
}

fn new_map_inner() -> *mut indexmap::IndexMap<SetKey, JsValue> {
    Box::into_raw(Box::new(indexmap::IndexMap::new()))
}

fn alloc_map(vm: &mut Vm) -> *mut JsObject {
    let map_proto = vm.kernel().builtin_world().map_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(map_proto));
    let inner = new_map_inner();
    obj.set_prop_at(MAP_PROP_INDEX, JsValue::object(inner as *mut u8));
    vm.epoch().alloc(obj)
}

pub fn map_constructor(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let map_obj = alloc_map(vm);
    Ok(JsValue::from_js_object(map_obj))
}

pub fn map_set(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_map_inner(vm, this_val)?;
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let val = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    unsafe {
        (*inner).insert(SetKey(key), val);
    }
    Ok(this_val)
}

pub fn map_get(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_map_inner(vm, this_val)?;
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).get(&SetKey(key)).copied() };
    Ok(found.unwrap_or(JsValue::undefined()))
}

pub fn map_has(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_map_inner(vm, this_val)?;
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).contains_key(&SetKey(key)) };
    Ok(JsValue::bool(found))
}

pub fn map_delete(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_map_inner(vm, this_val)?;
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let removed = unsafe { (*inner).shift_remove(&SetKey(key)) };
    Ok(JsValue::bool(removed.is_some()))
}

pub fn map_clear(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_map_inner(vm, this_val)?;
    unsafe {
        (*inner).clear();
    }
    Ok(JsValue::undefined())
}

pub fn map_size(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_map_inner(vm, this_val)?;
    Ok(JsValue::float(unsafe { (*inner).len() } as f64))
}
