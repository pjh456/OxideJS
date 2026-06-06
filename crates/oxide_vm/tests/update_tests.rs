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
fn eval_inc_pre() {
    assert_eq!(eval("var x=0; ++x; x"), "1");
}

#[test]
fn eval_inc_pre_expr() {
    assert_eq!(eval("var x=0; ++x"), "1");
}

#[test]
fn eval_inc_post_var() {
    assert_eq!(eval("var x=0; x++; x"), "1");
}

#[test]
fn eval_inc_post_expr() {
    assert_eq!(eval("var x=0; x++"), "0");
}

#[test]
fn eval_dec_pre() {
    assert_eq!(eval("var x=5; --x; x"), "4");
}

#[test]
fn eval_dec_pre_expr() {
    assert_eq!(eval("var x=5; --x"), "4");
}

#[test]
fn eval_dec_post_var() {
    assert_eq!(eval("var x=5; x--; x"), "4");
}

#[test]
fn eval_dec_post_expr() {
    assert_eq!(eval("var x=5; x--"), "5");
}

#[test]
fn eval_inc_string_coerce() {
    assert_eq!(eval("var x='5'; ++x"), "6");
}

#[test]
fn eval_inc_nan() {
    assert_eq!(eval("var x='hello'; ++x"), "NaN");
}

#[test]
fn eval_multi_update() {
    assert_eq!(eval("var x=1; x++; x; ++x"), "3");
}

#[test]
fn eval_inc_post_in_assign() {
    assert_eq!(eval("var x=0; var y=x++; y"), "0");
}
