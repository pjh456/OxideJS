use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

fn new_obj() -> JsObject {
    JsObject::new_empty(0, JsValue::null())
}

#[test]
fn test_set_frozen() {
    let mut obj = new_obj();
    assert!(!obj.is_frozen());
    obj.set_frozen(true);
    assert!(obj.is_frozen());
}

#[test]
fn test_set_sealed() {
    let mut obj = new_obj();
    assert!(!obj.is_sealed());
    obj.set_sealed(true);
    assert!(obj.is_sealed());
}

#[test]
fn test_frozen_and_sealed_independent() {
    let mut obj = new_obj();
    obj.set_frozen(true);
    assert!(obj.is_frozen());
    assert!(!obj.is_sealed());
    obj.set_sealed(true);
    assert!(obj.is_frozen());
    assert!(obj.is_sealed());
    obj.set_frozen(false);
    assert!(!obj.is_frozen());
    assert!(obj.is_sealed());
}

#[test]
fn test_defaults_are_false() {
    let obj = new_obj();
    assert!(!obj.is_frozen());
    assert!(!obj.is_sealed());
}

#[test]
fn test_clear_frozen() {
    let mut obj = new_obj();
    obj.set_frozen(true);
    obj.set_frozen(false);
    assert!(!obj.is_frozen());
}

#[test]
fn test_clear_sealed() {
    let mut obj = new_obj();
    obj.set_sealed(true);
    obj.set_sealed(false);
    assert!(!obj.is_sealed());
}
