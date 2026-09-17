//! switch CaseBlock 块作用域：case 内函数/词法声明按块绑定，switch 外不可见；
//! sloppy 下 case 内函数声明的外层 var 半仍按 Annex B 写回，块绑定在 CaseBlock
//! 入口即持有函数对象（声明点前读命中函数）。
//!
//! 覆盖：strict 不泄漏、声明点前读、跨 case 可见、同名重复源序末位、lexical
//! （let/const/class）不泄漏与遮蔽、嵌套块隔离、var 穿透提升、标签直接子体、
//! 捕获逃逸、块绑定与外层 var 独立、重复声明拒绝、完成值守卫。

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

/// 求值为字符串的用例：断言 `is_string` 后取内容。
fn eval_str(source: &str) -> String {
    let value = eval(source).unwrap();
    assert!(value.is_string(), "expected string, got {value:?}");
    // SAFETY: is_string 已确认值为字符串指针，借用仅在断言语句内消费。
    unsafe { &*value.as_string_ptr() }.as_str().to_string()
}

#[test]
fn switch_case_function_strict_not_leaked() {
    // strict 下 case 内函数声明是 CaseBlock 块绑定，switch 外不可见。
    assert_eq!(eval_str("'use strict'; switch(0){case 0:function g(){}} typeof g"), "undefined");
}

#[test]
fn switch_case_function_read_before_decl() {
    // 块函数在 CaseBlock 入口物化：声明语句前读为函数对象。
    assert_eq!(
        eval_str("function f(){ switch(0){case 0: var t=typeof g; function g(){return 1}; return t } } f()"),
        "function"
    );
}

#[test]
fn switch_case_function_visible_across_cases() {
    // 未命中 case 的函数声明同样参与入口物化，case 选择与环境无关。
    assert_eq!(eval_str("switch(1){case 0:function g(){return 1}; default: typeof g }"), "function");
}

#[test]
fn switch_case_duplicate_function_last_wins() {
    // 同名重复声明共享 CaseBlock 块绑定：入口值取源序末次声明。
    assert_eq!(
        eval("switch(0){case 0:function g(){return 1}; break; case 1:function g(){return 2}} g()")
            .unwrap()
            .as_int(),
        2
    );
}

#[test]
fn switch_case_lexical_not_leaked() {
    // case 内 lexical 属 CaseBlock，switch 外读取抛 ReferenceError。
    let err = eval("switch(0){default:const x=1;} x").unwrap_err();
    assert!(err.contains("is not defined"), "got: {err}");
}

#[test]
fn switch_case_let_shadows_outer() {
    // case 内 let 遮蔽外层同名绑定，switch 外读仍是外层值。
    assert_eq!(eval("let x=1; switch(0){case 0:let x=2;} x").unwrap().as_int(), 1);
}

#[test]
fn switch_case_const_not_leaked() {
    let err = eval("switch(0){case 0:const c=1;} c").unwrap_err();
    assert!(err.contains("is not defined"), "got: {err}");
}

#[test]
fn switch_case_class_not_leaked() {
    assert_eq!(eval_str("switch(0){case 0:class C{}} typeof C"), "undefined");
}

#[test]
fn switch_case_lexical_tdz_before_decl() {
    // 声明点前读 case 内 let：命中 TDZ 占位而非隐式全局。
    let err = eval("function f(){ switch(0){case 0: return x; case 1: let x=1;} } f()").unwrap_err();
    assert!(err.contains("before initialization"), "got: {err}");
}

#[test]
fn switch_case_nested_block_lexical_not_leaked() {
    // case 内嵌套块的 lexical 归嵌套块自身，不泄漏到 CaseBlock。
    assert_eq!(
        eval_str("function f(){ switch(0){case 0:{ let y=3; }} return typeof y; } f()"),
        "undefined"
    );
}

#[test]
fn switch_case_var_hoists_to_function_scope() {
    // var 声明穿透 CaseBlock 提升到函数作用域。
    assert_eq!(
        eval("function f(){ switch(0){case 0: var x=1;} return x; } f()")
            .unwrap()
            .as_int(),
        1
    );
}

#[test]
fn switch_case_labeled_function_leaks_outer() {
    // 标签直接子体函数声明同样按块函数处理，sloppy 外层 var 写回。
    assert_eq!(eval_str("switch(0){case 0: l: function g(){return 1}} typeof g"), "function");
}

#[test]
fn switch_case_function_captured_by_outer() {
    // case 内函数被外层引用（捕获路径）后块外可调用。
    assert_eq!(
        eval("var f; switch(0){case 0: function g(){return 5}; f=g; } f()")
            .unwrap()
            .as_int(),
        5
    );
}

#[test]
fn switch_case_function_binding_independent_of_outer_var() {
    // 块绑定可变且与外层 var 独立：块内对 f 重新赋值不改写外层 var 绑定。
    let src = "var r1, r2; (function(){ var initialBV, currentBV; \
        switch(1){case 1: function f(){ initialBV=f; f=123; currentBV=f; return 'decl'; }} \
        var varBinding=f; f(); r1 = initialBV() === 'decl'; \
        r2 = currentBV === 123 && varBinding() === 'decl'; }()); r1 && r2";
    assert!(eval(src).unwrap().as_bool(), "块绑定应独立于外层 var 绑定");
}

#[test]
fn switch_case_function_let_conflict_rejected() {
    // case 内 let 与函数声明同名：重复声明早期错误。
    let err = eval("switch(0){case 0:let g; function g(){}}").unwrap_err();
    assert!(err.contains("already been declared"), "got: {err}");
}

#[test]
fn switch_case_completion_value_preserved() {
    // CaseBlock 作用域压/弹不影响 switch 完成值收敛。
    assert_eq!(eval("switch(1){case 1: 42}").unwrap().as_int(), 42);
    assert_eq!(eval("switch(2){case 1: 42; default: 7}").unwrap().as_int(), 7);
}
