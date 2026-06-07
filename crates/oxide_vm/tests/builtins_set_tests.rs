use oxide_types::value::JsValue;
use oxide_vm::builtins::set::{
    set_add, set_clear, set_constructor as new_set, set_delete, set_has, set_size,
};
use oxide_vm::vm::Vm;

#[test]
fn tset_constructor_returns_object() {
    let mut vm = Vm::new();
    let r = new_set(&mut vm, &[0]).unwrap();
    assert!(r.is_object());
}

#[test]
fn tset_add_has() {
    let mut vm = Vm::new();
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(42.0));
    let _ = set_add(&mut vm, &[0, 1]).unwrap();
    assert!(set_has(&mut vm, &[0, 1]).unwrap().as_bool());
}

#[test]
fn tset_has_missing() {
    let mut vm = Vm::new();
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(99.0));
    assert!(!set_has(&mut vm, &[0, 1]).unwrap().as_bool());
}

#[test]
fn tset_delete_works() {
    let mut vm = Vm::new();
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(7.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert!(set_delete(&mut vm, &[0, 1]).unwrap().as_bool());
    assert!(!set_has(&mut vm, &[0, 1]).unwrap().as_bool());
}

#[test]
fn tset_size_increases() {
    let mut vm = Vm::new();
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 0.0);
    vm.set_reg(1, JsValue::float(1.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    vm.set_reg(1, JsValue::float(2.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 2.0);
}

#[test]
fn tset_clear_works() {
    let mut vm = Vm::new();
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(1.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    set_clear(&mut vm, &[0]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 0.0);
}

#[test]
fn tset_nan_equality() {
    let mut vm = Vm::new();
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(f64::NAN));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 1.0);
    set_add(&mut vm, &[0, 1]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 1.0);
}

#[test]
fn tset_signed_zero() {
    let mut vm = Vm::new();
    let s = new_set(&mut vm, &[0]).unwrap();
    vm.set_reg(0, s);
    vm.set_reg(1, JsValue::float(0.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    vm.set_reg(1, JsValue::float(-0.0));
    set_add(&mut vm, &[0, 1]).unwrap();
    assert_eq!(set_size(&mut vm, &[0]).unwrap().as_double(), 1.0);
}
