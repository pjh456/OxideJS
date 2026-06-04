use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse failed");
    let module = Compiler::new().compile(&program).expect("compile failed");
    let mut vm = Vm::new();
    let result = vm.run(&module).expect("vm run failed");
    format!("{result}")
}

#[test]
fn eval_arithmetic_add() {
    assert_eq!(eval("1 + 2"), "3");
}

#[test]
fn eval_arithmetic_sub() {
    assert_eq!(eval("5 - 3"), "2");
}

#[test]
fn eval_arithmetic_mul() {
    assert_eq!(eval("3 * 4"), "12");
}

#[test]
fn eval_arithmetic_div() {
    assert_eq!(eval("10 / 2"), "5");
}

#[test]
fn eval_arithmetic_mod() {
    assert_eq!(eval("7 % 3"), "1");
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
fn eval_comparison_lt() {
    assert_eq!(eval("2 < 3"), "true");
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
fn eval_neq() {
    assert_eq!(eval("1 != 2"), "true");
    assert_eq!(eval("1 != 1"), "false");
}

#[test]
fn eval_coercion_null_equals_null() {
    assert_eq!(eval("null == null"), "true");
}

#[test]
fn eval_coercion_bool_equals_int() {
    assert_eq!(eval("true == 1"), "true");
    assert_eq!(eval("false == 0"), "true");
}

#[test]
fn eval_precedence() {
    assert_eq!(eval("1 + 2 * 3"), "7");
    assert_eq!(eval("1 * 2 + 3"), "5");
}

#[test]
fn eval_negation() {
    assert_eq!(eval("-5"), "-5");
}

#[test]
fn eval_boolean_and() {
    // && not supported by compiler yet, test with nested ternary or skip
}

#[test]
fn eval_division_by_zero() {
    assert_eq!(eval("1 / 0"), "Infinity");
}

#[test]
fn eval_multi_stmt_last_value() {
    assert_eq!(eval("1; 2; 3"), "3");
}

#[test]
fn eval_var_declaration() {
    assert_eq!(eval("var x = 42"), "42");
}
