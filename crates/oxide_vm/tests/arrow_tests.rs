use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {}", e),
    };
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(result) => format!("{}", result),
        Err(e) => format!("vm error: {}", e),
    }
}

fn eval_val(source: &str) -> (Vm, Result<JsValue, String>) {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return (Vm::new(), Err(format!("parse error: {}", e[0].message))),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return (Vm::new(), Err(format!("compile error: {}", e))),
    };
    let mut vm = Vm::new();
    let result = vm.run(&module);
    (vm, result)
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    if val.is_string() {
        vm.kernel()
            .string_forge()
            .lookup(val.as_string_index())
            .unwrap_or_default()
    } else {
        format!("{}", val)
    }
}

// -- Arrow function integration tests --

#[test]
fn arrow_expression_body_returns_value() {
    assert_eq!(eval("(() => 42)()"), "42");
}

#[test]
fn arrow_block_body_returns_value() {
    assert_eq!(eval("((a,b) => { return a+b; })(3,4)"), "7");
}

#[test]
fn arrow_single_param_no_parens() {
    assert_eq!(eval("(x => x * 2)(5)"), "10");
}

#[test]
fn arrow_multiple_params_with_parens() {
    assert_eq!(eval("((x, y) => x + y)(3, 7)"), "10");
}

#[test]
fn arrow_stored_in_variable_callable() {
    assert_eq!(eval("var f = () => 99; f()"), "99");
}

#[test]
fn arrow_stored_in_variable_with_params() {
    assert_eq!(eval("var add = (a,b) => a+b; add(10,20)"), "30");
}

#[test]
fn arrow_not_constructable_throws() {
    let result = eval("new (() => {})");
    assert!(
        result.contains("TypeError") || result.contains("arrow"),
        "Expected arrow constructor TypeError, got: {}",
        result
    );
}

#[test]
fn arrow_no_params_string_result() {
    let (vm, result) = eval_val("(() => 'hello')()");
    let s = to_str(&vm, result.unwrap());
    assert_eq!(s, "hello");
}

#[test]
fn arrow_expression_body_implicit_return() {
    assert_eq!(eval("(() => 1 + 2)()"), "3");
}

#[test]
fn arrow_block_body_needs_explicit_return() {
    assert_eq!(eval("((a,b) => { a+b; })(3,4)"), "undefined");
}

#[test]
fn arrow_at_global_scope_this_is_undefined() {
    assert_eq!(eval("(() => this)()"), "undefined");
}

#[test]
fn multiple_arrows_in_same_scope() {
    assert_eq!(eval("var a = () => 1, b = () => 2; a() + b()"), "3");
}

// NOTE: Arrow functions capturing lexical `this` from enclosing functions
// require nested function call support (sub_module flattening), which is
// a pre-existing limitation. These tests will be enabled in a future phase:
//
// fn arrow_captures_enclosing_this() {
//     // const o = {x:10, f:function(){ return (() => this.x)(); }}; o.f()
//     // Expected: 10
// }
