use super::*;
use crate::object::JsObject;
use crate::shape::EMPTY_SHAPE_ID;
use crate::value::JsValue;

#[test]
fn is_epoch_ptr_returns_true_for_epoch_object() {
    let epoch = Epoch::new();
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
    let ptr = epoch.alloc(obj);

    assert!(epoch.is_epoch_ptr(ptr.cast::<u8>()));
}

#[test]
fn is_epoch_ptr_returns_false_for_heap_and_stack_pointers() {
    let epoch = Epoch::new();
    let persistent = P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()));
    let stack_value = 7i32;

    assert!(!epoch.is_epoch_ptr(persistent.as_ptr().cast::<u8>()));
    assert!(!epoch.is_epoch_ptr((&stack_value as *const i32).cast::<u8>()));
    assert!(!epoch.is_epoch_ptr(std::ptr::null()));
}
