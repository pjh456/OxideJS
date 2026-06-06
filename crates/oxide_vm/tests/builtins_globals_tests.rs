use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = Allocator::default();
    let program =
        oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new()
        .compile(&program)
        .map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

#[test]
fn eval_nan_returns_nan() {
    let result = eval("NaN").unwrap();
    assert!(result.is_double());
    assert!(result.as_double().is_nan());
}

#[test]
fn eval_undefined_returns_undefined() {
    let result = eval("undefined").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn typeof_nan_is_number() {
    let result = eval("typeof NaN").unwrap();
    assert!(result.is_string());
}

#[test]
fn typeof_undefined_is_undefined() {
    let result = eval("typeof undefined").unwrap();
    assert!(result.is_string());
}
