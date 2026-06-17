use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn assert_num(value: JsValue, expected: f64) {
    let actual = if value.is_int() { value.as_int() as f64 } else { value.as_double() };
    assert!((actual - expected).abs() < 0.0001, "expected {expected}, got {actual}");
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
fn class_instance_getter_returns_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { get x() { return 2; } } new A().x").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn class_instance_setter_updates_receiver() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { set x(v) { this.y = v; } } var a = new A(); a.x = 4; a.y").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn class_static_getter_returns_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { static get x() { return 5; } } A.x").unwrap();
    assert_eq!(result.as_int(), 5);
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

#[test]
fn derived_constructor_super_initializes_parent_state() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { constructor(){ this.x = 1 } } class B extends A { constructor(){ super(); this.y = 2 } } let b = new B(); b.x + b.y",
    )
    .unwrap();
    assert_num(result, 3.0);
}

#[test]
fn derived_default_constructor_delegates_to_parent() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor(){ this.x = 1 } } class B extends A {} new B().x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn derived_constructor_this_before_super_throws_reference_error() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "class A { constructor(){ this.x = 1 } } class B extends A { constructor(){ this.x = 2; super(); } } new B()",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

#[test]
fn derived_method_super_call_uses_current_receiver() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { m(){ return this.x } } class B extends A { constructor(){ super(); this.x = 2 } m(){ return super.m() + 1 } } new B().m()",
    )
    .unwrap();
    assert_num(result, 3.0);
}

#[test]
fn static_method_is_callable_on_constructor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { static m(){ return 1 } } A.m()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn static_and_instance_methods_do_not_collide() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { m(){ return 1 } static m(){ return 2 } } let a = new A(); a.m() + A.m()",
    )
    .unwrap();
    assert_num(result, 3.0);
}

#[test]
fn derived_static_method_super_call_resolves_parent_constructor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { static m(){ return 1 } } class B extends A { static n(){ return super.m() + 1 } } B.n()",
    )
    .unwrap();
    assert_num(result, 2.0);
}

#[test]
fn inherited_static_method_is_found_on_constructor_chain() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { static m(){ return 4 } } class B extends A {} B.m()").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn public_class_field_initializer_sets_own_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { x = 1; } new C().x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn public_class_field_without_initializer_is_undefined() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { x; } typeof new C().x").unwrap();
    let ty = vm.lookup_str(result).expect("typeof result should be string");
    assert_eq!(ty, "undefined");
}

#[test]
fn public_class_fields_run_before_base_constructor_body() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { x = this.y; constructor(){ this.y = 2; } } typeof new C().x").unwrap();
    let ty = vm.lookup_str(result).expect("typeof result should be string");
    assert_eq!(ty, "undefined");
}

#[test]
fn public_class_fields_run_after_super_in_derived_constructor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class B { constructor(){ this.b = 1; } } class D extends B { x = this.b; } new D().x",
    )
    .unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn static_class_field_sets_constructor_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { static x = 1; } C.x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn static_class_block_binds_this_to_constructor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { static x = 1; static { this.y = this.x + 1; } static z = this.y + 1; } C.z",
    )
    .unwrap();
    assert_num(result, 3.0);
}

#[test]
fn computed_public_class_method_key_is_supported() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var k = 'm'; class C { [k](){ return 3; } } new C().m()").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn computed_public_class_field_key_is_supported() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var k = 'x'; class C { [k] = 5; } new C().x").unwrap();
    assert_eq!(result.as_int(), 5);
}

#[test]
fn computed_static_class_field_key_is_supported() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var k = 'x'; class C { static [k] = 7; } C.x").unwrap();
    assert_eq!(result.as_int(), 7);
}
