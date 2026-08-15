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
fn parse_int_prefix_and_radix_semantics() {
    let mut vm = Vm::new();
    let cases = [
        // 前缀解析：遇非法字符停止，取最长合法数字前缀。
        ("parseInt('123abc') === 123", true),
        // +/- 符号前缀。
        ("parseInt('+42') === 42", true),
        ("parseInt('-42') === -42", true),
        // radix 显式时不触发 0x；缺省/0/16 时触发。
        ("parseInt('0x10', 10) === 0", true),
        ("parseInt('0x10') === 16", true),
        ("parseInt('0x10', 0) === 16", true),
        ("parseInt('0x10', 16) === 16", true),
        ("parseInt('+0x10') === 16", true),
        ("parseInt('0XFF') === 255", true),
        // 超 i32 用 f64 近似（允许舍入），i32 域内保持精确。
        ("parseInt('99999999999999999999') === 1e20", true),
        ("parseInt('9007199254740993') === 9007199254740992", true),
        ("parseInt('2147483647') === 2147483647", true),
        ("parseInt('2147483648') === 2147483648", true),
        // radix 有效字符过滤与空白处理。
        ("parseInt('10', 2) === 2", true),
        ("parseInt('1a', 2) === 1", true),
        ("parseInt('  -42  ') === -42", true),
        // 负零保留符号。
        ("1 / parseInt('-0') === -Infinity", true),
        // 无有效数字 / radix 越界 / 超大 radix 经 ToInt32 回绕。
        ("isNaN(parseInt(''))", true),
        ("isNaN(parseInt('0x'))", true),
        ("isNaN(parseInt('0xG'))", true),
        ("isNaN(parseInt('z'))", true),
        ("isNaN(parseInt('10', 1))", true),
        ("isNaN(parseInt('10', 37))", true),
        ("isNaN(parseInt('z', 2 ** 40))", true),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
}

#[test]
fn parse_float_prefix_semantics() {
    let mut vm = Vm::new();
    let cases = [
        // 前缀解析：取最长合法 StrDecimalLiteral。
        ("parseFloat('3.14abc') === 3.14", true),
        ("parseFloat('1.2.3') === 1.2", true),
        ("parseFloat('1e5') === 100000", true),
        ("parseFloat('1e+5') === 100000", true),
        ("parseFloat('.5') === 0.5", true),
        ("parseFloat('  3.14  ') === 3.14", true),
        // Infinity 大小写敏感，可带符号与后缀。
        ("parseFloat('Infinity') === Infinity", true),
        ("parseFloat('+Infinity') === Infinity", true),
        ("parseFloat('-Infinity') === -Infinity", true),
        ("parseFloat('Infinityx') === Infinity", true),
        ("isNaN(parseFloat('-infinity'))", true),
        // 十六进制不解析，十进制前缀在 'x' 处停止；溢出归 Infinity。
        ("parseFloat('0x10') === 0", true),
        ("parseFloat('1e400') === Infinity", true),
        ("parseFloat('-1e400') === -Infinity", true),
        // 无合法前缀。
        ("isNaN(parseFloat(''))", true),
        ("isNaN(parseFloat('.'))", true),
        ("isNaN(parseFloat('.e5'))", true),
        ("isNaN(parseFloat('abc'))", true),
        // 指数后无数字：'e' 不构成 ExponentPart，最长合法前缀为整数部分。
        ("parseFloat('1e') === 1", true),
        ("parseFloat('1e+') === 1", true),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        assert_eq!(result.as_bool(), expected, "for {}", src);
    }
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
fn to_string_scientific_boundaries() {
    let mut vm = Vm::new();
    let cases = [
        ("(1e21).toString()", "1e+21"),
        ("(1e20).toString()", "100000000000000000000"),
        ("(123.45).toString()", "123.45"),
        ("(0.000001).toString()", "0.000001"),
        ("(0.0000001).toString()", "1e-7"),
        ("(1e15).toString()", "1000000000000000"),
        ("(0.1+0.2).toString()", "0.30000000000000004"),
        ("String(1e21)", "1e+21"),
        ("(\"\" + 1e21)", "1e+21"),
        ("(-1e21).toString()", "-1e+21"),
        ("JSON.stringify(1e21)", "1e+21"),
        ("JSON.stringify(1e20)", "100000000000000000000"),
        // 整数 double 快路径（2^53 内直写）。
        ("(42.0).toString()", "42"),
        ("(1e10).toString()", "10000000000"),
        ("String(-42.0)", "-42"),
        ("(-0).toString()", "0"),
        ("JSON.stringify(1.5)", "1.5"),
        ("JSON.stringify(-0)", "0"),
        // 2^53 边界与 1e22：超出快路径范围仍输出规范形态。
        ("Math.pow(2,53).toString()", "9007199254740992"),
        ("Math.pow(2,63).toString()", "9223372036854776000"),
        ("(1e22).toString()", "1e+22"),
        ("(1.5e20).toString()", "150000000000000000000"),
        ("(9999999999999999).toString()", "10000000000000000"),
    ];
    for (src, expected) in cases {
        let result = eval(&mut vm, src).unwrap();
        let s = vm.lookup_str(result).unwrap_or_default();
        assert_eq!(s, expected, "for {}", src);
    }
}

#[test]
fn to_string_radix_and_range_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "(255).toString(16)").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "ff");

    let result = eval(&mut vm, "(2).toString(2)").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "10");

    let err = eval(&mut vm, "(10).toString(1)").unwrap_err();
    assert!(err.contains("RangeError"), "got: {}", err);

    let err = eval(&mut vm, "(10).toString(37)").unwrap_err();
    assert!(err.contains("RangeError"), "got: {}", err);
}

#[test]
fn number_constructor_string_rules() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Number('0xa')").unwrap();
    assert_eq!(result.as_int(), 10);

    let result = eval(&mut vm, "Number('')").unwrap();
    assert_eq!(result.as_int(), 0);

    let result = eval(&mut vm, "Number('INFINITY')").unwrap();
    assert!(result.as_double().is_nan());
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
