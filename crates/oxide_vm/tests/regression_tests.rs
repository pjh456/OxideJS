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
fn regression_rerun_clears_ic_cache() {
    let allocator = Allocator::default();
    let source = "var o = {a: 1}; o.a";
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    assert_eq!(format!("{}", vm.run(&module).unwrap()), "1");

    let source2 = "var o = {a: 2}; o.a";
    let program2 = oxide_parser::parse(&allocator, source2).expect("parse");
    let module2 = Compiler::new().compile(&program2).expect("compile");
    vm.run(&module2).unwrap();
    assert_eq!(
        format!("{}", vm.rerun().unwrap()),
        "2",
        "rerun should re-read IC-cached property, not stale value from previous run"
    );
}

#[test]
fn regression_recursion_depth_limit() {
    assert_eq!(
        eval("function f(){f()} f()"),
        "vm error: RangeError: Maximum call stack size exceeded"
    );
}
