use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
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
fn parse_parse_int_object_arg_tostring() {
    let mut vm = Vm::new();
    // 对象参数经 ToString 完整转换：取 toString 结果再解析。
    let result = eval(&mut vm, "parseInt({toString: () => '42'})").unwrap();
    assert_eq!(result.as_int(), 42);
    #[allow(clippy::approx_constant)]
    let expected = 3.14;
    let result = eval(&mut vm, "parseFloat({toString: () => '3.14abc'})").unwrap();
    assert!((result.as_double() - expected).abs() < 0.001);
    // Symbol 参数按规范抛 TypeError。
    let err = eval(&mut vm, "parseInt(Symbol())").unwrap_err();
    assert!(err.contains("TypeError"), "got: {}", err);
    let err = eval(&mut vm, "parseFloat(Symbol('x'))").unwrap_err();
    assert!(err.contains("TypeError"), "got: {}", err);
    // 对象 toString 抛出的原始异常原样传播（保留异常对象）。
    let err = eval(&mut vm, "try { parseInt({toString(){ throw 42 }}) } catch (e) { e }").unwrap();
    assert_eq!(err.as_int(), 42);
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
    // 指数恒带符号（规范科学式形态）。
    assert_eq!(s, "1.23e+2");
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

fn str_of(vm: &mut Vm, source: &str) -> String {
    let r = eval(vm, source).unwrap();
    vm.lookup_str(r).unwrap_or_default()
}

#[test]
fn number_proto_methods_this_number_value() {
    let mut vm = Vm::new();
    // Number 原始值与 Number 对象均合法 this。
    assert_eq!(str_of(&mut vm, "(new Number(7)).toFixed(1)"), "7.0");
    assert_eq!(str_of(&mut vm, "(new Number(7)).toPrecision(2)"), "7.0");
    assert_eq!(str_of(&mut vm, "(new Number(7)).toExponential(1)"), "7.0e+0");
    assert_eq!(str_of(&mut vm, "(new Number(255)).toString(16)"), "ff");
    // Number.prototype 本体是 [[NumberData]] = +0 的 Number 对象。
    assert_eq!(str_of(&mut vm, "Number.prototype.toFixed(1)"), "0.0");
    assert_eq!(str_of(&mut vm, "Number.prototype.toExponential(0)"), "0e+0");
    // 其余 this 一律 TypeError（四方法非通用）。
    for method in ["toString", "toFixed", "toExponential", "toPrecision"] {
        let src = format!("Number.prototype.{}.call({{}})", method);
        let err = eval(&mut vm, &src).unwrap_err();
        assert!(err.contains("TypeError"), "for {}: {}", method, err);
    }
    let err = eval(&mut vm, "Number.prototype.toFixed.call(new String('1'))").unwrap_err();
    assert!(err.contains("TypeError"), "got: {}", err);
    let err = eval(&mut vm, "Number.prototype.toPrecision.call(null)").unwrap_err();
    assert!(err.contains("TypeError"), "got: {}", err);
}

#[test]
fn number_methods_argument_coercion_and_ordering() {
    let mut vm = Vm::new();
    // 参数走完整 ToNumber：Symbol/BigInt 抛 TypeError。
    for method in ["toFixed", "toExponential", "toPrecision"] {
        let err = eval(&mut vm, &format!("(0).{}(0n)", method)).unwrap_err();
        assert!(err.contains("TypeError"), "bigint for {}: {}", method, err);
        let err = eval(&mut vm, &format!("(0).{}(Symbol())", method)).unwrap_err();
        assert!(err.contains("TypeError"), "symbol for {}: {}", method, err);
    }
    // 参数 valueOf 抛出的原始异常原样传播（保留异常值）。
    let v = eval(&mut vm, "try { (0).toFixed({valueOf(){ throw 42 }}) } catch (e) { e }").unwrap();
    assert_eq!(v.as_int(), 42);
    // toFixed：范围检查先于 NaN 短路。
    let err = eval(&mut vm, "NaN.toFixed(Infinity)").unwrap_err();
    assert!(err.contains("RangeError"), "got: {}", err);
    assert_eq!(str_of(&mut vm, "NaN.toFixed(0)"), "NaN");
    // toPrecision：NaN/±∞ 专名先于范围检查。
    assert_eq!(str_of(&mut vm, "NaN.toPrecision(1000)"), "NaN");
    assert_eq!(str_of(&mut vm, "(Infinity).toPrecision(1000)"), "Infinity");
    assert_eq!(str_of(&mut vm, "(-Infinity).toPrecision(1000)"), "-Infinity");
    let err = eval(&mut vm, "(10).toPrecision(0)").unwrap_err();
    assert!(err.contains("RangeError"), "got: {}", err);
    // toExponential：NaN/±∞ 专名先于范围检查。
    assert_eq!(str_of(&mut vm, "NaN.toExponential(101)"), "NaN");
    assert_eq!(str_of(&mut vm, "(Infinity).toExponential(101)"), "Infinity");
    let err = eval(&mut vm, "(3).toExponential(101)").unwrap_err();
    assert!(err.contains("RangeError"), "got: {}", err);
    let err = eval(&mut vm, "(3).toExponential(-1)").unwrap_err();
    assert!(err.contains("RangeError"), "got: {}", err);
    // toExponential 的 undefined 走最短式，非按 0 位处理。
    assert_eq!(str_of(&mut vm, "(123.456).toExponential(undefined)"), "1.23456e+2");
}

#[test]
fn number_methods_default_precision_and_boundaries() {
    let mut vm = Vm::new();
    // toPrecision 缺省 precision = ToString(x)。
    assert_eq!(str_of(&mut vm, "(42).toPrecision()"), "42");
    assert_eq!(str_of(&mut vm, "(123.456).toPrecision(undefined)"), "123.456");
    assert_eq!(str_of(&mut vm, "NaN.toPrecision()"), "NaN");
    assert_eq!(str_of(&mut vm, "Number.prototype.toPrecision()"), "0");
    // toFixed 的 x ≥ 1e21 退化为 String 科学式。
    assert_eq!(str_of(&mut vm, "(1e21).toFixed()"), "1e+21");
    assert_eq!(str_of(&mut vm, "(-1e21).toFixed(0)"), "-1e+21");
    assert_eq!(str_of(&mut vm, "(new Number(1e21)).toFixed()"), "1e+21");
    // -0 三面：构造、toFixed、toExponential。
    assert_eq!(str_of(&mut vm, "String(Object.is(Number(-0), -0))"), "true");
    assert_eq!(str_of(&mut vm, "String(Object.is(new Number(-0).valueOf(), -0))"), "true");
    assert_eq!(str_of(&mut vm, "(-0).toFixed(0)"), "0");
    assert_eq!(str_of(&mut vm, "(-0).toExponential(0)"), "0e+0");
    assert_eq!(str_of(&mut vm, "(-0).toPrecision(2)"), "0.0");
    // 指数恒带符号。
    assert_eq!(str_of(&mut vm, "(3).toExponential(0)"), "3e+0");
    assert_eq!(str_of(&mut vm, "(0).toExponential(1)"), "0.0e+0");
    assert_eq!(str_of(&mut vm, "(10).toPrecision(1)"), "1e+1");
    assert_eq!(str_of(&mut vm, "(1.1e-32).toExponential()"), "1.1e-32");
}

#[test]
fn number_isnan_isfinite_strict_type() {
    let mut vm = Vm::new();
    // 非 Number 类型（含字符串/对象）一律 false，不强转。
    assert_eq!(str_of(&mut vm, "String(Number.isFinite('1'))"), "false");
    assert_eq!(str_of(&mut vm, "String(Number.isNaN('NaN'))"), "false");
    assert_eq!(str_of(&mut vm, "String(Number.isFinite(new Number(42)))"), "false");
    assert_eq!(str_of(&mut vm, "String(Number.isFinite(42))"), "true");
    assert_eq!(str_of(&mut vm, "String(Number.isNaN(NaN))"), "true");
    assert_eq!(str_of(&mut vm, "String(Number.isFinite(Infinity))"), "false");
}

#[test]
fn number_proto_tag_and_metadata() {
    let mut vm = Vm::new();
    // Number.prototype 本体 tag 为 Number（品牌表据 type_tag 命中）。
    assert_eq!(str_of(&mut vm, "Object.prototype.toString.call(Number.prototype)"), "[object Number]");
    // parseInt/parseFloat 与全局同名函数是同一函数对象。
    assert_eq!(str_of(&mut vm, "String(Number.parseInt === parseInt)"), "true");
    assert_eq!(str_of(&mut vm, "String(Number.parseFloat === parseFloat)"), "true");
    // toLocaleString 为 own 属性，行为同 toString（locale 参数忽略）。
    assert_eq!(str_of(&mut vm, "String(Number.prototype.hasOwnProperty('toLocaleString'))"), "true");
    assert_eq!(str_of(&mut vm, "(1.5).toLocaleString()"), "1.5");
    // 四方法 length 为 1。
    assert_eq!(str_of(&mut vm, "String(Number.prototype.toString.length)"), "1");
    assert_eq!(str_of(&mut vm, "String(Number.prototype.toFixed.length)"), "1");
    assert_eq!(str_of(&mut vm, "String(Number.prototype.toExponential.length)"), "1");
    assert_eq!(str_of(&mut vm, "String(Number.prototype.toPrecision.length)"), "1");
}

/// full_reset 脏重建：number 家族重建后 Number.prototype 仍带 Number 对象 tag
/// 与 +0 包值（漏此分支则 tag 翻回 "Object"）。
#[test]
fn number_proto_tag_survives_dirty_rebuild() {
    let mut vm = Vm::new();
    // 属性写使 number 家族置脏，触发选择性重建路径。
    eval(&mut vm, "Number.prototype.marker = 1").unwrap();
    vm.full_reset();
    assert_eq!(str_of(&mut vm, "Object.prototype.toString.call(Number.prototype)"), "[object Number]");
    assert_eq!(str_of(&mut vm, "Number.prototype.toFixed(1)"), "0.0");
    assert_eq!(str_of(&mut vm, "String(Number.parseInt === parseInt)"), "true");
}

// 全表值对照 node v20.19.2 逐位核定（half-up 对 half-even 的分歧点、
// 进位传播、位数增长、精确展开面、零与 -0 三面）。
#[test]
fn number_format_tie_half_up_and_carry() {
    let mut vm = Vm::new();
    let cases = [
        // tie 取较大者（away-from-zero），负数同形。
        ("(2.5).toExponential(0)", "3e+0"),
        ("(2.4).toExponential(0)", "2e+0"),
        ("(-2.5).toExponential(0)", "-3e+0"),
        ("(-0.5).toExponential(0)", "-5e-1"),
        ("(0.5).toFixed(0)", "1"),
        ("(0.4).toFixed(0)", "0"),
        ("(2.5).toFixed(0)", "3"),
        ("(-0.5).toFixed(0)", "-1"),
        ("(2.675).toFixed(2)", "2.67"),
        // 精确展开低于 tie 点时不得进位（仅看最短形会误进位）。
        ("(1.15).toFixed(1)", "1.1"),
        ("(1.005).toFixed(2)", "1.00"),
        ("(1.005).toFixed(1)", "1.0"),
        ("(0.05).toFixed(1)", "0.1"),
        ("(0.145).toFixed(2)", "0.14"),
        ("(1.2345).toFixed(3)", "1.234"),
        ("(1.0005).toFixed(3)", "1.000"),
        ("(0.615).toFixed(2)", "0.61"),
        // 进位传播与位数增长（9.99→10.0 型）。
        ("(9.999).toFixed(1)", "10.0"),
        ("(9.999).toFixed(0)", "10"),
        ("(-9.999).toFixed(1)", "-10.0"),
        ("(0.9999).toExponential(0)", "1e+0"),
        ("(-0.9999).toExponential(0)", "-1e+0"),
        ("(9.9999999999999999).toFixed(0)", "10"),
        ("(999).toPrecision(2)", "1.0e+3"),
        ("(-999).toPrecision(2)", "-1.0e+3"),
        ("(99.99).toPrecision(3)", "100"),
        ("(999999).toPrecision(2)", "1.0e+6"),
        ("(25).toExponential(0)", "3e+1"),
        ("(12345).toExponential(3)", "1.235e+4"),
        // 精确整数展开（位数超最短形）。
        ("(1000000000000000128).toFixed(0)", "1000000000000000128"),
        ("(9007199254740993).toFixed(0)", "9007199254740992"),
        ("(9.9e20).toFixed(0)", "990000000000000000000"),
        // 进位后 e 决定定点/指数分界。
        ("(1.23456).toPrecision(4)", "1.235"),
        ("(123456).toPrecision(3)", "1.23e+5"),
        ("(1e-6).toPrecision(2)", "0.0000010"),
        ("(1e-7).toPrecision(2)", "1.0e-7"),
        ("(0.5).toPrecision(1)", "0.5"),
        ("(0.05).toPrecision(2)", "0.050"),
        ("(10).toPrecision(1)", "1e+1"),
        // 零与 -0 三面（-0 落正分支）。
        ("(0).toFixed(3)", "0.000"),
        ("(0).toExponential(2)", "0.00e+0"),
        ("(0).toPrecision(2)", "0.0"),
        ("(-0).toFixed(3)", "0.000"),
        ("(-0).toExponential(2)", "0.00e+0"),
        ("(-0).toPrecision(2)", "0.0"),
        // x ≥ 1e21 走科学式 String（f64 已达 1e21）。
        ("(1e21-1).toFixed(0)", "1e+21"),
        ("(-1e21).toFixed(0)", "-1e+21"),
        ("(1234567890123456789012345678901234567890).toFixed(0)", "1.2345678901234568e+39"),
        // 长小数面（展开终止后按 0 补齐）。
        ("(3.141592653589793).toPrecision(17)", "3.1415926535897931"),
        (
            "(3.141592653589793).toFixed(100)",
            "3.1415926535897931159979634685441851615905761718750000000000000000000000000000000000000000000000000000",
        ),
        // 次正规数与超大指数面。
        ("(1e308).toPrecision(2)", "1.0e+308"),
        ("(1.5e308).toExponential(1)", "1.5e+308"),
        ("(1e-323).toExponential(2)", "9.88e-324"),
        ("(5e-324).toExponential(0)", "5e-324"),
        ("(4.9e-324).toFixed(1)", "0.0"),
    ];
    for (src, expected) in cases {
        assert_eq!(str_of(&mut vm, src), expected, "for {}", src);
    }
}

#[test]
fn number_format_default_shortest_digits() {
    let mut vm = Vm::new();
    // 缺省位数 = 最短精确有效数字数 − 1（round-trip 前缀，非任意最短十进制形）。
    let cases = [
        ("(123.456).toExponential()", "1.23456e+2"),
        ("(2).toExponential()", "2e+0"),
        ("(0.1).toExponential()", "1e-1"),
        ("(0.3).toExponential()", "3e-1"),
        ("(1e-7).toExponential()", "1e-7"),
        ("(1.1e-32).toExponential()", "1.1e-32"),
        ("(1e20).toExponential()", "1e+20"),
        ("(1e-20).toExponential()", "1e-20"),
        ("(0.1+0.2).toExponential()", "3.0000000000000004e-1"),
        ("(123456789012345678901234567890.5).toExponential()", "1.2345678901234568e+29"),
        ("(42).toPrecision()", "42"),
        ("(0.0000001).toPrecision(3)", "1.00e-7"),
        ("(0.00000012345).toPrecision(2)", "1.2e-7"),
    ];
    for (src, expected) in cases {
        assert_eq!(str_of(&mut vm, src), expected, "for {}", src);
    }
}

// 二进指数 q 为 limb 位宽整数倍（q ≡ 0 mod 32，偏置指数 e2 ≡ 19 mod 32）
// 的分数面：余数掩码边界恰落整 limb，digit 提取失真即此面（对照 node
// v20.19.2 逐位核定）。
#[test]
fn number_format_limb_aligned_fraction_digits() {
    let mut vm = Vm::new();
    let cases = [
        // q = 64（[2^-12, 2^-11) 量级）。
        ("(2 ** -12).toExponential(5)", "2.44141e-4"),
        ("(2 ** -12).toExponential(15)", "2.441406250000000e-4"),
        // q = 32（[2^20, 2^21) 量级）。
        ("(1048576.0000000002).toExponential(15)", "1.048576000000000e+6"),
        ("(1048576.0000000002).toFixed(10)", "1048576.0000000002"),
        // 非受影响类的整数面守卫（小数值与 2^32 + 1）。
        ("(4.294967297).toFixed(2)", "4.29"),
        ("(4294967297).toFixed(2)", "4294967297.00"),
    ];
    for (src, expected) in cases {
        assert_eq!(str_of(&mut vm, src), expected, "for {}", src);
    }
}
