//! 块级 async/生成器函数声明不泄漏：Annex B.3.3 的 web-compat 外层 var 绑定
//! 仅覆盖普通函数声明。async 函数、生成器与 async 生成器是块级词法绑定，
//! 块外不可见，也不覆写同名外层 var；块内读取仍命中块绑定。
//!
//! 覆盖脚本顶层块、函数体内块、直接/间接 eval、嵌套块、switch case、try 块，
//! 以及同名 var 碰撞与普通函数泄漏正例守卫。

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

fn assert_true(source: &str) {
    let result = eval(source).unwrap();
    assert!(result.as_bool(), "期望 true，源: {source}，得 {result:?}");
}

#[test]
fn block_async_fn_decl_not_leaked_in_script() {
    // 脚本顶层块：块级 async 函数声明不建外层绑定，块外 typeof 为 undefined。
    assert_true("{async function a(){}} typeof a === 'undefined'");
}

#[test]
fn block_generator_decl_not_leaked_in_script() {
    // 生成器声明同为块级词法绑定，不泄漏。
    assert_true("{function* g(){}} typeof g === 'undefined'");
}

#[test]
fn block_async_generator_decl_not_leaked_in_script() {
    // async 生成器声明不泄漏。
    assert_true("{async function* ag(){}} typeof ag === 'undefined'");
}

#[test]
fn block_async_fn_decl_not_reflected_on_global() {
    // 不泄漏即不建全局对象属性：globalThis 反射不到块级 async 名。
    assert_true("{async function a(){}} !('a' in globalThis)");
}

#[test]
fn block_async_fn_decl_not_leaked_in_function_body() {
    // 函数体内块与脚本面同口径：块外读为 undefined。
    assert_true("function q(){ {async function a(){}} return typeof a === 'undefined' } q()");
}

#[test]
fn block_generator_decl_not_leaked_in_function_body() {
    // 函数体内块生成器同口径。
    assert_true("function q(){ {function* g(){}} return typeof g === 'undefined' } q()");
}

#[test]
fn block_async_fn_decl_not_leaked_in_direct_eval() {
    // 直接 eval 顶层块：不泄漏到 eval 的 var 环境。
    assert_true(r#"function q(){ eval('{async function a(){}}'); return typeof a === 'undefined' } q()"#);
}

#[test]
fn block_generator_decl_not_leaked_in_indirect_eval() {
    // 间接 eval 顶层块：生成器名不落全局。
    assert_true(r#"(0,eval)('{function* g(){}}'); typeof g === 'undefined'"#);
}

#[test]
fn nested_block_async_fn_decl_not_leaked() {
    // 嵌套块：任一深度都不泄漏。
    assert_true("{{async function a(){}}} typeof a === 'undefined'");
}

#[test]
fn switch_case_async_fn_decl_not_leaked() {
    // switch CaseBlock 内 async 声明不泄漏到 switch 外。
    assert_true("switch(0){case 0:async function a(){}} typeof a === 'undefined'");
}

#[test]
fn try_block_async_fn_decl_not_leaked() {
    // try 块内 async 声明不泄漏到语句外，且块内可读。
    assert_true("try{async function a(){}}catch(e){} typeof a === 'undefined'");
}

#[test]
fn try_block_function_decl_entry_visible() {
    // try 块作为独立块作用域：直接子函数声明的块入口物化，声明点前读可用。
    assert_true("var t; try{ t = typeof f; function f(){} }catch(e){} t === 'function'");
}

#[test]
fn block_async_fn_var_collision_keeps_var() {
    // 同名外层 var 不被块级 async 声明覆写。
    assert_true("var a=1; {async function a(){}} typeof a === 'number'");
}

#[test]
fn block_async_fn_var_collision_in_function_body() {
    // 函数体内同名 var 碰撞：块级 async 声明不写回外层 var，仍为 undefined。
    assert_true("(function q(){ {async function a(){}} var a; return typeof a === 'undefined' })()");
}

#[test]
fn block_generator_var_collision_keeps_var() {
    // 生成器同名 var 碰撞同口径。
    assert_true("{function* g(){}} var g; typeof g === 'undefined'");
}

#[test]
fn block_async_fn_readable_inside_block() {
    // 块内读取仍命中块级绑定（声明点后为 function）。
    assert_true("(function(){ {async function a(){}; return typeof a === 'function'} })()");
}

#[test]
fn plain_function_block_still_leaks() {
    // 正例守卫：普通块级函数声明仍按 Annex B.3.3 泄漏外层。
    assert_true("{function f(){}} typeof f === 'function'");
}

#[test]
fn plain_function_var_collision_still_overwritten() {
    // 正例守卫：普通块级函数声明求值写回同名外层 var。
    assert_true("var f=1; {function f(){}} typeof f === 'function'");
}

#[test]
fn strict_block_async_fn_not_leaked() {
    // strict 面不建外层绑定，块级 async 声明不泄漏。
    assert_true("'use strict'; {async function a(){}} typeof a === 'undefined'");
}

#[test]
fn top_level_async_fn_still_global() {
    // 守卫：脚本顶层 async 函数声明（非块级）仍建全局绑定并可反射。
    assert_true("async function top(){} typeof top === 'function' && ('top' in globalThis)");
}
