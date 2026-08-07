//! arguments 对象执行语义测试：实参个数/索引读取、箭头函数词法继承、
//! 默认参数引用、方法调用、闭包捕获、显式声明屏蔽、嵌套函数调用帧边界。

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
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
    assert!((result.as_double() - 4.0).abs() < 0.0001);
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
