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

#[test]
fn eval_nullish_coalescing() {
    assert_eq!(eval("null ?? 5"), "5");
    assert_eq!(eval("undefined ?? 'x'"), "{string}");
    assert_eq!(eval("0 ?? 5"), "0");
    assert_eq!(eval("false ?? 5"), "false");
}

#[test]
fn eval_nullish_coalescing_short_circuits_rhs() {
    assert_eq!(eval("var x = 0; 1 ?? (x = 5); x"), "0");
}

#[test]
fn eval_optional_member_chain() {
    assert_eq!(eval("var a={b:{c:7}}; a?.b?.c"), "7");
    assert_eq!(eval("var a=null; a?.b"), "undefined");
    assert_eq!(eval("var a={b:1}; a?.b"), "1");
    assert_eq!(eval("var a=null; a?.b.c.d"), "undefined");
}

#[test]
fn eval_optional_computed_chain() {
    assert_eq!(eval("var o={k:'b'}, a={b:9}; a?.[o.k]"), "9");
    assert_eq!(eval("var o={k:'b'}, a=null; a?.[o.k]"), "undefined");
}

#[test]
fn eval_optional_parenthesized_reset() {
    assert_eq!(eval("var a=null; (a?.b)"), "undefined");
    assert!(eval("var a=null; (a?.b).c").contains("TypeError"));
}

#[test]
fn eval_optional_call() {
    assert_eq!(eval("var a={b:function(){return this.n},n:5}; a?.b()"), "5");
    assert_eq!(eval("var a=null; a?.b()"), "undefined");
    assert!(eval("var a={b:5}; a?.b()").contains("TypeError"));
}

#[test]
fn eval_delete_optional_chain() {
    assert_eq!(eval("var a=null; delete a?.b"), "true");
    assert_eq!(eval("var a={b:1}; delete a?.b; a.b"), "undefined");
}

#[test]
fn eval_logical_assignment_vars() {
    assert_eq!(eval("var a=0; a ||= 5; a"), "5");
    assert_eq!(eval("var a=1; a ||= 5; a"), "1");
    assert_eq!(eval("var a=1; a &&= 9; a"), "9");
    assert_eq!(eval("var a=0; a &&= 9; a"), "0");
    assert_eq!(eval("var a=null; a ??= 7; a"), "7");
    assert_eq!(eval("var a=0; a ??= 7; a"), "0");
}

#[test]
fn eval_logical_assignment_rhs_short_circuits() {
    assert_eq!(eval("var a=1; var x=0; a ||= (x=5); x"), "0");
    assert_eq!(eval("var a=0; var x=0; a &&= (x=5); x"), "0");
    assert_eq!(eval("var a=0; var x=0; a ??= (x=5); x"), "0");
}

#[test]
fn eval_logical_assignment_members() {
    assert_eq!(eval("var o={x:0}; o.x ||= 5; o.x"), "5");
    assert_eq!(eval("var o={x:1}; o.x ||= 5; o.x"), "1");
    assert_eq!(eval("var o={x:1}; o.x &&= 9; o.x"), "9");
    assert_eq!(eval("var o={x:0}; o.x &&= 9; o.x"), "0");
    assert_eq!(eval("var o={x:null}; o.x ??= 7; o.x"), "7");
    assert_eq!(eval("var o={x:0}; o.x ??= 7; o.x"), "0");
}
