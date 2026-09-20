//! delete 全局内置标识符（DELETE_GLOBAL_PROP_C）回归测试。
//!
//! 可删全局内置（可写全局名除宿主名 $262，描述符
//! {writable:true, configurable:true}）的 delete 标识符运行期真删全局对象属性
//! 并返 true，删除成功时清镜像槽（裸读与 globalThis 反射不失步）。覆盖：删除返
//! 值、属性真删、镜像槽清、删后重写、重复删、三常量 c:false 拒绝、globalThis 真
//! 删、成员形不受影响、参数遮蔽不命中。

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

/// 运行期应抛 ReferenceError 的钉：删后裸读经 A 侧属性在位判定抛缺位引用错误。
fn expect_reference_error(source: &str) {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let err = vm.run(&Arc::new(module)).expect_err("expected ReferenceError");
    assert!(
        err.to_lowercase().contains("referenceerror"),
        "expected ReferenceError, got: {err}\nsource: {source}"
    );
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

// ── 删后读：A 侧缺失抛 ReferenceError（裸读路由全局对象属性）──
#[test]
fn delete_math_then_bare_read_reference_error() {
    expect_reference_error("var m = Math; delete Math; Math");
}

#[test]
fn fn_body_delete_bare_read_reference_error() {
    expect_reference_error("function f() { var m = Math; delete Math; return Math; } f()");
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

// ── globalThis c:true：真删且返 true，镜像槽清 ──
#[test]
fn delete_global_this_returns_true_and_property_removed() {
    // 裸读 globalThis 删后抛 ReferenceError（见 delete_builtin_read_tests）；
    // 此钉保删除返值与 A 侧成员反射。
    eval_truthy(
        "var g = globalThis; var d = delete globalThis; \
         d === true && g.globalThis === undefined",
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
