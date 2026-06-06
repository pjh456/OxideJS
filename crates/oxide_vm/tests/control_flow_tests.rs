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
fn eval_if_true() {
    assert_eq!(eval("if (true) { 1 } else { 2 }"), "1");
}

#[test]
fn eval_if_false() {
    assert_eq!(eval("if (false) { 1 } else { 2 }"), "2");
}

#[test]
fn eval_if_without_else_true() {
    assert_eq!(eval("if (true) { 42 }"), "42");
}

#[test]
fn eval_if_without_else_false() {
    assert_eq!(eval("if (false) { 42 }"), "undefined");
}

#[test]
fn eval_dangling_else() {
    assert_eq!(eval("if (true) { if (false) { 1 } else { 2 } }"), "2");
}

#[test]
fn eval_while_zero_iterations() {
    assert_eq!(eval("var x = 0; while (false) { x = 1; } x"), "0");
}

#[test]
fn eval_while_multi_iterations() {
    assert_eq!(eval("var i = 0; while (i < 3) { i = i + 1; } i"), "3");
}

#[test]
fn eval_while_result_undefined() {
    assert_eq!(eval("while (false) { 1; }"), "undefined");
}

#[test]
fn eval_for_basic() {
    assert_eq!(
        eval("var r = 0; for (i = 0; i < 3; i = i + 1) { r = r + i; } r"),
        "3"
    );
}

#[test]
fn eval_for_no_test() {
    assert_eq!(
        eval("var r = 0; for (i = 0; ; i = i + 1) { if (i >= 3) { break; } r = r + i; } r"),
        "3"
    );
}

#[test]
fn eval_for_no_init_update() {
    assert_eq!(eval("var i = 0; for (; i < 3; ) { i = i + 1; } i"), "3");
}

#[test]
fn eval_ternary_true() {
    assert_eq!(eval("true ? 1 : 2"), "1");
}

#[test]
fn eval_ternary_false() {
    assert_eq!(eval("false ? 1 : 2"), "2");
}

#[test]
fn eval_ternary_nested() {
    assert_eq!(eval("true ? (false ? 1 : 2) : 3"), "2");
}

#[test]
fn eval_break_in_while() {
    assert_eq!(
        eval("var i = 0; while (true) { i = i + 1; if (i >= 3) { break; } } i"),
        "3"
    );
}

#[test]
fn eval_continue_in_while() {
    assert_eq!(
        eval("var i = 0; var r = 0; while (i < 5) { i = i + 1; if (i == 3) { continue; } r = r + 1; } r"),
        "4"
    );
}

#[test]
fn eval_break_in_nested_loop() {
    assert_eq!(
        eval("var x = 0; while (true) { while (true) { x = x + 1; if (x >= 3) { break; } } break; } x"),
        "3"
    );
}

#[test]
fn eval_break_in_for() {
    assert_eq!(
        eval("var r = 0; for (i = 0; i < 10; i = i + 1) { if (i >= 3) { break; } r = r + i; } r"),
        "3"
    );
}

#[test]
fn eval_continue_in_for() {
    assert_eq!(
        eval("var r = 0; for (i = 0; i < 5; i = i + 1) { if (i == 3) { continue; } r = r + i; } r"),
        "7"
    );
}

#[test]
fn eval_control_flow_complex() {
    assert_eq!(
        eval("var r = 0; for (i = 0; i < 5; i = i + 1) { if (i == 2) { continue; } r = r + i; } r"),
        "8"
    );
}
