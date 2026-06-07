use std::hash::{Hash, Hasher};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::native::NativeResult;
use crate::vm::Vm;

const SET_PROP_INDEX: u8 = 0;

#[derive(Clone, Copy)]
pub struct SetKey(pub JsValue);

impl PartialEq for SetKey {
    fn eq(&self, other: &Self) -> bool {
        if self.0.is_double() && other.0.is_double() {
            let a = self.0.as_double();
            let b = other.0.as_double();
            if a.is_nan() && b.is_nan() {
                return true;
            }
            if a == 0.0 && b == 0.0 {
                return true;
            }
            return a == b;
        }
        unsafe {
            std::mem::transmute::<JsValue, u64>(self.0)
                == std::mem::transmute::<JsValue, u64>(other.0)
        }
    }
}

impl Eq for SetKey {}

impl Hash for SetKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if self.0.is_double() {
            let d = self.0.as_double();
            if d.is_nan() {
                0u64.hash(state);
            } else if d == 0.0 {
                0.0f64.to_bits().hash(state);
            } else {
                d.to_bits().hash(state);
            }
        } else {
            unsafe { std::mem::transmute::<JsValue, u64>(self.0).hash(state) }
        }
    }
}

fn get_set_inner(
    vm: &mut Vm,
    this_val: JsValue,
) -> Result<*mut indexmap::IndexSet<SetKey>, JsValue> {
    if !this_val.is_object() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "called on non-Set object",
        ));
    }
    let set_ptr = this_val.as_js_object_ptr();
    if set_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "Set internal state invalid",
        ));
    }
    let set_obj = unsafe { &*set_ptr };
    let inner_val = set_obj.get_prop_at(SET_PROP_INDEX);
    if !inner_val.is_object() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "Set internal state invalid",
        ));
    }
    let inner_ptr = inner_val.as_js_object_ptr() as *mut indexmap::IndexSet<SetKey>;
    if inner_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "Set internal state invalid",
        ));
    }
    Ok(inner_ptr)
}

fn new_set_inner() -> *mut indexmap::IndexSet<SetKey> {
    Box::into_raw(Box::new(indexmap::IndexSet::new()))
}

fn alloc_set(vm: &mut Vm) -> *mut JsObject {
    let set_proto = vm.kernel().builtin_world().set_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(set_proto));
    let inner = new_set_inner();
    obj.set_prop_at(SET_PROP_INDEX, JsValue::object(inner as *mut u8));
    vm.epoch().alloc(obj)
}

pub fn set_constructor(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let set_obj = alloc_set(vm);
    Ok(JsValue::from_js_object(set_obj))
}

pub fn set_add(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_set_inner(vm, this_val)?;
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    unsafe {
        (*inner).insert(SetKey(val));
    }
    Ok(this_val)
}

pub fn set_has(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_set_inner(vm, this_val)?;
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).contains(&SetKey(val)) };
    Ok(JsValue::bool(found))
}

pub fn set_delete(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_set_inner(vm, this_val)?;
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let removed = unsafe { (*inner).shift_remove(&SetKey(val)) };
    Ok(JsValue::bool(removed))
}

pub fn set_clear(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_set_inner(vm, this_val)?;
    unsafe {
        (*inner).clear();
    }
    Ok(JsValue::undefined())
}

pub fn set_size(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = get_set_inner(vm, this_val)?;
    Ok(JsValue::float(unsafe { (*inner).len() } as f64))
}
