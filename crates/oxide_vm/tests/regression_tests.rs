use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
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

fn eval_with_kernel(source: &str, kernel: Arc<KernelCore>) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::with_kernel_core(kernel);
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

#[test]
fn regression_to_boolean_empty_string() {
    assert_eq!(eval("!''"), "true", "NOT empty string should be true");
    assert_eq!(eval("if ('') { 1 } else { 2 }"), "2", "empty string should be falsy");
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
    assert_eq!(eval("function f(){f()} f()"), "vm error: RangeError: Maximum call stack size exceeded");
}

#[test]
fn regression_throw_statement_errors() {
    let result = eval("throw 'error'");
    assert!(result.contains("uncaught"), "expected uncaught, got: {result}");
    assert!(
        result.contains("uncaught Error"),
        "expected string throw to default to Error, got: {result}"
    );
}

#[test]
fn regression_throw_statement_preserves_type_error_kind() {
    assert_eq!(eval("try { throw new TypeError('boom') } catch (e) { e.name == 'TypeError' }"), "true");
    assert_eq!(eval("throw new TypeError('boom')"), "vm error: uncaught TypeError: boom");
}

#[test]
fn regression_throw_statement_preserves_syntax_error_kind() {
    assert_eq!(
        eval("try { throw new SyntaxError('boom') } catch (e) { e.name == 'SyntaxError' }"),
        "true"
    );
    assert_eq!(eval("throw new SyntaxError('boom')"), "vm error: uncaught SyntaxError: boom");
}

#[test]
fn regression_for_in_prototype_chain() {
    assert_eq!(
        eval("var c=0;for(var k in {a:1}){c=c+1;}c"),
        "2",
        "for-in should include inherited constructor from prototype"
    );
}

#[test]
fn regression_vm_step_limit_is_configurable() {
    let mut config = KernelConfig::minimal();
    config.max_steps = Some(5);
    let kernel = KernelCore::new(config);
    let result = eval_with_kernel("while (true) {}", kernel);
    assert!(
        result.contains("VM step limit exceeded"),
        "expected configurable step limit error, got: {result}"
    );
}

#[test]
fn regression_new_expression_native_constructor_error_is_catchable() {
    assert_eq!(
        eval("try { new RegExp('['); 0 } catch (e) { 1 }"),
        "1",
        "expected native constructor error to enter catch"
    );
}

#[test]
fn regression_new_expression_bytecode_constructor_error_is_catchable() {
    assert_eq!(
        eval("try { class C { constructor() { throw new Error('boom') } } new C(); 0 } catch (e) { 1 }"),
        "1",
        "expected bytecode class constructor error to enter catch"
    );
}

#[test]
fn regression_delete_static_property_throws_type_error() {
    assert_eq!(eval("try { var o={x:1}; delete o.x; 0 } catch (e) { e.name == 'TypeError' }"), "true");
    assert_eq!(
        eval("var o={x:1}; delete o.x"),
        "vm error: uncaught TypeError: property deletion not supported"
    );
}

#[test]
fn regression_delete_dynamic_property_throws_type_error() {
    assert_eq!(eval("try { var o={x:1}; delete o['x']; 0 } catch (e) { e.name == 'TypeError' }"), "true");
    assert_eq!(
        eval("var o={x:1}; var k='x'; delete o[k]"),
        "vm error: uncaught TypeError: property deletion not supported"
    );
}
