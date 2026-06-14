use std::hash::{Hash, Hasher};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::native::NativeResult;
use crate::vm::Vm;

const SET_PROP_INDEX: u8 = 0;

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

/// Retrieve the `IndexSet` pointer stored at `SET_PROP_INDEX` of a Set object.
///
/// # Safety contract maintained by callers
///
/// The pointer is valid as long as the Set `JsObject` itself is alive. The `JsObject`
/// lives in the current `Epoch` arena and `Epoch::reset()` is never called while a
/// native builtin is executing. The `Box<IndexSet>` is allocated in `new_set_inner()`
/// and is never freed during the Set's lifetime. Only one live `*mut` alias exists per
/// Set object at a time because native calls are single-threaded.
fn get_set_inner(vm: &mut Vm, this_val: JsValue) -> Result<*mut indexmap::IndexSet<SetKey>, JsValue> {
    if !this_val.is_object() {
        return Err(crate::builtins::error::create_type_error(vm, "called on non-Set object"));
    }
    let set_ptr = this_val.as_js_object_ptr();
    if set_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "Set internal state invalid"));
    }
    // SAFETY: set_ptr is a non-null, aligned pointer to a JsObject bump-allocated in the
    // current Epoch. Remains valid for the duration of this call.
    let set_obj = unsafe { &*set_ptr };
    if !set_obj.is_set() {
        return Err(crate::builtins::error::create_type_error(
            vm,
            "Set.prototype.add called on incompatible receiver",
        ));
    }
    let inner_val = set_obj.get_prop_at(SET_PROP_INDEX);
    if !inner_val.is_object() {
        return Err(crate::builtins::error::create_type_error(vm, "Set internal state invalid"));
    }
    // SAFETY: inner_val holds the raw pointer written by `alloc_set` as
    // `JsValue::object(Box::into_raw(Box::new(IndexSet::new())) as *mut u8)`.
    // The pointer is a valid, heap-allocated `Box<IndexSet<SetKey>>` and is never freed
    // during the Set object's lifetime (see module safety contract above).
    // Alignment: IndexSet requires at most 8-byte alignment; the global allocator satisfies this.
    let inner_ptr = inner_val.as_js_object_ptr() as *mut indexmap::IndexSet<SetKey>;
    if inner_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "Set internal state invalid"));
    }
    Ok(inner_ptr)
}

/// Allocate a new `IndexSet` on the global heap.
/// The caller is responsible for storing the pointer in the Set `JsObject` at `SET_PROP_INDEX`
/// via `JsValue::object(ptr as *mut u8)` and ensuring it is never double-freed.
fn new_set_inner() -> *mut indexmap::IndexSet<SetKey> {
    Box::into_raw(Box::new(indexmap::IndexSet::new()))
}

fn alloc_set(vm: &mut Vm) -> *mut JsObject {
    let set_proto = vm.session().builtin_world().set_proto.as_ptr() as *mut JsObject;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(set_proto));
    obj.set_set(true);
    // SAFETY: new_set_inner() returns a valid Box<IndexSet> pointer; stored as a JsValue
    // object tag so get_set_inner() can retrieve and cast it back (same provenance).
    let inner = new_set_inner();
    obj.set_prop_at(SET_PROP_INDEX, JsValue::object(inner as *mut u8));
    vm.epoch().alloc(obj)
}

pub fn set_constructor(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let set_obj = alloc_set(vm);
    NativeResult::Ok(JsValue::from_js_object(set_obj))
}

pub fn set_add(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    unsafe {
        (*inner).insert(SetKey(val));
    }
    NativeResult::Ok(this_val)
}

pub fn set_has(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let found = unsafe { (*inner).contains(&SetKey(val)) };
    NativeResult::Ok(JsValue::bool(found))
}

pub fn set_delete(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    let val = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let removed = unsafe { (*inner).shift_remove(&SetKey(val)) };
    NativeResult::Ok(JsValue::bool(removed))
}

pub fn set_clear(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    unsafe {
        (*inner).clear();
    }
    NativeResult::Ok(JsValue::undefined())
}

pub fn set_size(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = native_try!(get_set_inner(vm, this_val));
    NativeResult::Ok(JsValue::float(unsafe { (*inner).len() } as f64))
}
