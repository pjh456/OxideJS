use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

#[test]
fn let_block_scoping_outer_unchanged() {
    let result = eval("let x = 1; { let x = 2; } x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn const_reassignment_throws() {
    let result = eval("const x = 1; x = 2");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("TypeError") || err.contains("constant"), "got: {}", err);
}

#[test]
fn var_block_not_isolated() {
    let result = eval("var x = 5; { x = 10; } x").unwrap();
    assert_eq!(result.as_int(), 10);
}

#[test]
fn delete_existing_property() {
    let result = eval("var o = {a: 1}; delete o.a; o.a").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn delete_returns_true() {
    let result = eval("var o = {a: 1}; delete o.a").unwrap();
    assert!(result.as_bool());
    assert!(result.as_bool());
}

#[test]
fn instanceof_array() {
    let result = eval("[] instanceof Array").unwrap();
    assert!(result.as_bool());
}

#[test]
fn instance_not_array() {
    let result = eval("({}) instanceof Array").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn void_returns_undefined() {
    let result = eval("void 0").unwrap();
    assert!(result.is_undefined());
}
