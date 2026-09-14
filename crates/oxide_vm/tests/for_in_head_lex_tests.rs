//! for-in/for-of 词法头独立环境钉：头名与顶层 var 同名的捕获错位（Gap-1）
//! 与右值区 TDZ 环境缺失（Gap-2）两面的引擎侧形态锁。
//!
//! 完成值一律收敛为布尔（JsValue 的 Display 只暴露 number/bool，不暴露
//! 字符串内容），字符串比较在引擎内完成。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
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
    match vm.run(&Arc::new(module)) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

// ── Gap-1：头名与顶层 var 同名，体内闭包捕获头绑定（非全局属性）──

#[test]
fn for_in_let_head_same_name_as_top_var_closure_reads_head() {
    // let 头与顶层 var 同名：循环后调用体内闭包读头绑定，值 = 末迭代键；
    // 读错成顶层 var 的全局属性（'top'）即红。
    assert_eq!(
        eval("var x = \"top\"; var p; for (let x in {a:1,b:2}) { p = function(){ return x; }; } p() === \"b\""),
        "true",
        "closure in body captures the let head binding, not the same-named top-level var"
    );
}

#[test]
fn for_in_nested_lexical_heads_inner_closure_reads_inner_head() {
    // 双层词法头同名遮蔽：内层头捕获优先，循环后读内层头末值（'r'）。
    assert_eq!(
        eval(
            "var x = \"top\"; var p; for (let x in {a:1}) { \
              for (let x of ['q','r']) { p = function(){ return x; }; } } \
              p() === \"r\""
        ),
        "true",
        "nested lexical heads shadow each other; the inner head wins in the inner body"
    );
}

#[test]
fn for_of_let_head_same_name_as_top_var_closure_reads_head() {
    // for-of 孪生主形：同步与 await 两臂共用同一覆盖。
    assert_eq!(
        eval("var x = \"top\"; var p; for (let x of ['q','r']) { p = function(){ return x; }; } p() === \"r\""),
        "true",
        "for-of let head closure captures the head binding, not the top-level var"
    );
}

// ── Gap-2：右值区对头名的读（直读/闭包捕获）必须抛 ReferenceError ──

#[test]
fn for_in_rhs_direct_read_of_let_head_throws_reference_error() {
    // 右值区直读头名：头名在 TDZ 环境中未初始化，读抛 ReferenceError；
    // 穿透外层已初始化同名绑定（不抛）即红。
    assert_eq!(
        eval(
            "let x = 1; var r = 0; \
              try { for (let x in { x }) { r = 2; } } \
              catch (e) { r = (e instanceof ReferenceError) ? 1 : 3; } r === 1"
        ),
        "true",
        "direct read of the uninitialized let head in the RHS zone throws ReferenceError"
    );
}

#[test]
fn for_of_rhs_closure_capturing_let_head_throws_reference_error() {
    // 右值区创建、捕获头名的闭包：循环后调用抛 ReferenceError；
    // 闭包捕获外层已初始化同名 cell（不抛）即红。
    assert_eq!(
        eval(
            "let x = 1; var probe; var r = 0; \
              for (let x in { a: (probe = function(){ return x; }) }) { r = 2; } \
              try { probe(); r = 3; } catch (e) { r = (e instanceof ReferenceError) ? 1 : 4; } r === 1"
        ),
        "true",
        "closure created in the RHS zone captures the TDZ head cell and throws on read"
    );
}
