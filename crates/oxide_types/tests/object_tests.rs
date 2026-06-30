use oxide_types::object::Cell;
use oxide_types::value::JsValue;

#[test]
fn cell_layout_size_check() {
    assert_eq!(std::mem::size_of::<Cell>(), 16);
}

#[test]
fn cell_initialized_flag() {
    let c = Cell::new(JsValue::int(42), true);
    assert!(c.is_initialized());
}

#[test]
fn cell_uninitialized_flag() {
    let c = Cell::new(JsValue::undefined(), false);
    assert!(!c.is_initialized());
}

#[test]
fn cell_set_initialized_toggle() {
    let mut c = Cell::new(JsValue::int(1), false);
    assert!(!c.is_initialized());
    c.set_initialized(true);
    assert!(c.is_initialized());
    c.set_initialized(false);
    assert!(!c.is_initialized());
}

#[test]
fn cell_gc_mark_flag() {
    let mut c = Cell::new(JsValue::undefined(), false);
    assert!(!c.is_gc_marked());
    c.set_gc_mark(true);
    assert!(c.is_gc_marked());
    c.set_gc_mark(false);
    assert!(!c.is_gc_marked());
}
