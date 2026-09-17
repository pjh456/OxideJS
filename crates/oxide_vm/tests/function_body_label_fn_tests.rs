//! 函数体直接子级标签声明：sloppy 下标签链直接包裹的函数声明（Annex B.3.2
//! Labelled Function Declarations）与直接子函数声明同等参与函数入口实例化，
//! 声明点前读即见函数对象，声明点不重编以保函数对象同一性。
//!
//! 覆盖：声明前读（typeof/裸读/可调用）、声明前后读、嵌套标签链、函数对象
//! 同一性、与形参同名覆盖、嵌套块词法同名抑制形的编译成功、混合直接子声明的
//! 源序末位生效、逻辑表达式自调用不重入（双输出症状闭合）、块内标签形不回归、
//! 重复标签与 strict 的早期错误。

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
fn labeled_fn_before_read_is_function() {
    // 主形：声明点前 `typeof g` 读到函数对象。
    let source = "(function(){ var t = typeof g; l: function g(){return 1}; return t })()";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn labeled_fn_before_and_after_read() {
    // 声明点前后读一致为 function。
    let source = "(function(){ var a = typeof g; l: function g(){return 1}; var b = typeof g; \
                  return a === 'function' && b === 'function' })()";
    assert!(eval(source).unwrap().as_bool());
}

#[test]
fn labeled_fn_bare_read_yields_callable_closure() {
    // 声明点前裸读 `g` 得到可调用闭包（非寄存器残留）。
    let source = "(function(){ var a = g; l: function g(){return 7}; return a() })()";
    assert_eq!(eval(source).unwrap().as_int(), 7);
}

#[test]
fn labeled_fn_identity_with_declaration_point() {
    // 声明点前读到的函数对象与声明名绑定同一（声明点不重编闭包）。
    let source = "(function(){ var a = g; l: function g(){return 1}; return a === g })()";
    assert!(eval(source).unwrap().as_bool());
}

#[test]
fn labeled_fn_nested_label_chain_hoists() {
    // 嵌套标签链：多层标签直接包裹同样参与函数入口提升。
    let source = "(function(){ var t = typeof g; l1: l2: function g(){return 1}; return t })()";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn labeled_fn_overrides_parameter() {
    // 与形参同名：函数声明入口实例化覆盖形参值，读面见函数对象。
    let source = "(function f(g){ var t = typeof g; l: function g(){return 1}; return t })(42)";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn labeled_fn_suppressed_by_nested_lexical_compiles() {
    // 嵌套块内词法同名不压制函数体顶层标签函数声明的绑定：编译成功且读面为函数。
    let source = "(function(){ l: function g(){return 1}; { let g; } return typeof g })()";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn labeled_fn_mixed_with_direct_child_last_wins() {
    // 直接子与标签形混合：源序末位声明生效，声明点前读与末位绑定同一。
    let source = "(function(){ var a = g; l: function g(){return 1}; function g(){return 2}; \
                  return a === g && g() === 2 })()";
    assert!(eval(source).unwrap().as_bool());
}

#[test]
fn labeled_fn_logical_call_value() {
    // 声明点前读入变量后在逻辑表达式右操作数调用，值等于函数返回值。
    let source = "(function(){ var a = g; l: function g(){return 1}; return a && a() })()";
    assert_eq!(eval(source).unwrap().as_int(), 1);
}

#[test]
fn labeled_fn_logical_call_no_reentry() {
    // 入口持有真闭包后自调用不重入：计数与返回值同帧收敛为 11；若重入读到非
    // 函数，`globalThis.n * 10 + (a && a())` 会得 NaN。
    let source = "(function(){ var a = g; l: function g(){return 1}; \
                  globalThis.n = (globalThis.n||0)+1; return globalThis.n * 10 + (a && a()) })()";
    let value = eval(source).unwrap();
    let number = if value.is_int() { value.as_int() as f64 } else { value.as_double() };
    assert_eq!(number, 11.0, "got {value:?}");
}

#[test]
fn labeled_fn_after_read_guard() {
    // 守卫：声明点后读仍为 function。
    let source = "(function(){ l: function g(){return 1}; return typeof g })()";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn labeled_fn_other_name_before_read_undefined_guard() {
    // 守卫：标签声明其它名字时，未声明名前读仍为 undefined。
    let source = "(function(){ var t = typeof g; l: function h(){return 1}; return t })()";
    assert_eq!(eval_str(source).unwrap(), "undefined");
}

#[test]
fn direct_child_fn_before_read_guard() {
    // 守卫：直接子函数声明前读保持函数对象，与本修路径无关。
    let source = "(function(){ var a = h; function h(){return 1}; return a && a() })()";
    assert_eq!(eval(source).unwrap().as_int(), 1);
}

#[test]
fn labeled_fn_in_block_still_entry_visible_guard() {
    // 守卫：块内标签形走块面入口物化，不因函数面改动回归。
    let source = "{ var t = typeof g; l: function g(){return 1}; t }";
    assert_eq!(eval_str(source).unwrap(), "function");
}

#[test]
fn labeled_fn_in_block_leaks_outer_guard() {
    // 守卫：块内标签形经外层 var 写回泄漏块外。
    assert_eq!(eval("{ l: function g(){return 1} } g()").unwrap().as_int(), 1);
}

#[test]
fn labeled_fn_duplicate_label_is_early_error() {
    // 重复标签是早期错误：函数面提升不放过非法标签形。
    let err = eval("(function(){ a: a: function g(){} })").unwrap_err();
    assert!(err.contains("already been declared"), "got: {err}");
}

#[test]
fn labeled_fn_strict_is_early_error() {
    // strict 代码中标签函数声明是早期错误。
    let err = eval("(function(){ 'use strict'; l: function g(){} })").unwrap_err();
    assert!(err.contains("Invalid function declaration"), "got: {err}");
}
