use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&Arc::new(module)) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

#[test]
fn eval_comparison_eq_true() {
    assert_eq!(eval("1 == 1"), "true");
}

#[test]
fn eval_comparison_eq_false() {
    assert_eq!(eval("1 == 2"), "false");
}

#[test]
fn eval_neq() {
    assert_eq!(eval("1 != 2"), "true");
}

#[test]
fn eval_comparison_lt() {
    assert_eq!(eval("3 < 4"), "true");
}

#[test]
fn eval_comparison_gt() {
    assert_eq!(eval("4 > 5"), "false");
}

#[test]
fn eval_comparison_lte() {
    assert_eq!(eval("3 <= 3"), "true");
}

#[test]
fn eval_comparison_gte() {
    assert_eq!(eval("5 >= 4"), "true");
}

#[test]
fn eval_string_number_eq() {
    assert_eq!(eval("5 == 5"), "true");
}

#[test]
fn eval_strict_eq_true() {
    assert_eq!(eval("1 === 1"), "true");
}

#[test]
fn eval_strict_eq_false_type() {
    assert_eq!(eval("1 === '1'"), "false");
}

#[test]
fn eval_strict_eq_null_undefined() {
    let source = r#" 
    var undef;
    undef === null
    "#;
    assert_eq!(eval(source), "false");
}

#[test]
fn eval_strict_eq_bool_number() {
    assert_eq!(eval("true === 1"), "false");
}

#[test]
fn eval_strict_eq_signed_zero() {
    assert_eq!(eval("+0 === -0"), "true");
}

#[test]
fn eval_strict_neq_true() {
    assert_eq!(eval("1 !== '1'"), "true");
}

#[test]
fn eval_strict_neq_false() {
    assert_eq!(eval("1 !== 1"), "false");
}

#[test]
fn eval_unary_plus_number() {
    assert_eq!(eval("+'42'"), "42");
}

#[test]
fn eval_unary_plus_bool() {
    assert_eq!(eval("+true"), "1");
}

#[test]
fn eval_unary_plus_null() {
    assert_eq!(eval("+null"), "0");
}

#[test]
fn eval_unary_plus_nan() {
    assert_eq!(eval("+'hello'"), "NaN");
}

#[test]
fn eval_unary_plus_false() {
    assert_eq!(eval("+false"), "0");
}

#[test]
fn eval_string_relational_utf16_code_unit_order() {
    // 字符串关系比较按 UTF-16 码元序：补充平面字符（首码元 D800..DBFF）小于
    // U+E000..U+FFFF 的 BMP 字符。覆盖双 Flat、单侧 rope、ASCII 与等值边界。
    let cases = [
        // 超平面 vs BMP 高区，四运算符双向。
        (r#""\u{10000}" < "\uFFFF""#, "true"),
        (r#""\u{10000}" > "\uFFFF""#, "false"),
        (r#""\u{10000}" <= "\uFFFF""#, "true"),
        (r#""\u{10000}" >= "\uFFFF""#, "false"),
        (r#""\uFFFF" < "\u{10000}""#, "false"),
        (r#""\u{10000}" < "\uE000""#, "true"),
        (r#""\uE000" < "\u{10000}""#, "false"),
        // 等值边界：代理对写法与转义写法内容相同。
        (r#""\u{10000}" < "\u{10000}""#, "false"),
        (r#""\u{10000}" <= "\u{10000}""#, "true"),
        (r#""\u{10000}" >= "\uD800\uDC00""#, "true"),
        // 单侧非 Flat：左侧 rope 首码元为 D800，兜底单元通道结果与双 Flat 一致。
        (r#""\u{10000}" + "a".repeat(200) < "\uFFFF""#, "true"),
        (r#""\uFFFF" < "\u{10000}" + "a".repeat(200)"#, "false"),
        // ASCII 与公共前缀：字节序与码元序一致。
        (r#""a" < "b""#, "true"),
        (r#""abc" < "abd""#, "true"),
        (r#""abc" < "abc""#, "false"),
        (r#""abc" <= "abc""#, "true"),
        (r#""b" > "a""#, "true"),
        ("\"\" < \"a\"", "true"),
    ];
    for (source, expected) in cases {
        assert_eq!(eval(source), expected, "source: {source}");
    }
}
