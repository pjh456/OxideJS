use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::builtins::map::{map_clear, map_constructor as new_map, map_delete, map_get, map_has, map_set, map_size};
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn str_val(vm: &Vm, val: JsValue) -> String {
    vm.kernel_core().string_forge().lookup(val.as_string_index()).unwrap_or_default()
}

// -- direct native fn tests --

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
    assert!(map_get(&mut vm, &[0, 99]).unwrap().is_undefined());
}

#[test]
fn tmap_has_and_delete() {
    let mut vm = Vm::new();
    let m = new_map(&mut vm, &[0]).unwrap();
    vm.set_reg(0, m);
    vm.set_reg(1, JsValue::float(7.0));
    vm.set_reg(2, JsValue::float(8.0));
    map_set(&mut vm, &[0, 1, 2]).unwrap();
    assert!(map_has(&mut vm, &[0, 1]).unwrap().as_bool());
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

// -- JS eval tests using new keyword (all in one eval per test) --

#[test]
fn map_new_set_get() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set('k', 'v'); m.get('k')").unwrap();
    assert_eq!(str_val(&vm, r), "v");
}

#[test]
fn map_new_get_missing() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.get('nope')").unwrap();
    assert!(r.is_undefined());
}

#[test]
fn map_new_has() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set('a', 1); m.has('a')").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "var m2 = new Map(); m2.has('b')").unwrap();
    assert!(!r.as_bool());
}

#[test]
fn map_new_delete() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var m = new Map(); m.set('x', 10); m.delete('x'); m.has('x')").unwrap();
    assert!(!r.as_bool());
}
