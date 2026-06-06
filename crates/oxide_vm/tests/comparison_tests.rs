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
