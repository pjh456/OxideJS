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
fn regression_to_boolean_empty_string() {
    assert_eq!(eval("!''"), "true", "NOT empty string should be true");
    assert_eq!(
        eval("if ('') { 1 } else { 2 }"),
        "2",
        "empty string should be falsy"
    );
}

#[test]
fn regression_continue_in_for() {
    assert_eq!(
        eval("var r=0; for(i=0;i<3;i=i+1){if(i==1)continue;r=r+i;}r"),
        "2",
        "continue should skip iteration body"
    );
}
