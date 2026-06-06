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
fn eval_var_declaration() {
    assert_eq!(eval("var x = 42"), "42");
}

#[test]
fn eval_multi_stmt_last_value() {
    assert_eq!(eval("1; 2; 3"), "3");
}

#[test]
fn eval_reassignment() {
    assert_eq!(eval("var x = 1; x = 2; x"), "2");
}

#[test]
fn eval_multi_var_declaration() {
    assert_eq!(eval("var x = 1; var y = 2; x + y"), "3");
}
