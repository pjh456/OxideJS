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
    #[allow(clippy::approx_constant)]
    let expected = 3.14;
    assert!((result.as_double() - expected).abs() < 0.001);
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
    let s = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(s, "42");
}

#[test]
fn to_fixed_of_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var n = 3.14159; n.toFixed(2)").unwrap();
    let s = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(s, "3.14");
}

#[test]
fn number_is_integer_true() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number.isInteger(42)").unwrap();
    assert!(result.as_bool());
}

#[test]
fn number_is_integer_false() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number.isInteger(42.5)").unwrap();
    assert!(!result.as_bool());
}

#[test]
fn number_epsilon_is_positive() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number.EPSILON > 0").unwrap();
    assert!(result.as_bool());
}

#[test]
fn number_to_exponential() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var n = 123.456; n.toExponential(2)").unwrap();
    let s = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(s, "1.23e2");
}

#[test]
fn number_static_constants_remain_bound() {
    let mut vm = Vm::new();
    assert_eq!(eval(&mut vm, "Number.MAX_SAFE_INTEGER").unwrap().as_double(), 9007199254740991f64);
    assert!(eval(&mut vm, "Number.POSITIVE_INFINITY > 0").unwrap().as_bool());
}

#[test]
fn boxed_number_valueof_preserves_int_and_float() {
    let mut vm = Vm::new();
    let i = eval(&mut vm, "new Number(5).valueOf()").unwrap();
    assert!(i.is_int());
    assert_eq!(i.as_int(), 5);

    let f = eval(&mut vm, "new Number(1.5).valueOf()").unwrap();
    assert!(f.is_double());
    assert_eq!(f.as_double(), 1.5);

    let s = eval(&mut vm, "new Number('7').valueOf()").unwrap();
    assert_eq!(s.as_int(), 7);
}

#[test]
fn boxed_number_is_object_and_call_stays_primitive() {
    let mut vm = Vm::new();
    let ty = eval(&mut vm, "typeof new Number(5)").unwrap();
    assert_eq!(vm.lookup_str(ty).unwrap(), "object");

    let n = eval(&mut vm, "Number('7')").unwrap();
    assert_eq!(n.as_int(), 7);
}
