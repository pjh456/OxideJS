//! 可写 builtin 全局双写（镜像槽 + 全局对象属性）回归测试。
//!
//! 规范可写全局（11 个 w:true 函数 + 48 个构造器/命名空间 + globalThis + 宿主名，
//! 描述符 {writable:true, enumerable:false, configurable:true}）的标识符写须同步
//! 全局对象属性，裸读（镜像）与 globalThis 反射（属性）不失步。覆盖：简单/复合/
//! 更新/逻辑赋值、for-in/for-of 标识符目标、解构两臂、局部遮蔽不误伤、strict 可写
//! put 成功、eval 路径与 TypedArray 宿主名抽样。

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

// ── 简单赋值：镜像与属性同步落值 ──
#[test]
fn simple_assign_math_dual_writes() {
    eval_truthy("var m = Math; Math = 5; Math === 5 && globalThis.Math === 5 && m !== 5");
}

#[test]
fn simple_assign_writable_names_dual_write() {
    for name in ["Object", "JSON", "Symbol", "Promise"] {
        let src = format!("var m = {name}; {name} = 5; {name} === 5 && globalThis.{name} === 5");
        eval_truthy(&src);
    }
}

// ── w:true 函数族：strict 下可写 put 成功（无 TypeError） ──
#[test]
fn strict_writable_function_put_succeeds() {
    eval_truthy("'use strict'; parseInt = 42; parseInt === 42 && globalThis.parseInt === 42");
}

#[test]
fn sloppy_writable_function_put_succeeds() {
    eval_truthy("parseInt = 42; parseInt === 42 && globalThis.parseInt === 42");
}

#[test]
fn uri_family_dual_write() {
    eval_truthy("var m = encodeURI; encodeURI = 1; encodeURI === 1 && globalThis.encodeURI === 1");
}

// ── 复合/更新/逻辑赋值：RMW 新值两侧同步 ──
#[test]
fn compound_assign_dual_writes() {
    eval_truthy("var m = Math; Math = 5; Math += 1; Math === 6 && globalThis.Math === 6");
}

#[test]
fn update_assign_dual_writes() {
    eval_truthy("var m = Math; Math = 5; var r = (Math++); r === 5 && Math === 6 && globalThis.Math === 6");
}

#[test]
fn logical_assign_dual_writes() {
    // ||=：LHS falsy 触发 put；??=：LHS nullish 触发 put。
    eval_truthy("var m = Math; Math = 0; Math ||= 9; Math === 9 && globalThis.Math === 9");
    eval_truthy("var m = Math; Math = null; Math ??= 2; Math === 2 && globalThis.Math === 2");
}

#[test]
fn logical_assign_short_circuit_no_put() {
    // 短路未通过不发生 put（&&= 遇 falsy、??= 遇非 nullish）：值与属性均保持。
    eval_truthy("var m = Math; Math = 0; Math &&= 1; Math === 0 && globalThis.Math === 0");
    eval_truthy("var m = Math; Math = 5; Math ??= 2; Math === 5 && globalThis.Math === 5");
}

// ── for-in/for-of 标识符目标：迭代值两侧同步 ──
#[test]
fn for_in_target_dual_writes() {
    eval_truthy("var m = Math; Math = 5; for (Math in {a:1,b:2}) {} Math === 'b' && globalThis.Math === 'b'");
}

#[test]
fn for_of_target_dual_writes() {
    eval_truthy("var m = Math; for (Math of [1,2]) {} Math === 2 && globalThis.Math === 2");
}

// ── 解构赋值两臂：值两侧同步 ──
#[test]
fn destructure_assign_dual_writes() {
    eval_truthy("var m = Math; ({Math} = {Math: 1}); Math === 1 && globalThis.Math === 1");
    eval_truthy("var m = Math; ([Math = 1] = [2]); Math === 2 && globalThis.Math === 2");
}

// ── 局部遮蔽不误伤：函数内 var 同名写不触全局 ──
#[test]
fn local_shadow_not_dual_written() {
    eval_truthy(
        "var m = Math; function g(){ var Math = 7; return Math; } \
         g() === 7 && Math === m && globalThis.Math === m",
    );
}

// ── globalThis 自身可写：镜像与属性同步 ──
#[test]
fn global_this_self_dual_writes() {
    eval_truthy(
        "var m = globalThis; globalThis = 1; \
         globalThis === 1 && Object.getOwnPropertyDescriptor(m, 'globalThis').value === 1",
    );
}

// ── 宿主名（TypedArray 抽象运算对象）同双写面 ──
#[test]
fn typed_array_host_name_dual_writes() {
    eval_truthy("var m = TypedArray; TypedArray = 1; TypedArray === 1 && globalThis.TypedArray === 1");
}

// ── eval 路径：同一 emit 管线双写同效（外层镜像预载不重载，只钉属性面） ──
#[test]
fn eval_code_dual_writes_property() {
    eval_truthy("eval('Math = 5'); globalThis.Math === 5");
}
