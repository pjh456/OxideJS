use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

/// 求值一个应得布尔结果的表达式并断言其类型。
fn eval_bool(source: &str) -> bool {
    let result = eval(source).expect("eval should succeed");
    assert!(result.is_bool(), "expected boolean result, got {result:?}");
    result.as_bool()
}

// --- Number 参数：NumberToBigInt 精确值 ---

/// 1e300 的 double 精确值（f64 位分解的整数部分），非十进制 10^300。
const ONE_E300: &str = "1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160";

#[test]
fn double_1e300_exact_value() {
    assert!(eval_bool(&format!("BigInt(1e300) === {ONE_E300}n")));
    assert!(eval_bool(&format!("BigInt(-1e300) === -{ONE_E300}n")));
}

#[test]
fn double_two_pow_53_exact_value() {
    assert!(eval_bool("BigInt(9007199254740992) === 9007199254740992n"));
    assert!(eval_bool("BigInt(9007199254740991.0) === 9007199254740991n"));
}

#[test]
fn double_between_two_pow_67_exact_value() {
    assert!(eval_bool("BigInt(2.5e20) === 250000000000000000000n"));
    assert!(eval_bool("BigInt(3500000000000000.0) === 3500000000000000n"));
}

#[test]
fn double_negative_zero_is_zero_bigint() {
    assert!(eval_bool("BigInt(-0.0) === 0n"));
}

// --- 构造形态：new 无条件 TypeError ---

#[test]
fn new_bigint_no_arg_throws_type_error() {
    assert!(eval_bool("try { new BigInt(); 0 } catch (e) { e instanceof TypeError }"));
}

#[test]
fn new_bigint_with_value_throws_type_error() {
    assert!(eval_bool("try { new BigInt(1); 0 } catch (e) { e instanceof TypeError }"));
}

// --- 无参：ToBigInt(undefined) → TypeError ---

#[test]
fn no_arg_throws_type_error() {
    assert!(eval_bool("try { BigInt(); 0 } catch (e) { e instanceof TypeError }"));
}

#[test]
fn explicit_undefined_throws_type_error() {
    assert!(eval_bool("try { BigInt(undefined); 0 } catch (e) { e instanceof TypeError }"));
}

// --- 字符串参数：数字分隔符非法 ---

#[test]
fn decimal_digit_separator_syntax_error() {
    assert!(eval_bool("try { BigInt(\"1_0\"); 0 } catch (e) { e instanceof SyntaxError }"));
}

#[test]
fn hex_digit_separator_syntax_error() {
    assert!(eval_bool("try { BigInt(\"0x1_F\"); 0 } catch (e) { e instanceof SyntaxError }"));
}

#[test]
fn plain_string_forms_unaffected() {
    assert!(eval_bool("BigInt(\"10\") === 10n"));
    assert!(eval_bool("BigInt(\"0x1F\") === 31n"));
    assert!(eval_bool("BigInt(\"  0o17  \") === 15n"));
}
