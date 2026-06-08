use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
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

#[test]
fn eval_infinity_returns_infinity() {
    let result = eval("Infinity").unwrap();
    assert!(result.is_double());
    assert!(result.as_double().is_infinite());
    assert!(result.as_double().is_sign_positive());
}

#[test]
fn eval_neg_infinity_via_division() {
    let result = eval("1 / Infinity").unwrap();
    assert!(result.is_double());
    assert_eq!(result.as_double(), 0.0);
}

#[test]
fn typeof_infinity_is_number() {
    let result = eval("typeof Infinity").unwrap();
    assert!(result.is_string());
}

#[test]
fn infinity_ne_nan() {
    let result = eval("Infinity !== NaN").unwrap();
    assert!(result.is_bool());
    assert!(result.as_bool());
}

#[test]
fn infinity_gt_large_number() {
    let result = eval("Infinity > 1e308").unwrap();
    assert!(result.is_bool());
    assert!(result.as_bool());
}

#[test]
fn division_by_zero_gives_infinity() {
    let result = eval("1 / 0").unwrap();
    assert!(result.is_double());
    assert!(result.as_double().is_infinite());
}
