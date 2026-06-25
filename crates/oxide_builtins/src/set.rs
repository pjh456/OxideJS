use std::hash::{Hash, Hasher};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

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
        // SAFETY: JsValue is an 8-byte NaN-boxed Copy value; raw bits define non-double identity here.
        unsafe { std::mem::transmute::<JsValue, u64>(self.0) == std::mem::transmute::<JsValue, u64>(other.0) }
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
            // SAFETY: JsValue is an 8-byte NaN-boxed Copy value; hashing raw bits matches equality above.
            unsafe { std::mem::transmute::<JsValue, u64>(self.0).hash(state) }
        }
    }
}

type SetInner = indexmap::IndexSet<SetKey>;

/// Retrieve the `IndexSet` pointer stored in a Set object's native-data slot.
///
/// # Safety contract maintained by callers
///
/// The pointer is valid as long as the Set `JsObject` itself is alive. The `JsObject`
/// lives in the current `Epoch` arena and `Epoch::reset()` is never called while a
/// native builtin is executing. The `Box<IndexSet>` is allocated in `new_set_inner()`
/// and is never freed during the Set's lifetime. Only one live `*mut` alias exists per
/// Set object at a time because native calls are single-threaded.
fn get_set_inner<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<*mut SetInner, JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "called on non-Set object"));
    }
    let set_ptr = this_val.as_js_object_ptr();
    if set_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "Set internal state invalid"));
    }
    // SAFETY: set_ptr is a non-null, aligned pointer to a JsObject bump-allocated in the
    // current Epoch. Remains valid for the duration of this call.
    let set_obj = unsafe { &*set_ptr };
    if !set_obj.is_set() {
        return Err(crate::error::create_type_error(vm, "Set.prototype.add called on incompatible receiver"));
    }
    // SAFETY: native_data holds the raw pointer written by `alloc_set`.
    // The pointer is a valid, heap-allocated `Box<IndexSet<SetKey>>`.
    // Alignment: IndexSet requires at most 8-byte alignment; the global allocator satisfies this.
    let inner_ptr = set_obj.native_data() as *mut SetInner;
    if inner_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "Set internal state invalid"));
    }
    Ok(inner_ptr)
}

fn new_set_inner() -> *mut SetInner {
    Box::into_raw(Box::new(SetInner::new()))
}

fn alloc_set<H: VmHost>(vm: &mut H) -> *mut JsObject {
    let set_proto = vm.session().builtin_world().set_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(set_proto));
    obj.set_set(true);
    let inner = new_set_inner();
    obj.set_native_data(inner as *mut u8);
    vm.alloc_object(obj)
}

pub fn set_native_edges(obj: &JsObject) -> Vec<JsValue> {
    if !obj.is_set() {
        return Vec::new();
    }
    let inner = obj.native_data() as *const SetInner;
    if inner.is_null() {
        return Vec::new();
    }
    unsafe {
        (*inner)
            .iter()
            .map(|key| key.0)
            .filter(|value: &JsValue| value.is_object())
            .collect()
    }
}

pub fn clone_set_native_with_rewrite<F>(src: &JsObject, dst: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !src.is_set() {
        return;
    }
    let inner = src.native_data() as *const SetInner;
    if inner.is_null() {
        dst.set_native_data(std::ptr::null_mut());
        return;
    }
    let mut cloned = SetInner::new();
    unsafe {
        for key in (*inner).iter() {
            let new_key = if key.0.is_object() { SetKey(rewrite(key.0)) } else { *key };
            cloned.insert(new_key);
        }
    }
    dst.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

pub fn rewrite_set_native<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !obj.is_set() {
        return;
    }
    let inner = obj.native_data() as *mut SetInner;
    if inner.is_null() {
        return;
    }
    unsafe {
        let mut rewritten = SetInner::with_capacity((*inner).len());
        for key in (*inner).iter() {
            let new_key = if key.0.is_object() { SetKey(rewrite(key.0)) } else { *key };
            rewritten.insert(new_key);
        }
        *inner = rewritten;
    }
}

pub fn drop_set_native(obj: &mut JsObject) -> u64 {
    if !obj.is_set() {
        return 0;
    }
    let inner = obj.native_data() as *mut SetInner;
    if inner.is_null() {
        return 0;
    }
    unsafe {
        let boxed: Box<SetInner> = Box::from_raw(inner);
        let bytes = std::mem::size_of::<SetInner>() + boxed.capacity() * std::mem::size_of::<SetKey>();
        drop(boxed);
        obj.set_native_data(std::ptr::null_mut());
        bytes as u64
    }
}

pub fn set_constructor<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let set_obj = alloc_set(vm);
    NativeResult::Ok(JsValue::from_js_object(set_obj))
}

pub fn set_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    unsafe {
        (*inner).insert(SetKey(val));
    }
    NativeResult::Ok(this_val)
}

pub fn set_has<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).contains(&SetKey(val)) };
    NativeResult::Ok(JsValue::bool(found))
}

pub fn set_delete<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let removed = unsafe { (*inner).shift_remove(&SetKey(val)) };
    NativeResult::Ok(JsValue::bool(removed))
}

pub fn set_clear<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    unsafe {
        (*inner).clear();
    }
    NativeResult::Ok(JsValue::undefined())
}

pub fn set_size<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    NativeResult::Ok(JsValue::float(unsafe { (*inner).len() } as f64))
}
