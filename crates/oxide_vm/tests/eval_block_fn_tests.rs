//! sloppy eval 顶层块级函数声明的 web-compat 外层绑定：Annex B.3.3.3 要求
//! eval 代码中不属于直接词汇声明的块级函数名并入 eval var 环境，间接/全局
//! 口径下即全局对象可配置属性。覆盖块形/嵌套块/标签、`if` 支臂、声明前读、
//! 描述符、delete、同名 var/let 碰撞、strict 守卫与可写内置名。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

/// 求值并把结果按字符串提取（`JsValue` Display 不携带字符串内容）。
fn eval_str(source: &str) -> String {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).expect("run");
    vm.lookup_str(result).unwrap_or_else(|| format!("{result}"))
}

fn assert_bool(result: Result<JsValue, String>) {
    let value = result.unwrap();
    assert!(value.is_bool() && value.as_bool(), "expected true, got {:?}", value);
}

// ── 主形：eval 顶层块函数泄漏到全局，调用方 typeof 见 function ──
#[test]
fn eval_block_function_leaks_to_global_typeof() {
    let s = eval_str("eval('{function g(){}}'); typeof g");
    assert_eq!(s, "function", "eval 顶层块函数应泄漏到全局，实际 {s}");
}

// ── 反射面：globalThis.g 可见 ──
#[test]
fn eval_block_function_visible_on_global_object() {
    assert_bool(eval("eval('{function g(){}}'); typeof globalThis.g === 'function'"));
}

// ── 描述符：eval 全局属性可写/可枚举/可配置 ──
#[test]
fn eval_block_function_global_descriptor_all_true() {
    let s = eval_str(
        "eval('{function g(){}}'); \
         var d = Object.getOwnPropertyDescriptor(globalThis, 'g'); \
         String(d.writable) + ':' + String(d.enumerable) + ':' + String(d.configurable)",
    );
    assert_eq!(s, "true:true:true", "eval 块函数全局属性描述符应全 true，实际 {s}");
}

// ── 声明前读：var 绑定在求值前实例化为 undefined，属性已存在 ──
#[test]
fn eval_block_function_binding_created_before_declaration() {
    let r = eval(
        "var t = eval('var t = typeof f; {function f(){}}; t'); \
         t === 'undefined' && ('f' in globalThis)",
    );
    assert_bool(r);
}

// ── 嵌套块：内层块函数的泄漏名仍并入 eval var 环境 ──
#[test]
fn eval_nested_block_function_leaks_to_global() {
    let s = eval_str("eval('{{function g(){}}}'); typeof g");
    assert_eq!(s, "function", "eval 嵌套块函数应泄漏到全局，实际 {s}");
}

// ── 标签包裹：块内标签直接子函数同走外层绑定 ──
#[test]
fn eval_labeled_block_function_leaks_to_global() {
    let s = eval_str("eval('{ l: function g(){} }'); typeof g");
    assert_eq!(s, "function", "eval 块内标签函数应泄漏到全局，实际 {s}");
}

// ── 顶层标签：无块包裹的标签函数声明名同样并入 eval var 环境 ──
#[test]
fn eval_top_level_labeled_function_leaks_to_global() {
    let s = eval_str("eval('l: function g(){}'); typeof g");
    assert_eq!(s, "function", "eval 顶层标签函数应泄漏到全局，实际 {s}");
}

// ── delete：eval 顶层名走可删探测，属性 configurable:true 故可删 ──
#[test]
fn eval_block_function_global_property_deletable() {
    assert_bool(eval("eval('{function g(){}}'); delete g"));
}

// ── if 支臂：支臂函数声明在 eval 顶层有外层 var 承载，不以块 Let 遮蔽 ──
#[test]
fn eval_if_arm_function_leaks_to_global() {
    let s = eval_str("eval('{ if(true) function g(){} }'); typeof g");
    assert_eq!(s, "function", "eval 顶层 if 支臂函数应泄漏到全局，实际 {s}");
}

// ── if 支臂赋值：支臂后赋值写外层 var（全局属性），非支臂块 Let ──
#[test]
fn eval_if_arm_function_assignment_writes_global() {
    let s = eval_str("eval('{ if(true) function g(){}; g = 123; }'); typeof globalThis.g");
    assert_eq!(s, "number", "支臂后赋值应写全局属性，实际 {s}");
}

// ── var 碰撞：同名 var 存在时块函数写回该 var，最终值为函数 ──
#[test]
fn eval_block_function_overrides_same_name_var() {
    let s = eval_str("eval('var f=1; {function f(){}}'); typeof f");
    assert_eq!(s, "function", "同名 var 应被块函数写回覆盖，实际 {s}");
}

// ── let 碰撞守卫：同名词法声明抑制外层绑定，块函数不覆写词法值 ──
#[test]
fn eval_block_function_let_collision_keeps_lexical() {
    let s = eval_str("eval('let f=1; {function f(){}} typeof f')");
    assert_eq!(s, "number", "同名 let 不应被块函数覆写，实际 {s}");
}

// ── strict 守卫：strict eval 的块函数不泄漏全局 ──
#[test]
fn strict_eval_block_function_does_not_leak() {
    let s = eval_str("eval(\"'use strict'; {function g(){}}\"); typeof g");
    assert_eq!(s, "undefined", "strict eval 块函数不应泄漏全局，实际 {s}");
}

// ── 可写内置名：块函数覆写 parseInt 全局属性，调用取覆写值 ──
#[test]
fn eval_block_function_overrides_writable_builtin() {
    let s = eval_str("eval('{function parseInt(){return \"z\"}}'); parseInt('42')");
    assert_eq!(s, "z", "块函数应覆写可写内置名，实际 {s}");
}
