//! 块级函数外层 `var` 绑定的函数入口实例化：sloppy 下块内函数声明名在求值前
//! 读取须见 undefined，不得取到调用方遗留的寄存器值。
//!
//! 覆盖：显式块主形（块前读/块后读/调用）、if 支臂形、显式块内 if 支臂形、
//! 嵌套块形、strict 不泄漏守卫、裸读形、形参同名抑制守卫、直接子标签函数
//! 声明守卫、被嵌套函数捕获形。

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
fn block_fn_before_read_is_undefined_and_after_is_function() {
    // 主形：块前读外层 var 绑定见 undefined，块后读与调用见函数对象。
    truthy("function t(){ var r = typeof g; { function g(){return 7} } return r === 'undefined' && typeof g === 'function' && g() === 7 } t()");
}

#[test]
fn block_fn_if_arm_before_read_is_undefined() {
    // if 支臂形（无显式块）：支臂声明名同样在入口实例化为 undefined。
    truthy("function t(){ var r = typeof g; if(true) function g(){return 1}; return r === 'undefined' && typeof g === 'function' } t()");
}

#[test]
fn block_fn_explicit_block_if_arm_before_read_is_undefined() {
    // 显式块内 if 支臂形：块前读见 undefined，不依赖块 Let 巧合。
    truthy("function t(){ var r = typeof g; { if(true) function g(){return 1}; return r === 'undefined' } } t()");
}

#[test]
fn block_fn_nested_block_before_read_is_undefined() {
    // 嵌套块形：内层块函数名仍提升到函数作用域并在入口实例化。
    truthy("function t(){ var r = typeof g; { { function g(){} } } return r === 'undefined' && typeof g === 'function' } t()");
}

#[test]
fn block_fn_bare_read_before_block_is_undefined() {
    // 裸读形（非 typeof）：入口实例化对裸读同样生效。
    truthy("function t(){ var a = g; { function g(){} } return a === undefined } t()");
}

#[test]
fn block_fn_strict_stays_block_scoped() {
    // strict 守卫：块函数是块级词法绑定，不建外层 var，块外读恒 undefined。
    truthy("function t(){ 'use strict'; var r = typeof g; { function g(){} } return r === 'undefined' && typeof g === 'undefined' } t()");
}

#[test]
fn block_fn_param_suppression_keeps_param_value() {
    // 形参同名抑制守卫：块函数不建外层 var，形参实参值不被入口写冲毁。
    truthy(
        "function t(g){ var r = typeof g; { function g(){} } return r === 'number' && typeof g === 'number' } t(42)",
    );
}

#[test]
fn block_fn_direct_labeled_decl_before_read_stays_function() {
    // 直接子标签函数声明守卫：入口物化闭包，声明点前读仍是函数对象。
    truthy("function t(){ var r = typeof g; l: function g(){return 1}; return r === 'function' && typeof g === 'function' } t()");
}

#[test]
fn block_fn_captured_before_read_is_undefined() {
    // 捕获形：块函数名被嵌套函数引用时同样在入口实例化为 undefined，块前读不得
    // 取调用方寄存器残留；块求值后闭包读到写回的函数对象。
    truthy("function t(){ var r = typeof g; var f = function(){ return g }; { function g(){} } return r === 'undefined' && typeof f() === 'function' } t()");
}
