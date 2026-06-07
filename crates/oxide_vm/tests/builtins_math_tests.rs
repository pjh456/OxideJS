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

#[test]
fn math_acosh() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.acosh(1)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_asinh() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.asinh(0)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_atanh() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.atanh(0)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_clz32() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.clz32(0)").unwrap();
    assert_eq!(result.as_int(), 32);
}

#[test]
fn math_expm1() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.expm1(0)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_fround() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.fround(1.5)").unwrap();
    assert!((result.as_double() - 1.5).abs() < 0.0001);
}

#[test]
fn math_log1p() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.log1p(0)").unwrap();
    assert!((result.as_double() - 0.0).abs() < 0.0001);
}

#[test]
fn math_constant_e() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.E").unwrap();
    assert!((result.as_double() - std::f64::consts::E).abs() < 0.0001);
}

#[test]
fn math_constant_ln10() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.LN10").unwrap();
    assert!((result.as_double() - std::f64::consts::LN_10).abs() < 0.0001);
}

#[test]
fn math_constant_ln2() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.LN2").unwrap();
    assert!((result.as_double() - std::f64::consts::LN_2).abs() < 0.0001);
}

#[test]
fn math_constant_log10e() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.LOG10E").unwrap();
    assert!((result.as_double() - std::f64::consts::LOG10_E).abs() < 0.0001);
}

#[test]
fn math_constant_log2e() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.LOG2E").unwrap();
    assert!((result.as_double() - std::f64::consts::LOG2_E).abs() < 0.0001);
}

#[test]
fn math_constant_sqrt1_2() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.SQRT1_2").unwrap();
    assert!((result.as_double() - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.0001);
}

#[test]
fn math_constant_sqrt2() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.SQRT2").unwrap();
    assert!((result.as_double() - std::f64::consts::SQRT_2).abs() < 0.0001);
}
