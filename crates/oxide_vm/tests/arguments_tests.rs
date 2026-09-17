//! arguments 对象执行语义测试：实参个数/索引读取、箭头函数词法继承、
//! 默认参数引用、方法调用、闭包捕获、显式声明屏蔽、嵌套函数调用帧边界。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn eval_str(vm: &mut Vm, source: &str) -> String {
    let result = eval(vm, source).unwrap_or_else(|e| panic!("{source} -> {e}"));
    vm.lookup_str(result).unwrap_or_else(|| panic!("{source} -> 非字符串结果"))
}

#[test]
fn arguments_length_counts_actual_args() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){return arguments.length}; f(1,2,3)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn arguments_index_reads_first_arg() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(a){return arguments[0]}; f(42)").unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn arguments_index_reads_extra_arg() {
    // 实参多于形参：多余实参仍可从 arguments 读取。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(a){return arguments[1]}; f(1, 99)").unwrap();
    assert_eq!(result.as_int(), 99);
}

#[test]
fn arrow_inherits_outer_arguments() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){return (()=>arguments.length)()}; f(1,2)").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn default_param_can_read_arguments() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(a=arguments.length){return a}; f()").unwrap();
    assert_eq!(result.as_int(), 0);
}

#[test]
fn method_call_arguments() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var o={m:function(){return arguments.length}}; o.m(5,6)").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn closure_captures_arguments() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){var g=()=>arguments[0]; return g()}; f('x')").unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("x"));
}

#[test]
fn arguments_not_created_when_param_named_arguments() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(arguments){return arguments}; f(7)").unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn arguments_not_created_when_var_shadowed() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){var arguments=9; return arguments}; f(1)").unwrap();
    assert_eq!(result.as_int(), 9);
}

#[test]
fn arguments_object_has_length_and_callee() {
    let mut vm = Vm::new();
    let result =
        eval(&mut vm, "function f(){return arguments.callee === f && arguments.length === 2}; f(1,2)").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn arguments_frame_boundary_across_nested_calls() {
    // 子调用返回后父函数的 arguments 实参区仍完好（帧恢复截断不越界）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function g(){return 1} function f(){return arguments.length + g()}; f(4,5,6)").unwrap();
    assert_eq!(result.as_int(), 4, "arguments.length 3 + g() 1 = 4");
}

#[test]
fn nested_plain_function_has_own_arguments() {
    // 非箭头嵌套函数创建自己的 arguments（空实参），不继承外层。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function f(){function g(){return arguments.length} return g() === 0 && arguments.length === 1}; f(8)",
    )
    .unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn new_expression_target_gets_arguments() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function Ctor(){this.n=arguments.length} var c=new Ctor(1,2,3); c.n").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn nested_block_fn_named_arguments_keeps_object_before_decl() {
    // 嵌套块内函数声明名 arguments 不抑制自动对象：块求值前读为 Arguments 对象，
    // 块求值后 web-compat 写回同槽成函数（与 node 一致）。
    let mut vm = Vm::new();
    let result = eval_str(
        &mut vm,
        "function f(){var r=typeof arguments;{function arguments(){}}return r+',' +typeof arguments} f()",
    );
    assert_eq!(result, "object,function");
}

#[test]
fn nested_block_lexical_named_arguments_keeps_object_before_decl() {
    // 嵌套块 let arguments 是块作用域绑定，不进入函数顶层 lexicalNames。
    let mut vm = Vm::new();
    let result = eval_str(&mut vm, "function f(){var r=typeof arguments;{let arguments=1;}return r} f()");
    assert_eq!(result, "object");
}

#[test]
fn var_named_arguments_keeps_object() {
    // var arguments 不在规范抑制面内，入口仍建对象。
    let mut vm = Vm::new();
    let result = eval_str(&mut vm, "function f(){var r=typeof arguments;var arguments;return r} f()");
    assert_eq!(result, "object");
}

#[test]
fn top_level_lexical_named_arguments_suppresses_object() {
    // 函数体顶层 let arguments 与自动绑定同作用域冲突，词法绑定胜出。
    let mut vm = Vm::new();
    let result = eval_str(&mut vm, "function f(){ let arguments='L'; return typeof arguments } f()");
    assert_eq!(result, "string");
}

#[test]
fn destructured_param_leaf_named_arguments_suppresses_object() {
    // 解构形参叶名 arguments 是规范 paramNames 成员，自动对象被抑制。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f({arguments}){return typeof arguments} f({arguments:5})").unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("number"));
}

#[test]
fn default_param_reads_arguments_when_body_has_arguments_fn() {
    // 含参数默认值时顶层函数声明不抑制对象：默认参数内可读到 Arguments 对象。
    let mut vm = Vm::new();
    let result = eval_str(
        &mut vm,
        "var args; function f(x = args = arguments) { function arguments() {} } f(); typeof args+',' +args.length",
    );
    assert_eq!(result, "object,0");
}

#[test]
fn top_level_fn_named_arguments_suppresses_object() {
    // 无参数默认值时函数体顶层函数声明 arguments 抑制自动对象，body 见函数对象。
    let mut vm = Vm::new();
    let result = eval_str(&mut vm, "function f(){return typeof arguments; function arguments(){}} f()");
    assert_eq!(result, "function");
}
