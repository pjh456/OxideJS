//! 成员写 globalThis 后镜像槽反向失步收口测试。
//!
//! 全局对象双存储：A 侧 = 全局对象属性存储，B 侧 = 镜像寄存器槽（裸标识符
//! 读走 LOAD_VAR）。成员写 `globalThis.X = v` 只动 A 侧，同 run 内 B 侧须经
//! 写原语族挂点反向同步；跨调用/跨帧经返回边界重预载。覆盖：同 run 成员写、
//! 重复写、跨调用、成员删（同 run 与跨调用）、defineProperty 数据描述符形、
//! 裸名写面不回归、写失败不过同步、三常量拦截不回归、meta 不变量白盒钉。
//!
//! 核心形断言方向：成员写后裸读应见新属性值，`Math === m`（m 为写前保存）
//! 期望 false；旧行为（槽不刷新）为 true。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

fn eval_str(source: &str) -> String {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("run");
    vm.lookup_str(result).unwrap_or_else(|| format!("{result}"))
}

fn eval_truthy(source: &str) {
    let r = eval(source).expect("run");
    assert!(r.is_bool() && r.as_bool(), "expected true, got: {r:?}\nsource: {source}");
}

// ── 核心形与重复写：同 run 成员写后裸读见新值 ──
#[test]
fn member_write_same_run_bare_read_sees_new_value() {
    eval_truthy("var m = Math; globalThis.Math = {x:1}; Math !== m && Math.x === 1");
}

#[test]
fn repeated_member_write_hot_path() {
    eval_truthy(
        "var m = Math; globalThis.Math = {a:1}; globalThis.Math = {x:2}; \
         Math !== m && Math.x === 2",
    );
}

// ── 跨调用：调用期 A 侧变更经返回边界重预载 ──
#[test]
fn member_write_across_call() {
    eval_truthy("var m = Math; (function(){ globalThis.Math = {x:1}; })(); Math !== m && Math.x === 1");
}

#[test]
fn member_delete_across_call_bare_read_undefined() {
    eval_truthy(
        "var m = Math; (function(){ delete globalThis.Math; })(); \
         Math === undefined && Math !== m",
    );
}

// ── 成员删同 run：delete 成员形后裸读 undefined ──
#[test]
fn member_delete_same_run_bare_read_undefined() {
    eval_truthy("var m = Math; delete globalThis.Math; Math === undefined && Math !== m");
}

// ── defineProperty 数据描述符形同收口 ──
#[test]
fn define_property_member_form_same_run() {
    eval_truthy(
        "var m = Math; Object.defineProperty(globalThis,'Math',\
         {value:{x:1},writable:true,configurable:true,enumerable:false}); \
         Math !== m && Math.x === 1",
    );
}

// ── 裸名写面不回归 ──
#[test]
fn bare_write_double_write_not_regressed() {
    eval_truthy("parseInt = 42; parseInt === 42 && globalThis.parseInt === 42");
}

#[test]
fn mixed_bare_then_member_write() {
    eval_truthy("var m = parseInt; parseInt = 42; globalThis.parseInt = 43; parseInt === 43");
}

// ── 写失败不过同步：只读属性 no-op 写不污染镜像槽 ──
#[test]
fn failed_write_does_not_sync_mirror() {
    let s = eval_str("globalThis.undefined = 5; '' + undefined");
    assert_eq!(s, "undefined", "只读 no-op 写不得同步镜像槽，裸 undefined 读回 undefined");
}

// ── 三常量裸名拦截不回归 ──
#[test]
fn constant_bare_write_still_intercepted() {
    let s = eval_str("undefined = 3; '' + undefined");
    assert_eq!(s, "undefined", "sloppy 裸写三常量静默 no-op");
    let r = eval("(function(){ 'use strict'; undefined = 3; })()");
    assert!(r.is_err(), "strict 裸写三常量须抛 TypeError，got: {r:?}");
}

// ── meta 不变量白盒钉：全局对象恒带 prop_meta ──
// 该不变量是 IC 写命中分支对全局对象不可达的前提；若被破坏（全局对象
// meta-less 化），IC 快路径可达而镜像同步面须重审，此钉先红提示。
#[test]
fn global_object_always_has_prop_meta() {
    let core = oxide_kernel::KernelCore::new(oxide_kernel::KernelConfig::minimal());
    let session = oxide_kernel::KernelSession::new(&core);
    assert!(
        session.global_object().has_prop_meta(),
        "全局对象自构造起恒带 prop_meta（三常量 set_data_meta 落定）"
    );
}
