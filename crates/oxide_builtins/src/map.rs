use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::set::SetKey;

use oxide_runtime_api::{NativeResult, VmHost};

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

type MapInner = indexmap::IndexMap<SetKey, JsValue>;

/// Retrieve the `IndexMap` pointer stored in a Map object's native-data slot.
///
/// # Safety contract maintained by callers
///
/// The pointer is valid as long as the Map `JsObject` itself is alive, which is guaranteed
/// because the `JsObject` lives in the current `Epoch` arena and `Epoch::reset()` is never
/// called while a native builtin is executing. The `Box<IndexMap>` that owns the allocation
/// is created in `new_map_inner()` and is never freed until the process exits (intentional
/// leak — lifetime is tied to the epoch). Only one live `*mut` alias exists at a time
/// per Map object because native calls are single-threaded.
fn get_map_inner<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<*mut MapInner, JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "called on non-Map object"));
    }
    let map_ptr = this_val.as_js_object_ptr();
    if map_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "Map internal state invalid"));
    }
    // SAFETY: map_ptr is a non-null, aligned pointer to a JsObject bump-allocated in the
    // current Epoch. It remains valid for the duration of this call (epoch is not reset
    // during native execution).
    let map_obj = unsafe { &*map_ptr };
    if !map_obj.is_map() {
        return Err(crate::error::create_type_error(
            vm,
            "Map.prototype method called on incompatible receiver",
        ));
    }
    // SAFETY: native_data holds the raw pointer written by `alloc_map`.
    // The pointer is a valid, heap-allocated `Box<IndexMap<SetKey, JsValue>>`.
    // Alignment: IndexMap requires at most 8-byte alignment; the global allocator satisfies this.
    let inner_ptr = map_obj.native_data() as *mut MapInner;
    if inner_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "Map internal state invalid"));
    }
    Ok(inner_ptr)
}

fn new_map_inner() -> *mut MapInner {
    Box::into_raw(Box::new(MapInner::new()))
}

fn alloc_map<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let map_proto = vm.session().builtin_world().map_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(map_proto));
    obj.set_map(true);
    let inner = new_map_inner();
    obj.set_native_data(inner as *mut u8);
    vm.alloc_object(obj)
}

pub fn map_native_edges(obj: &JsObject) -> Vec<JsValue> {
    if !obj.is_map() {
        return Vec::new();
    }
    let inner = obj.native_data() as *const MapInner;
    if inner.is_null() {
        return Vec::new();
    }
    unsafe {
        (*inner)
            .iter()
            .flat_map(|(key, value)| [key.0, *value])
            .filter(|value: &JsValue| value.is_object())
            .collect()
    }
}

pub fn clone_map_native_with_rewrite<F>(src: &JsObject, dst: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !src.is_map() {
        return;
    }
    let inner = src.native_data() as *const MapInner;
    if inner.is_null() {
        dst.set_native_data(std::ptr::null_mut());
        return;
    }
    let mut cloned = MapInner::new();
    unsafe {
        for (key, value) in (*inner).iter() {
            let new_key = if key.0.is_object() { SetKey(rewrite(key.0)) } else { *key };
            let new_value = if value.is_object() { rewrite(*value) } else { *value };
            cloned.insert(new_key, new_value);
        }
    }
    dst.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

pub fn rewrite_map_native<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !obj.is_map() {
        return;
    }
    let inner = obj.native_data() as *mut MapInner;
    if inner.is_null() {
        return;
    }
    unsafe {
        let mut rewritten = MapInner::with_capacity((*inner).len());
        for (key, value) in (*inner).iter() {
            let new_key = if key.0.is_object() { SetKey(rewrite(key.0)) } else { *key };
            let new_value = if value.is_object() { rewrite(*value) } else { *value };
            rewritten.insert(new_key, new_value);
        }
        *inner = rewritten;
    }
}

pub fn drop_map_native(obj: &mut JsObject) -> u64 {
    if !obj.is_map() {
        return 0;
    }
    let inner = obj.native_data() as *mut MapInner;
    if inner.is_null() {
        return 0;
    }
    unsafe {
        let boxed: Box<MapInner> = Box::from_raw(inner);
        let bytes = std::mem::size_of::<MapInner>() + boxed.capacity() * std::mem::size_of::<(SetKey, JsValue)>();
        drop(boxed);
        obj.set_native_data(std::ptr::null_mut());
        bytes as u64
    }
}

pub fn map_constructor<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let map_obj = alloc_map(vm);
    NativeResult::Ok(JsValue::from_js_object(map_obj))
}

pub fn map_set<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let val = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    unsafe {
        (*inner).insert(SetKey(key), val);
    }
    NativeResult::Ok(this_val)
}

pub fn map_get<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).get(&SetKey(key)).copied() };
    NativeResult::Ok(found.unwrap_or(JsValue::undefined()))
}

pub fn map_has<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).contains_key(&SetKey(key)) };
    NativeResult::Ok(JsValue::bool(found))
}

pub fn map_delete<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    let key = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let removed = unsafe { (*inner).shift_remove(&SetKey(key)) };
    NativeResult::Ok(JsValue::bool(removed.is_some()))
}

pub fn map_clear<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    unsafe {
        (*inner).clear();
    }
    NativeResult::Ok(JsValue::undefined())
}

pub fn map_size<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_map_inner(vm, this_val));
    NativeResult::Ok(JsValue::float(unsafe { (*inner).len() } as f64))
}
