//! eval 脚本 var/函数声明落全局对象的属性语义测试：configurable:true（区别于普通
//! 脚本顶层的 false）、可 delete、跨 eval 轮次经全局对象可见。

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

fn eval_many(lines: &[&str]) -> Result<JsValue, String> {
    eval(&lines.join("; "))
}

fn assert_err_contains(result: Result<JsValue, String>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error containing '{}', got Ok", expected),
        Err(e) => assert!(e.contains(expected), "expected error containing '{}', got: {}", expected, e),
    }
}

/// 求值并把结果按字符串提取（`JsValue` Display 不携带字符串内容）。
fn eval_str(source: &str) -> String {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&module).expect("run");
    vm.lookup_str(result).unwrap_or_else(|| format!("{result}"))
}

// ── eval var 落全局属性 configurable:true ──
#[test]
fn eval_var_global_property_configurable_true() {
    let s = eval_str("eval('var x;'); var d = Object.getOwnPropertyDescriptor(globalThis, 'x'); String(d.configurable) + ':' + String(d.writable) + ':' + String(d.enumerable)");
    assert_eq!(s, "true:true:true", "eval var 全局属性描述符应全 true，实际 {s}");
}

// ── eval var 属性可 delete（configurable:true 的推论） ──
#[test]
fn eval_var_global_property_deletable() {
    let r = eval_many(&["eval('var x = 1')", "delete globalThis.x === true && typeof x === 'undefined'"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "eval var 属性应可 delete 且删后读回 undefined，实际 {:?}", r);
}

// ── eval var 带初始化值经全局对象可见 ──
#[test]
fn eval_var_with_initializer_lands_on_global() {
    let r = eval_many(&["eval('var x = 5')", "globalThis.x === 5"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "eval var 初始化值应落全局对象，实际 {:?}", r);
}

// ── 跨 eval 轮次：全局属性经 session 全局对象共享 ──
#[test]
fn eval_var_visible_across_eval_rounds() {
    let r = eval_many(&["eval('var a = 1')", "eval('a') === 1"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "跨 eval 轮次应经全局对象可见，实际 {:?}", r);
}

// ── eval 函数声明落全局属性 configurable:true 且可调用 ──
#[test]
fn eval_function_declaration_configurable_and_callable() {
    let s = eval_str(
        "eval('function f(){ return 1 }'); \
         var d = Object.getOwnPropertyDescriptor(globalThis, 'f'); \
         String(d.configurable) + ':' + String(f())",
    );
    assert_eq!(s, "true:1", "eval 函数声明应落全局 configurable:true 且可调用，实际 {s}");
}

// ── 回归：普通脚本顶层 var 仍 configurable:false ──
#[test]
fn script_top_level_var_still_configurable_false() {
    let r = eval_many(&["var y;", "Object.getOwnPropertyDescriptor(globalThis, 'y').configurable === false"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "脚本顶层 var 应保持 configurable:false，实际 {:?}", r);
}

// ── 严格 eval：未声明写抛 ReferenceError（Step 2 语义经 eval 路径） ──
#[test]
fn strict_eval_undeclared_write_throws_reference_error() {
    let result = eval("eval(\"'use strict'; x = 1\")");
    assert_err_contains(result, "x is not defined");
}
