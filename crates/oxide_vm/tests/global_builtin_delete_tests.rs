//! delete 全局内置标识符（DELETE_GLOBAL_PROP_C）回归测试。
//!
//! 可删全局内置（可写全局名除 globalThis 与宿主名，描述符
//! {writable:true, configurable:true}）的 delete 标识符运行期真删全局对象属性
//! 并返 true，删除成功时清镜像槽（裸读与 globalThis 反射不失步）。覆盖：删除返
//! 值、属性真删、镜像槽清、删后重写、重复删、三常量 c:false 拒绝、globalThis 拒
//! 绝、成员形不受影响、参数遮蔽不命中。

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

// ── 删除返值与属性真删 ──
#[test]
fn delete_math_returns_true_and_property_removed() {
    eval_truthy("var m = Math; var d = delete Math; d === true && globalThis.Math === undefined");
}

#[test]
fn delete_writable_builtin_names() {
    for name in ["Object", "JSON", "Symbol", "Promise", "parseInt"] {
        let src = format!("var m = {name}; var d = delete {name}; d === true && globalThis.{name} === undefined");
        eval_truthy(&src);
    }
}

// ── 镜像槽清：删除后裸读回 undefined ──
#[test]
fn delete_math_then_bare_read_via_mirror() {
    eval_truthy("var m = Math; delete Math; Math === undefined");
}

#[test]
fn fn_body_delete_clears_frame_mirror() {
    eval_truthy(
        "function f() { var m = Math; delete Math; \
         return Math === undefined && globalThis.Math === undefined; } f() === true",
    );
}

// ── 删后重写：两侧再同步 ──
#[test]
fn delete_then_reassign_both_sides() {
    eval_truthy("var m = Math; delete Math; Math = 5; Math === 5 && globalThis.Math === 5");
}

// ── 重复删：属性已缺失仍返 true ──
#[test]
fn delete_twice_second_returns_true() {
    eval_truthy("var m = Math; delete Math; delete Math === true && globalThis.Math === undefined");
}

// ── 三常量 c:false：拒绝且属性保留 ──
#[test]
fn delete_three_constants_refused_property_kept() {
    eval_truthy(
        "delete undefined === false && delete NaN === false && delete Infinity === false \
         && Object.getOwnPropertyDescriptor(globalThis, 'undefined') !== undefined \
         && Object.getOwnPropertyDescriptor(globalThis, 'NaN') !== undefined \
         && Object.getOwnPropertyDescriptor(globalThis, 'Infinity') !== undefined",
    );
}

// ── globalThis c:false：拒绝且属性保留 ──
#[test]
fn delete_global_this_refused_property_kept() {
    eval_truthy(
        "delete globalThis === false \
         && Object.getOwnPropertyDescriptor(globalThis, 'globalThis') !== undefined",
    );
}

// ── 成员形不受影响（既有 DELETE_PROP_STATIC 路径） ──
#[test]
fn member_delete_unchanged() {
    eval_truthy("delete Math.LN2 === false");
}

// ── 参数遮蔽：不注册镜像槽，delete 落非严格 false ──
#[test]
fn parameter_shadow_delete_false() {
    eval_truthy("function f(Math) { return delete Math; } f(1) === false");
}
