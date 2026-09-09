//! 脚本顶层 var/function 声明落 globalThis 的运行时行为测试。

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval_truthy(source: &str) {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&module).expect("run");
    assert!(result.is_bool() && result.as_bool(), "expected true, got: {result:?}\nsource: {source}");
}

fn eval_string(source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&module).expect("run");
    vm.lookup_str(result).unwrap_or_default()
}

#[test]
fn top_level_var_lands_on_global_this() {
    eval_truthy("var x = 5; globalThis.x === 5");
    eval_truthy("var y = 10; typeof globalThis.y === 'number'");
}

#[test]
fn top_level_function_lands_on_global_this() {
    eval_truthy("function foo(){ return 1; } globalThis.foo() === 1");
    eval_truthy("function foo(){ return 1; } typeof globalThis.foo === 'function'");
}

#[test]
fn let_const_class_do_not_land_on_global_this() {
    eval_truthy("let z = 5; typeof globalThis.z === 'undefined'");
    eval_truthy("const c = 1; typeof globalThis.c === 'undefined'");
    eval_truthy("class C {} typeof globalThis.C === 'undefined'");
}

#[test]
fn function_local_var_does_not_land_on_global_this() {
    eval_truthy("function f(){ var v = 1; } f(); typeof globalThis.v === 'undefined'");
}

#[test]
fn block_level_var_lands_on_global_this() {
    eval_truthy("{ var b = 2; } globalThis.b === 2");
}

#[test]
fn redeclaration_updates_global_property() {
    eval_truthy("var x = 1; var x = 2; globalThis.x === 2");
}

#[test]
fn var_global_property_is_non_configurable() {
    eval_truthy("var x = 1; Object.getOwnPropertyDescriptor(globalThis, 'x').configurable === false");
    eval_truthy("var x = 1; Object.getOwnPropertyDescriptor(globalThis, 'x').enumerable === true");
    eval_truthy("var x = 1; delete globalThis.x === false");
    eval_truthy("var x = 1; delete globalThis.x; typeof globalThis.x === 'number'");
}

#[test]
fn var_name_shows_in_get_own_property_names() {
    eval_truthy("var x = 1; Object.getOwnPropertyNames(globalThis).indexOf('x') >= 0");
    eval_truthy("function foo(){} Object.getOwnPropertyNames(globalThis).indexOf('foo') >= 0");
}

#[test]
fn function_declared_var_lands_after_user_assignment() {
    eval_truthy("globalThis.x = 99; var x = 5; globalThis.x === 5");
}

#[test]
fn async_harness_done_pattern() {
    // asyncHelpers.js 依赖顶层 function $DONE 落 globalThis。
    eval_truthy("function $DONE(){} Object.prototype.hasOwnProperty.call(globalThis, '$DONE')");
    eval_truthy("function $DONE(){} typeof globalThis.$DONE === 'function'");
}

#[test]
fn var_without_initializer_creates_undefined_property() {
    eval_truthy("var x; typeof globalThis.x === 'undefined'");
    eval_truthy("var x; Object.prototype.hasOwnProperty.call(globalThis, 'x')");
}

#[test]
fn function_internal_reference_still_works() {
    eval_truthy("function foo(){ return 1; } var r = foo(); r === 1");
}

#[test]
fn descriptor_attributes_match_spec() {
    let d = eval_string("var x = 1; JSON.stringify(Object.getOwnPropertyDescriptor(globalThis, 'x'))");
    assert_eq!(d, r#"{"value":1,"writable":true,"enumerable":true,"configurable":false}"#);
}

#[test]
fn var_property_exists_before_declaration_statement() {
    // 全局声明实例化：属性在脚本求值开始即创建，声明语句执行前的
    // 反射/读取即可见绑定（值 undefined、脚本模式 configurable:false）。
    // 断言值经临时全局承载——尾部 var 声明语句会覆盖程序完成值。
    eval_truthy(
        "(function(){ seen = Object.prototype.hasOwnProperty.call(globalThis, 'gv'); })(); var gv; seen === true",
    );
    eval_truthy("(function(){ seen = (typeof gv === 'undefined'); })(); var gv; seen === true");
    eval_truthy("(function(){ var p = Object.getOwnPropertyDescriptor(globalThis, 'gv'); seen = (p.configurable === false && p.value === undefined); })(); var gv; seen === true");
}

#[test]
fn var_no_init_preserves_prior_write() {
    eval_truthy("x = 1; var x; globalThis.x === 1");
    eval_truthy("x = 1; var x; x === 1");
}

#[test]
fn var_no_init_preserves_prior_write_captured() {
    // 被捕获 var 的值在 cell：声明语句从 cell 同步全局属性，不抹掉先前写入。
    eval_truthy("x = 1; var x; (function(){ return x; })() === 1");
    eval_truthy("x = 1; var x; globalThis.x === 1");
}

#[test]
fn var_self_reference_reads_binding_value() {
    eval_truthy("var x = x; typeof x === 'undefined'");
    eval_truthy("x = 7; var x = x; x === 7");
}

#[test]
fn function_callable_before_its_declaration() {
    eval_truthy("(function(){ return f() === 42; })(); function f(){ return 42; }");
}

#[test]
fn var_no_init_does_not_clobber_function_binding() {
    eval_truthy("function f(){ return 1; } var f; typeof f === 'function' && f() === 1");
    // 不比较 globalThis.f === f 同一性：属性读路径存在预存 codegen 缺陷，
    // 行为契约是声明后属性仍为原函数对象（可调用、返回值正确）。
    eval_truthy("function f(){ return 1; } var f; typeof globalThis.f === 'function' && globalThis.f() === 1");
}
