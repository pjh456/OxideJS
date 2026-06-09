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
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
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
fn eval_precedence() {
    assert_eq!(eval("1 * 2 + 3"), "5");
}

#[test]
fn eval_negation() {
    assert_eq!(eval("-5"), "-5");
}

#[test]
fn eval_division_by_zero() {
    assert_eq!(eval("1 / 0"), "Infinity");
}

#[test]
fn eval_string_concat() {
    assert_eq!(eval("1 + 2 + 3"), "6");
}

#[test]
fn eval_compound_add() {
    assert_eq!(eval("var x=5; x+=3; x"), "8");
}

#[test]
fn eval_compound_sub() {
    assert_eq!(eval("var x=10; x-=4; x"), "6");
}

#[test]
fn eval_compound_mul() {
    assert_eq!(eval("var x=2; x*=3; x"), "6");
}

#[test]
fn eval_compound_div() {
    assert_eq!(eval("var x=10; x/=2; x"), "5");
}

#[test]
fn eval_compound_mod() {
    assert_eq!(eval("var x=7; x%=3; x"), "1");
}

#[test]
fn eval_compound_exp() {
    assert_eq!(eval("var x=2; x**=3; x"), "8");
}

#[test]
fn eval_compound_add_expr_value() {
    assert_eq!(eval("var x=5; x+=3"), "8");
}

#[test]
fn eval_compound_undefined() {
    assert_eq!(eval("var x; x+=1; x"), "NaN");
}

#[test]
fn eval_bitwise_direct_ops() {
    assert_eq!(eval("5 & 3"), "1");
    assert_eq!(eval("5 | 2"), "7");
    assert_eq!(eval("5 ^ 1"), "4");
    assert_eq!(eval("1 << 3"), "8");
    assert_eq!(eval("-8 >> 1"), "-4");
    assert_eq!(eval("-1 >>> 0"), "4294967295");
    assert_eq!(eval("~0"), "-1");
}

#[test]
fn eval_bitwise_coercion() {
    assert_eq!(eval("'3' | 0"), "3");
    assert_eq!(eval("'x' | 0"), "0");
    assert_eq!(eval("undefined | 0"), "0");
    assert_eq!(eval("NaN | 0"), "0");
    assert_eq!(eval("1.9 | 0"), "1");
    assert_eq!(eval("-1.9 | 0"), "-1");
}

#[test]
fn eval_shift_masking() {
    assert_eq!(eval("1 << 32"), "1");
    assert_eq!(eval("1 << 33"), "2");
    assert_eq!(eval("8 >> 33"), "4");
}

#[test]
fn eval_ushr_boundaries() {
    assert_eq!(eval("2147483647 >>> 0"), "2147483647");
    assert_eq!(eval("2147483648 >>> 0"), "2147483648");
    assert_eq!(eval("-1 >>> 1"), "2147483647");
}

#[test]
fn eval_compound_bitwise_ops() {
    assert_eq!(eval("var x=5; x &= 3; x"), "1");
    assert_eq!(eval("var x=5; x |= 2; x"), "7");
    assert_eq!(eval("var x=5; x ^= 1; x"), "4");
    assert_eq!(eval("var x=1; x <<= 5; x"), "32");
    assert_eq!(eval("var x=-8; x >>= 1; x"), "-4");
    assert_eq!(eval("var x=-1; x >>>= 0; x"), "4294967295");
}

#[test]
fn eval_bitwise_in_control_flow() {
    assert_eq!(eval("var x=3; if ((x & 1) == 1) { 7 } else { 9 }"), "7");
}
