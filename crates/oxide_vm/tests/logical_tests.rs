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
fn eval_not_true() {
    assert_eq!(eval("!true"), "false");
}

#[test]
fn eval_not_false() {
    assert_eq!(eval("!false"), "true");
}

#[test]
fn eval_not_zero() {
    assert_eq!(eval("!0"), "true");
}

#[test]
fn eval_not_string() {
    assert_eq!(eval("!'hello'"), "false");
}

#[test]
fn eval_and_short_circuit() {
    assert_eq!(eval("var x = 0; false && (x = 5); x"), "0");
}

#[test]
fn eval_and_truthy() {
    assert_eq!(eval("1 && 2"), "2");
}

#[test]
fn eval_and_falsy_first() {
    assert_eq!(eval("0 && 2"), "0");
}

#[test]
fn eval_or_short_circuit() {
    assert_eq!(eval("var x = 0; true || (x = 5); x"), "0");
}

#[test]
fn eval_or_truthy_first() {
    assert_eq!(eval("1 || 2"), "1");
}

#[test]
fn eval_or_falsy_both() {
    assert_eq!(eval("0 || false"), "false");
}

#[test]
fn eval_and_chain() {
    assert_eq!(eval("1 && 2 && 3"), "3");
}

#[test]
fn eval_or_chain() {
    assert_eq!(eval("0 || false || 42"), "42");
}
