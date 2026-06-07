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

#[test]
fn eval_do_while_body_executes_once() {
    assert_eq!(eval("var x = 0; do { x = 1; } while (false); x"), "1");
}

#[test]
fn eval_do_while_multi_iteration() {
    assert_eq!(eval("var i = 0; do { i = i + 1; } while (i < 3); i"), "3");
}

#[test]
fn eval_do_while_break() {
    assert_eq!(
        eval("var i = 0; do { i = i + 1; if (i >= 3) break; } while (true); i"),
        "3"
    );
}

#[test]
fn eval_do_while_continue() {
    assert_eq!(
        eval("var i = 0; var r = 0; do { i = i + 1; if (i == 3) continue; r = r + 1; } while (i < 5); r"),
        "4"
    );
}

#[test]
fn eval_for_in_enumerates_keys() {
    assert_eq!(eval("var n=0; for (k in {a:1,b:2}) { n=n+1; } n"), "3");
}

#[test]
fn eval_for_in_empty_object() {
    assert_eq!(eval("var r=0; for (k in {}) { r=1; } r"), "1");
}

#[test]
fn eval_for_in_non_object_throws() {
    let result = eval("for (k in 42) {}");
    assert!(
        result.contains("TypeError"),
        "expected TypeError, got: {result}"
    );
}

#[test]
fn eval_for_in_nested() {
    assert_eq!(
        eval("var r=0; for (k in {a:{x:1}}) { for (j in {y:2}) { r=r+1; } } r"),
        "2"
    );
}

#[test]
fn eval_for_in_break() {
    assert_eq!(
        eval("var r=0; for (k in {a:1,b:2,c:3}) { if (k=='b') break; r=r+1; } r"),
        "1"
    );
}

#[test]
fn eval_for_in_continue() {
    assert_eq!(
        eval("var r=0; for (k in {a:1,b:2}) { if (k=='a') continue; r=r+1; } r"),
        "2"
    );
}

#[test]
fn eval_for_in_let_scoping() {
    let result = eval("var r=0; for (let k in {a:1}) { r=1; } r");
    assert_eq!(result, "1");
}

#[test]
fn eval_switch_basic_match() {
    assert_eq!(
        eval("var x=0;switch(2){case 1:x=10;case 2:x=20;break;default:x=30;}x"),
        "20"
    );
}

#[test]
fn eval_switch_fallthrough() {
    assert_eq!(eval("var x=0;switch(1){case 1:case 2:x=42;}x"), "42");
}

#[test]
fn eval_switch_default() {
    assert_eq!(eval("var x=0;switch(99){case 1:x=10;default:x=30;}x"), "30");
}

#[test]
fn eval_switch_no_match_no_default() {
    assert_eq!(eval("var x=0;switch(99){case 1:x=10;}x"), "0");
}

#[test]
fn eval_switch_break() {
    assert_eq!(
        eval("var x=0;switch(1){case 1:x=10;break;case 2:x=20;}x"),
        "10"
    );
}

#[test]
fn eval_break_in_switch_in_loop() {
    assert_eq!(
        eval("var r=0;while(true){switch(1){case 1:r=1;break;}r=2;break;}r"),
        "2"
    );
}
