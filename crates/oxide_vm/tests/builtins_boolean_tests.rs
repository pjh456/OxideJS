use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use oxide_vm::builtins::boolean::{
    boolean_constructor, boolean_prototype_to_string, boolean_prototype_value_of,
};
use oxide_vm::vm::Vm;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;

fn bool_ctor_call(vm: &mut Vm, arg: JsValue) -> JsValue {
    vm.set_reg(1, arg);
    boolean_constructor(vm, &[0, 1]).unwrap()
}

fn tbool_wrap(vm: &mut Vm, arg: JsValue) -> JsValue {
    let proto_ptr = vm.kernel().builtin_world().boolean_proto.as_ptr() as *mut JsObject;
    let wrapper = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr));
    let ptr = vm.epoch().alloc(wrapper);
    let val = JsValue::from_js_object(ptr);
    vm.set_reg(0, val);
    vm.set_reg(1, arg);
    boolean_constructor(vm, &[0, 1]).unwrap()
}

#[test]
fn tboolean_call_true() {
    let mut vm = Vm::new();
    assert!(bool_ctor_call(&mut vm, JsValue::float(1.0)).as_bool());
}

#[test]
fn tboolean_call_false() {
    let mut vm = Vm::new();
    assert!(!bool_ctor_call(&mut vm, JsValue::float(0.0)).as_bool());
}

#[test]
fn tboolean_call_empty_is_false() {
    let mut vm = Vm::new();
    let empty = vm.intern("");
    assert!(!bool_ctor_call(&mut vm, empty).as_bool());
}

#[test]
fn tboolean_new_value_of_true() {
    let mut vm = Vm::new();
    let wrapper = tbool_wrap(&mut vm, JsValue::float(1.0));
    vm.set_reg(0, wrapper);
    assert!(boolean_prototype_value_of(&mut vm, &[0]).unwrap().as_bool());
}

#[test]
fn tboolean_new_value_of_false() {
    let mut vm = Vm::new();
    let wrapper = tbool_wrap(&mut vm, JsValue::float(0.0));
    vm.set_reg(0, wrapper);
    assert!(!boolean_prototype_value_of(&mut vm, &[0]).unwrap().as_bool());
}

#[test]
fn tboolean_new_tostring() {
    let mut vm = Vm::new();
    let w = tbool_wrap(&mut vm, JsValue::float(1.0));
    vm.set_reg(0, w);
    let r = boolean_prototype_to_string(&mut vm, &[0]).unwrap();
    let s = vm
        .kernel()
        .string_forge()
        .lookup(r.as_string_index())
        .unwrap();
    assert_eq!(s, "true");

    let w2 = tbool_wrap(&mut vm, JsValue::float(0.0));
    vm.set_reg(0, w2);
    let r2 = boolean_prototype_to_string(&mut vm, &[0]).unwrap();
    let s2 = vm
        .kernel()
        .string_forge()
        .lookup(r2.as_string_index())
        .unwrap();
    assert_eq!(s2, "false");
}
