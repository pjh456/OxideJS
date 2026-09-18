//! 函数入口 `var` 绑定实例化：未捕获 `var` 槽在首次写入前读取须见 undefined，
//! 不得取到调用方遗留的寄存器值。
//!
//! 覆盖：赋值前 typeof 读主形、纯声明无后续写、死分支/for 头 var、直接读、
//! 多 var、赋值后读与带初始化器守卫、嵌套函数自身 var、捕获名守卫、形参与
//! `arguments` 守卫、`x=5; var x` 声明 no-op 形、多形参移位染色形。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

fn truthy(source: &str) {
    let result = eval(source).unwrap_or_else(|e| panic!("{source} -> {e}"));
    assert!(result.as_bool(), "{source} -> 期望 true，得 {:?}", result);
}

#[test]
fn var_read_before_assignment_is_undefined() {
    // 主形：赋值前 `typeof x` 不得取到调用方残留（残留曾使结果为 'string'）。
    truthy("function f(){ var x; var r=typeof x; x=1; return r } f() === 'undefined'");
}

#[test]
fn var_typeof_before_any_write_is_undefined() {
    // 纯声明形：函数内 var 只声明不写，读者见 undefined。
    truthy("function f(){ var x; return typeof x } f() === 'undefined'");
}

#[test]
fn var_direct_read_before_assignment_is_undefined() {
    // 直接读（非 typeof）同样见 undefined。
    truthy("function f(){ var x; var r=x; x=1; return r } f() === undefined");
}

#[test]
fn var_in_dead_branch_read_is_undefined() {
    // 死分支内声明：var 仍提升到函数入口实例化。
    truthy("function f(){ if(false) var x; return typeof x } f() === 'undefined'");
}

#[test]
fn var_in_for_head_zero_iterations_read_is_undefined() {
    // C-for 头 var 零迭代：入口实例化兜底，无运行期写入也见 undefined。
    truthy("function f(){ for (var x; false;) {} return typeof x } f() === 'undefined'");
}

#[test]
fn var_in_for_in_head_zero_iterations_read_is_undefined() {
    // for-in 头 var 零迭代（空可迭代）：入口实例化是唯一到达定义，读见 undefined。
    truthy("function f(){ for (var x in {}) {} return typeof x } f() === 'undefined'");
}

#[test]
fn var_in_for_of_head_zero_iterations_read_is_undefined() {
    // for-of 头 var 零迭代（空可迭代）：入口实例化是唯一到达定义，读见 undefined。
    truthy("function f(){ for (var x of []) {} return typeof x } f() === 'undefined'");
}

#[test]
fn multi_var_all_undefined() {
    // 多 var 名每个槽都实例化（曾一名残留、一名 undefined）。
    truthy("function f(){ var a,b; return typeof a + typeof b } f() === 'undefinedundefined'");
}

#[test]
fn var_assignment_before_read_keeps_value() {
    // 守卫：赋值后读仍是赋值结果，入口实例化不覆写运行期写入。
    truthy("function f(){ var x; x=1; return typeof x } f() === 'number'");
}

#[test]
fn var_initializer_before_read_keeps_value() {
    // 守卫：带初始化器的声明在入口实例化之后执行，保留初始化值。
    truthy("function f(){ var x=1; return typeof x } f() === 'number'");
}

#[test]
fn nested_function_own_var_is_undefined() {
    // 嵌套函数自身的 var 同样在入口实例化。
    truthy("function f(){ function g(){ var y; return typeof y } return g() } f() === 'undefined'");
}

#[test]
fn captured_var_stays_undefined() {
    // 守卫：被闭包捕获的 var 由 cell 入口实例化，读取仍为 undefined。
    truthy("function f(){ var x; var g=function(){ return typeof x }; return g() } f() === 'undefined'");
}

#[test]
fn param_value_not_overwritten_by_entry_init() {
    // 守卫：形参名过滤，入口实例化不冲毁实参写入。
    truthy("function f(a){ var x; return a + typeof x } f(5) === '5undefined'");
}

#[test]
fn arguments_not_overwritten_by_entry_init() {
    // 守卫：arguments 名过滤，入口实例化不冲毁 arguments 对象。
    truthy("function f(){ var x; return arguments.length + typeof x } f(1,2) === '2undefined'");
}

#[test]
fn declaration_after_assignment_is_noop() {
    // 规范：`var x;` 在既有运行期写入后是空操作，保留值 5。
    truthy("function f(){ x=5; var x; return x } f() === 5");
}

#[test]
fn many_params_do_not_change_result() {
    // 多形参移位染色：结果与寄存器布局无关，恒为 undefined。
    truthy("function f(a,b,c,d,e,g,h,i){ var x; var r=typeof x; x=1; return r } f() === 'undefined'");
}

#[test]
fn captured_and_plain_var_both_undefined() {
    // 捕获名与非捕获名并存：两条实例化路径互不干扰。
    truthy(
        "function f(){ var x,y; var g=function(){ return x }; g(); return typeof x + typeof y } f() === 'undefinedundefined'",
    );
}
