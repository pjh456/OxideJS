use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

#[test]
fn class_declaration_is_function_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A {} ; typeof A").unwrap();
    let ty = vm.lookup_str(result).expect("typeof result should be string");
    assert_eq!(ty, "function");
}

#[test]
fn class_method_is_on_prototype() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { m() { return 1; } } new A().m()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn class_constructor_initializes_instance_state() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor(x) { this.x = x; } } new A(3).x").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn class_expression_constructs_instances() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const C = class Foo { method() { return 2; } }; new C().method()").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn class_expression_inner_name_is_visible_in_method_body() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const C = class Foo { self() { return Foo; } }; new C().self() === C").unwrap();
    assert!(
        result.is_bool() && result.as_bool(),
        "expected inner class name to resolve to constructor"
    );
}

#[test]
fn class_constructor_cannot_be_called_without_new() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class A {} ; A()").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

#[test]
fn class_constructor_object_return_overrides_instance() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor() { return { x: 1 }; } } new A().x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn class_constructor_primitive_return_preserves_instance() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor() { this.x = 1; return 5; } } new A().x").unwrap();
    assert_eq!(result.as_int(), 1);
}
