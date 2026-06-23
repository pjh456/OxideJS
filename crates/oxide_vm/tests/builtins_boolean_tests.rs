use oxide_compiler::compiler::Compiler;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use oxide_vm::builtins::boolean::{boolean_constructor, boolean_prototype_to_string, boolean_prototype_value_of};
use oxide_vm::vm::Vm;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn str_val(vm: &Vm, val: JsValue) -> String {
    vm.kernel_core()
        .string_forge()
        .lookup(val.as_string_index())
        .unwrap_or_default()
}

// -- direct native fn tests --

fn bool_ctor_call(vm: &mut Vm, arg: JsValue) -> JsValue {
    vm.set_reg(1, arg);
    boolean_constructor(vm, &[0, 1]).unwrap()
}

fn bool_ctor_new(vm: &mut Vm, arg: JsValue) -> JsValue {
    let proto_ptr = vm.session().builtin_world().boolean_proto.as_ptr() as *mut JsObject;
    let wrapper = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(proto_ptr));
    let ptr = vm.epoch().alloc(wrapper);
    let val = JsValue::from_js_object(ptr);
    vm.set_reg(0, val);
    vm.set_reg(1, arg);
    boolean_constructor(vm, &[0, 1]).unwrap()
}

#[test]
fn tboolean_call_true_false() {
    let mut vm = Vm::new();
    assert!(bool_ctor_call(&mut vm, JsValue::float(1.0)).as_bool());
    assert!(!bool_ctor_call(&mut vm, JsValue::float(0.0)).as_bool());
}

#[test]
fn tboolean_call_empty_string() {
    let mut vm = Vm::new();
    let empty = vm.intern("");
    assert!(!bool_ctor_call(&mut vm, empty).as_bool());
}

#[test]
fn tboolean_new_value_of() {
    let mut vm = Vm::new();
    let w = bool_ctor_new(&mut vm, JsValue::float(1.0));
    vm.set_reg(0, w);
    assert!(boolean_prototype_value_of(&mut vm, &[0]).unwrap().as_bool());
    let w2 = bool_ctor_new(&mut vm, JsValue::float(0.0));
    vm.set_reg(0, w2);
    assert!(!boolean_prototype_value_of(&mut vm, &[0]).unwrap().as_bool());
}

#[test]
fn tboolean_new_tostring() {
    let mut vm = Vm::new();
    let w = bool_ctor_new(&mut vm, JsValue::float(1.0));
    vm.set_reg(0, w);
    let r = boolean_prototype_to_string(&mut vm, &[0]).unwrap();
    assert_eq!(str_val(&vm, r), "true");
    let w2 = bool_ctor_new(&mut vm, JsValue::float(0.0));
    vm.set_reg(0, w2);
    let r2 = boolean_prototype_to_string(&mut vm, &[0]).unwrap();
    assert_eq!(str_val(&vm, r2), "false");
}

// -- JS eval tests using new keyword --

#[test]
fn boolean_call_true() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Boolean(1)").unwrap();
    assert!(r.as_bool());
}

#[test]
fn boolean_call_false() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Boolean(0)").unwrap();
    assert!(!r.as_bool());
}

#[test]
fn boolean_new_value_of() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Boolean(1).valueOf()").unwrap();
    assert!(r.as_bool());
}

#[test]
fn boolean_new_to_string() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Boolean(true).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "true");
    let r = eval(&mut vm, "new Boolean(false).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "false");
}

#[test]
fn boolean_primitive_to_string_autoboxes() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "true.toString()").unwrap();
    assert_eq!(str_val(&vm, r), "true");

    let r = eval(&mut vm, "false.toString()").unwrap();
    assert_eq!(str_val(&vm, r), "false");
}

#[test]
fn boolean_primitive_value_of_autoboxes() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "false.valueOf()").unwrap();
    assert_eq!(r, JsValue::bool(false));

    let r = eval(&mut vm, "(true).constructor === Boolean").unwrap();
    assert_eq!(r, JsValue::bool(true));
}

#[test]
fn boolean_boxes_participate_in_numeric_coercion() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "+new Boolean(true)").unwrap();
    assert_eq!(r.as_double(), 1.0);

    let r = eval(&mut vm, "new Boolean(false) | 0").unwrap();
    assert_eq!(r.as_int(), 0);
}

#[test]
fn boxed_boolean_valueof_tostring_and_object_e2e() {
    let mut vm = Vm::new();
    assert!(!eval(&mut vm, "new Boolean(false).valueOf()").unwrap().as_bool());
    assert!(eval(&mut vm, "new Boolean(1).valueOf()").unwrap().as_bool());

    let t = eval(&mut vm, "new Boolean(false).toString()").unwrap();
    assert_eq!(str_val(&vm, t), "false");

    let ty = eval(&mut vm, "typeof new Boolean(true)").unwrap();
    assert_eq!(str_val(&vm, ty), "object");
}
