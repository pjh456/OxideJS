use oxide_types::value::JsValue;
use oxide_vm::builtins::map::{
    map_clear, map_constructor as new_map, map_delete, map_get, map_has, map_set, map_size,
};
use oxide_vm::vm::Vm;

#[test]
fn tmap_constructor_returns_object() {
    let mut vm = Vm::new();
    let r = new_map(&mut vm, &[0]).unwrap();
    assert!(r.is_object());
}

#[test]
fn tmap_set_get() {
    let mut vm = Vm::new();
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    vm.set_reg(1, JsValue::float(42.0));
    vm.set_reg(2, JsValue::float(100.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    assert_eq!(map_get(&mut vm, &[0, 1]).unwrap().as_double(), 100.0);
}

#[test]
fn tmap_get_missing() {
    let mut vm = Vm::new();
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    vm.set_reg(1, JsValue::float(99.0));
    assert!(map_get(&mut vm, &[0, 1]).unwrap().is_undefined());
}

#[test]
fn tmap_has_check() {
    let mut vm = Vm::new();
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    vm.set_reg(1, JsValue::float(1.0));
    vm.set_reg(2, JsValue::float(2.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    assert!(map_has(&mut vm, &[0, 1]).unwrap().as_bool());
    assert!(!map_has(&mut vm, &[0, 99]).unwrap().as_bool());
}

#[test]
fn tmap_delete_works() {
    let mut vm = Vm::new();
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    vm.set_reg(1, JsValue::float(7.0));
    vm.set_reg(2, JsValue::float(8.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    assert!(map_delete(&mut vm, &[0, 1]).unwrap().as_bool());
    assert!(!map_has(&mut vm, &[0, 1]).unwrap().as_bool());
}

#[test]
fn tmap_size_and_clear() {
    let mut vm = Vm::new();
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    assert_eq!(map_size(&mut vm, &[0]).unwrap().as_double(), 0.0);
    vm.set_reg(1, JsValue::float(1.0));
    vm.set_reg(2, JsValue::float(10.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    vm.set_reg(1, JsValue::float(2.0));
    vm.set_reg(2, JsValue::float(20.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    assert_eq!(map_size(&mut vm, &[0]).unwrap().as_double(), 2.0);
    map_clear(&mut vm, &[0]).unwrap();
    assert_eq!(map_size(&mut vm, &[0]).unwrap().as_double(), 0.0);
}
