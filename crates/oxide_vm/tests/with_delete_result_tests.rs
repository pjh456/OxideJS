//! with 内 delete 标识符结果值回归测试。
//!
//! `with` 对象有该属性时删对象属性（返回删除结果）；对象无该属性时引用
//! 解析到外层绑定，按 DeleteBinding 定值（var/let/const 绑定恒 false；
//! 未声明名/隐式全局槽/可删全局内置走运行期探针真删）。覆盖：var 回退
//! false、let 回退 false、属性删除分支保真、未声明名探针、可删全局内置
//! 真删加清槽、with 内声明分支。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval_truthy(source: &str) {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("run");
    assert!(result.is_bool() && result.as_bool(), "expected true, got: {result:?}\nsource: {source}");
}

// ── var 回退：with 对象无该属性，外层 var c:false 恒 false ──
#[test]
fn with_delete_outer_var_returns_false() {
    eval_truthy("var o = {}; o.x = 1; var d; with(o) { d = delete o; } d === false && o.x === 1");
}

// ── let 回退：外层 lexical 绑定 DeleteBinding 恒 false ──
#[test]
fn with_delete_outer_let_returns_false() {
    eval_truthy("let l = 1; var o = {}; var d; with(o) { d = delete l; } d === false && l === 1");
}

// ── 属性删除分支保真：with 对象有该属性，真删返 true ──
#[test]
fn with_delete_object_property_returns_true() {
    eval_truthy("var o = {p: 1}; var d; with(o) { d = delete p; } d === true && !('p' in o)");
}

// ── 未声明名回退：探针缺失臂返 true ──
#[test]
fn with_delete_undeclared_name_returns_true() {
    eval_truthy("var o = {}; var d; with(o) { d = delete nomatch_w1; } d === true");
}

// ── 可删全局内置回退：真删全局属性并清镜像槽 ──
#[test]
fn with_delete_deletable_builtin_returns_true_and_removed() {
    eval_truthy(
        "var m = Math; var o = {}; var d; with(o) { d = delete Math; } \
         d === true && globalThis.Math === undefined",
    );
}

// ── with 内声明分支：with 内 let 优先于对象属性，落局部绑定 false ──
#[test]
fn with_delete_internal_let_returns_false() {
    eval_truthy("var o = {}; var d; with(o) { let p = 1; d = delete p; } d === false");
}
