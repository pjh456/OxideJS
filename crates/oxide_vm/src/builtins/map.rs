use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::builtins::set::SetKey;
use crate::native::NativeResult;
use crate::vm::Vm;

const MAP_PROP_INDEX: u8 = 0;

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

/// Retrieve the `IndexMap` pointer stored at `MAP_PROP_INDEX` of a Map object.
///
/// # Safety contract maintained by callers
///
/// The pointer is valid as long as the Map `JsObject` itself is alive, which is guaranteed
/// because the `JsObject` lives in the current `Epoch` arena and `Epoch::reset()` is never
/// called while a native builtin is executing. The `Box<IndexMap>` that owns the allocation
/// is created in `new_map_inner()` and is never freed until the process exits (intentional
/// leak — lifetime is tied to the epoch). Only one live `*mut` alias exists at a time
/// per Map object because native calls are single-threaded.
fn get_map_inner(vm: &mut Vm, this_val: JsValue) -> Result<*mut indexmap::IndexMap<SetKey, JsValue>, JsValue> {
    if !this_val.is_object() {
        return Err(crate::builtins::error::create_type_error(vm, "called on non-Map object"));
    }
    let map_ptr = this_val.as_js_object_ptr();
    if map_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "Map internal state invalid"));
    }
    // SAFETY: map_ptr is a non-null, aligned pointer to a JsObject bump-allocated in the
    // current Epoch. It remains valid for the duration of this call (epoch is not reset
    // during native execution).
    let map_obj = unsafe { &*map_ptr };
    if !map_obj.is_map() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "Map.prototype method called on incompatible receiver",
        ));
    }
    let inner_val = map_obj.get_prop_at(MAP_PROP_INDEX);
    if !inner_val.is_object() {
        return Err(crate::builtins::error::create_type_error(vm, "Map internal state invalid"));
    }
    // SAFETY: inner_val holds the raw pointer written by `alloc_map` as
    // `JsValue::object(Box::into_raw(Box::new(IndexMap::new())) as *mut u8)`.
    // The pointer is a valid, heap-allocated `Box<IndexMap<SetKey, JsValue>>` and is
    // never freed during the Map object's lifetime (see module safety contract above).
    // Alignment: IndexMap requires at most 8-byte alignment; the global allocator satisfies this.
    let inner_ptr = inner_val.as_js_object_ptr() as *mut indexmap::IndexMap<SetKey, JsValue>;
    if inner_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "Map internal state invalid"));
    }
    Ok(inner_ptr)
}

/// Allocate a new `IndexMap` on the global heap.
/// The caller is responsible for storing the pointer in the Map `JsObject` at `MAP_PROP_INDEX`
/// via `JsValue::object(ptr as *mut u8)` and ensuring it is never double-freed.
fn new_map_inner() -> *mut indexmap::IndexMap<SetKey, JsValue> {
    Box::into_raw(Box::new(indexmap::IndexMap::new()))
}

fn alloc_map(vm: &mut Vm) -> *mut JsObject {
    let map_proto = vm.session().builtin_world().map_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(map_proto));
    obj.set_map(true);
    // SAFETY: new_map_inner() returns a valid Box<IndexMap> pointer; stored as a JsValue
    // object tag so get_map_inner() can retrieve and cast it back (same provenance).
    let inner = new_map_inner();
    obj.set_prop_at(MAP_PROP_INDEX, JsValue::object(inner as *mut u8));
    vm.epoch().alloc(obj)
}

pub fn map_constructor(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let map_obj = alloc_map(vm);
    NativeResult::Ok(JsValue::from_js_object(map_obj))
}

pub fn map_set(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let val = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    unsafe {
        (*inner).insert(SetKey(key), val);
    }
    NativeResult::Ok(this_val)
}

pub fn map_get(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).get(&SetKey(key)).copied() };
    NativeResult::Ok(found.unwrap_or(JsValue::undefined()))
}

pub fn map_has(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).contains_key(&SetKey(key)) };
    NativeResult::Ok(JsValue::bool(found))
}

pub fn map_delete(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let removed = unsafe { (*inner).shift_remove(&SetKey(key)) };
    NativeResult::Ok(JsValue::bool(removed.is_some()))
}

pub fn map_clear(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    unsafe {
        (*inner).clear();
    }
    NativeResult::Ok(JsValue::undefined())
}

pub fn map_size(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    NativeResult::Ok(JsValue::float(unsafe { (*inner).len() } as f64))
}
