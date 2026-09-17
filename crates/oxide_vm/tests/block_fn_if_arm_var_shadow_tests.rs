//! 块内 `if` 支臂裸函数声明与同名 `var` 的绑定归属：Annex B 下支臂函数声明属
//! 隐式支臂块，其名在 var 面落函数/全局作用域 `var`，不得在外层块建块级绑定
//! 遮蔽该 `var`；块内前读须见 `var` 值、外层读须见支臂求值写回。
//!
//! 覆盖：块内前读/typeof 前读/后读、`if(false)` 未求值支、标签包裹支臂、支臂
//! 前 `var` 重赋值、块外后读、外层词法/形参同名不覆写、嵌套 `if`/`else if` 支臂
//! （eval 顶层与形参抑制面）、只读三常量名支臂、strict 早期错误。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

fn eval_str(source: &str) -> Result<String, String> {
    let value = eval(source)?;
    if value.is_string() {
        // SAFETY: 值已确认字符串类别，指针指向 VM 存活期内的 JsString。
        Ok(unsafe { &*value.as_string_ptr() }.as_str().to_string())
    } else {
        Err(format!("expected string, got {value:?}"))
    }
}

#[test]
fn if_arm_fn_var_read_before_branch_sees_var() {
    // 主形：支臂前读与支臂同名的块内 `var`，须见 `var` 已写的 1，
    // 不得取到支臂块绑定槽的陈旧值。
    let source = "function f(){ var g=1; { var r=g; if(true) function g(){return 2}; return r } } f()";
    assert_eq!(eval(source).unwrap().as_int(), 1);
}

#[test]
fn if_arm_fn_var_typeof_before_branch_sees_var() {
    // 主形变体：支臂前 `typeof` 读同名 `var`，须见 "number"。
    let source = "function f(){ var g=1; { var t=typeof g; if(true) function g(){return 2}; return t } } f()";
    assert_eq!(eval_str(source).unwrap(), "number");
}

#[test]
fn if_arm_fn_var_read_before_untaken_branch_sees_var() {
    // if(false) 支臂不求值：块外后读仍见 `var` 值 1。
    let source = "function f(){ var g=1; { if(false) function g(){}; } return g } f()";
    assert_eq!(eval(source).unwrap().as_int(), 1);
}

#[test]
fn if_arm_fn_var_read_after_taken_branch_calls_arm_fn() {
    // 支臂求值后块内读：闭包已写回同名 `var`，调用得支臂返回值 2。
    let source = "function f(){ var g=1; { if(true) function g(){return 2}; return g() } } f()";
    assert_eq!(eval(source).unwrap().as_int(), 2);
}

#[test]
fn labeled_if_arm_fn_var_read_before_branch_sees_var() {
    // 标签包裹的 if 支臂走同一路径：支臂前 `typeof` 见 `var` 值 "number"。
    let source = "function f(){ var g=1; { var t=typeof g; l: if(true) function g(){return 2}; return t } } f()";
    assert_eq!(eval_str(source).unwrap(), "number");
}

#[test]
fn if_arm_fn_var_reassigned_between_reads() {
    // 前读记为 `var` 原值、支臂求值写回后读为函数：读数与求值顺序一致。
    // 结果经布尔比对，避免在 VM 释放后读取拼接字符串指针。
    let source = "function f(){ var g=1; { var r=g; var g=2; if(true) function g(){return 2}; \
                  return r + ' ' + typeof g === '1 function' } } f()";
    assert!(eval(source).unwrap().as_bool());
}

#[test]
fn if_arm_fn_var_outer_read_after_block_is_fn() {
    // 块外后读：支臂闭包经外层 `var` 写回泄漏块外，`typeof` 见 "function"。
    let source = "function f(){ var g=1; { if(true) function g(){return 2}; } return typeof g } f()";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn if_arm_fn_keeps_outer_let_binding() {
    // 守卫：函数体顶层 `let` 与嵌套块内支臂函数同名时，支臂不覆写该词法绑定。
    let source = "function f(){ let g=1; { if(true) function g(){} } return g } f()";
    assert_eq!(eval(source).unwrap().as_int(), 1);
}

#[test]
fn if_arm_fn_keeps_outer_param_binding() {
    // 守卫：形参与嵌套块内支臂函数同名时，支臂不覆写形参值。
    let source = "function f(g){ { if(true) function g(){} } return g } f(42)";
    assert_eq!(eval(source).unwrap().as_int(), 42);
}

#[test]
fn nested_else_if_arm_fn_eval_top_level_keeps_block_binding() {
    // 嵌套 `else if` 支臂在 eval 顶层无外层 var 面承载：须下钻到叶子并建块级
    // 绑定，否则声明点无绑定可物化而报 `Identifier is not defined`。
    let source = "eval(\"{ if(false) {} else if(true) function g(){}; typeof g }\")";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn nested_if_if_arm_fn_eval_top_level_keeps_block_binding() {
    // 嵌套 `if` 支臂（不带 `else`）同形：逐层下钻后叶子建块级绑定。
    let source = "eval(\"{ if(true) if(true) function g(){}; typeof g }\")";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn nested_else_if_arm_fn_keeps_outer_param_binding() {
    // 形参抑制面嵌套 `else if`：支臂闭包写块槽，块外读仍见形参 42。
    let source = "function f(g){ { if(false){} else if(true) function g(){} } return g } f(42)";
    assert_eq!(eval(source).unwrap().as_int(), 42);
}

#[test]
fn if_arm_fn_named_undefined_at_script_top_keeps_block_binding() {
    // 脚本顶层只读三常量无外层 var 绑定承载：块内支臂须建块级绑定，
    // 否则声明点无绑定可物化而编译错。
    let source = "{ if(true) function undefined(){} }";
    assert!(eval(source).unwrap().is_undefined());
}

#[test]
fn if_arm_fn_named_nan_at_script_top_keeps_block_binding() {
    // 同只读三常量面：`NaN` 名为只读全局内置，块内支臂须建块级绑定。
    let source = "{ if(true) function NaN(){} }";
    assert!(eval(source).unwrap().is_undefined());
}

#[test]
fn if_arm_fn_strict_is_early_error() {
    // strict 代码中 if 子句内的函数声明是早期错误（Annex B 仅 sloppy）。
    let err = eval("'use strict'; if(true) function g(){}").unwrap_err();
    assert!(err.contains("Invalid function declaration"), "got: {err}");
}
