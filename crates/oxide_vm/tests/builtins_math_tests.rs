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
fn math_abs_negative() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.abs(-42)").unwrap();
    assert!((result.as_double() - 42.0).abs() < 0.0001);
}

#[test]
fn math_sqrt() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.sqrt(16)").unwrap();
    assert!((result.as_double() - 4.0).abs() < 0.0001);
}

#[test]
fn math_pow() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.pow(2, 10)").unwrap();
    assert!((result.as_double() - 1024.0).abs() < 0.0001);
}

#[test]
fn math_ceil() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.ceil(3.14)").unwrap();
    assert!((result.as_double() - 4.0).abs() < 0.0001);
}

#[test]
fn math_floor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.floor(3.14)").unwrap();
    assert!((result.as_double() - 3.0).abs() < 0.0001);
}

#[test]
fn math_round() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.round(3.6)").unwrap();
    assert!((result.as_double() - 4.0).abs() < 0.0001);
}

#[test]
fn math_max() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max(1, 5)").unwrap();
    assert!((result.as_double() - 5.0).abs() < 0.0001);
}

#[test]
fn math_min() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.min(10, 5)").unwrap();
    assert!((result.as_double() - 5.0).abs() < 0.0001);
}

#[test]
fn math_sin() {
    let mut vm = Vm::new();
    eval(&mut vm, "Math.sin(0)").unwrap();
}

#[test]
fn math_cos() {
    let mut vm = Vm::new();
    eval(&mut vm, "Math.cos(0)").unwrap();
}

#[test]
fn math_random_in_range() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.random()").unwrap();
    assert!(result.as_double() >= 0.0);
    assert!(result.as_double() < 1.0);
}

#[test]
fn math_pi() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.PI").unwrap();
    assert!((result.as_double() - std::f64::consts::PI).abs() < 0.0001);
}

#[test]
fn math_sign_positive() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.sign(42)").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn math_sign_negative() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.sign(-42)").unwrap();
    assert_eq!(result.as_int(), -1);
}

#[test]
fn math_trunc() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.trunc(3.9)").unwrap();
    assert!((result.as_double() - 3.0).abs() < 0.0001);
}
