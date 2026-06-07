use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program =
        oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new()
        .compile(&program)
        .map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

#[test]
fn number_is_nan_with_nan() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number.isNaN(NaN)").unwrap();
    assert!(result.as_bool());
}

#[test]
fn number_is_nan_with_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number.isNaN(42)").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn number_is_finite_with_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number.isFinite(42)").unwrap();
    assert!(result.as_bool());
}

#[test]
fn number_is_finite_with_infinity() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number.isFinite(1/0)").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn parse_int_decimal() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "parseInt('42')").unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn parse_int_hex() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "parseInt('0xFF')").unwrap();
    assert_eq!(result.as_int(), 255);
}

#[test]
fn parse_float_decimal() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "parseFloat('3.14')").unwrap();
    assert!((result.as_double() - 3.14).abs() < 0.001);
}

#[test]
fn parse_float_invalid() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "parseFloat('abc')").unwrap();
    assert!(result.as_double().is_nan());
}

#[test]
fn number_constructor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number('42')").unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn to_string_of_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var n = 42; n.toString()").unwrap();
    let s = vm
        .kernel()
        .string_forge()
        .lookup(result.as_string_index())
        .unwrap_or_default();
    assert_eq!(s, "42");
}

#[test]
fn to_fixed_of_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var n = 3.14159; n.toFixed(2)").unwrap();
    let s = vm
        .kernel()
        .string_forge()
        .lookup(result.as_string_index())
        .unwrap_or_default();
    assert_eq!(s, "3.14");
}
