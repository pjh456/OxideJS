//! sloppy 模式赋值静默失败语义测试：ordinary_set 失败分支（自身/继承只读数据、
//! 无 setter 访问器、不可扩展新属性）在 sloppy 静默 no-op，严格模式抛 TypeError。
//! 覆盖脚本顶层（`top_level_strict`）、函数级（帧 `strict`）与内联内置回调
//! （forEach 等，`inline_strict` + 帧基线）三种执行上下文。

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

// ── sloppy 脚本顶层：无 setter 访问器赋值静默 no-op（值不变、不抛） ──
#[test]
fn sloppy_script_top_level_accessor_no_setter_silently_fails() {
    let r = eval_many(&["var o = { get h() { return 1 } }", "o.h = 2", "o.h"]).unwrap();
    assert!(
        r.is_int() && r.as_int() == 1,
        "sloppy 无 setter 访问器写应静默 no-op，值保持 1，实际 {:?}",
        r
    );
}

// ── 严格脚本（顶层指令）：无 setter 访问器赋值抛 TypeError ──
#[test]
fn strict_script_top_level_accessor_no_setter_throws() {
    let result = eval("'use strict'; var o = { get h() { return 1 } }; o.h = 2");
    assert_err_contains(result, "no setter");
}

// ── sloppy 脚本顶层：只读自身属性赋值静默失败 ──
#[test]
fn sloppy_script_top_level_readonly_silently_fails() {
    let r = eval("var o = {}; Object.defineProperty(o, 'x', {value: 1, writable: false}); o.x = 2; o.x").unwrap();
    assert!(r.is_int() && r.as_int() == 1, "sloppy 只读属性写应静默 no-op，值保持 1，实际 {:?}", r);
}

// ── 严格脚本（顶层指令）：只读自身属性赋值抛 TypeError ──
#[test]
fn strict_script_top_level_readonly_throws() {
    let result = eval("'use strict'; var o = {}; Object.defineProperty(o, 'x', {value: 1, writable: false}); o.x = 2");
    assert_err_contains(result, "read-only");
}

// ── sloppy 函数级（非严格函数）：不可扩展对象写新属性静默失败 ──
#[test]
fn sloppy_function_write_new_prop_on_non_extensible_silently_fails() {
    let r = eval_many(&[
        "var o = Object.preventExtensions({})",
        "function w() { o.n = 3 }",
        "w()",
        "o.n === undefined",
    ])
    .unwrap();
    assert!(
        r.is_bool() && r.as_bool(),
        "sloppy 函数写不可扩展对象新属性应静默 no-op，属性不创建，实际 {:?}",
        r
    );
}

// ── 严格函数：不可扩展对象写新属性抛 TypeError ──
#[test]
fn strict_function_write_new_prop_on_non_extensible_throws() {
    let result = eval_many(&["var o = Object.preventExtensions({})", "(function() { 'use strict'; o.n = 3 })()"]);
    assert_err_contains(result, "not extensible");
}

// ── 继承只读数据属性：sloppy 赋值静默失败且不创建自有属性 ──
#[test]
fn sloppy_inherited_readonly_silently_fails_without_shadow() {
    let s = eval_str(
        "var p = {}; Object.defineProperty(p, 'x', {value: 1, writable: false, configurable: true}); \
         var c = Object.create(p); c.x = 9; c.x + ':' + c.hasOwnProperty('x')",
    );
    assert_eq!(s, "1:false", "sloppy 只读继承属性写应静默 no-op，值保持 1 且不遮蔽");
}

// ── 内联回调（forEach）：sloppy 顶层 + sloppy 回调，只读属性写静默失败 ──
#[test]
fn sloppy_inline_callback_write_to_readonly_silently_fails() {
    let r = eval_many(&[
        "var o = {}",
        "Object.defineProperty(o, 'p', {value: 0, writable: false, configurable: true})",
        "[1].forEach(function() { o.p = 1 })",
        "o.p",
    ])
    .unwrap();
    assert!(r.is_int() && r.as_int() == 0, "sloppy 内联回调内只读属性写应静默 no-op，实际 {:?}", r);
}

// ── 内联回调：严格函数上下文（回调继承严格），只读属性写抛 TypeError ──
#[test]
fn strict_inline_callback_write_to_readonly_throws() {
    let r = eval_many(&[
        "var o = {}",
        "Object.defineProperty(o, 'p', {value: 0, writable: false, configurable: true})",
        "var threw = false",
        "(function() { 'use strict'; [1].forEach(function() { try { o.p = 1 } catch (e) { threw = true } }) })()",
        "threw",
    ])
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "严格内联回调内只读属性写应抛 TypeError，实际 {:?}", r);
}

// ── 嵌套内联：sloppy 回调内调用严格动态函数，写只读属性抛 TypeError
//    （严格性随内联快照逐层保存/恢复，内层取内层目标） ──
#[test]
fn nested_inline_strict_target_write_throws() {
    let r = eval_many(&[
        "var o = {}",
        "Object.defineProperty(o, 'p', {value: 0, writable: false, configurable: true})",
        "var strictWrite = new Function('o', \"'use strict'; o.p = 1\")",
        "var threw = false",
        "[1].forEach(function() { try { strictWrite(o) } catch (e) { threw = true } })",
        "threw",
    ])
    .unwrap();
    assert!(
        r.is_bool() && r.as_bool(),
        "嵌套内联中严格目标函数的只读属性写应抛 TypeError，实际 {:?}",
        r
    );
}

// ── 嵌套内联反向：严格回调内调用 sloppy 动态函数，写只读属性静默失败 ──
#[test]
fn nested_inline_sloppy_target_write_silently_fails() {
    let r = eval_many(&[
        "var o = {}",
        "Object.defineProperty(o, 'p', {value: 0, writable: false, configurable: true})",
        "var sloppyWrite = new Function('o', 'o.p = 1')",
        "(function() { 'use strict'; [1].forEach(function() { sloppyWrite(o) }) })()",
        "o.p",
    ])
    .unwrap();
    assert!(
        r.is_int() && r.as_int() == 0,
        "嵌套内联中 sloppy 目标函数的只读属性写应静默 no-op，实际 {:?}",
        r
    );
}

// ── 链式赋值中间对象只读（冻结）：sloppy 对中间对象写新属性静默失败 ──
#[test]
fn sloppy_chained_write_through_readonly_middle_silently_fails() {
    let s = eval_str(
        "var holder = Object.freeze({}); var o = { b: holder }; o.b.c = 1; \
         Object.keys(o.b).length + ':' + (o.b.c === undefined)",
    );
    assert_eq!(s, "0:true", "sloppy 链式中间冻结对象写新属性应静默 no-op，c 不创建");
}

// ── 链式赋值中间对象只读（冻结）：严格模式抛 TypeError ──
#[test]
fn strict_chained_write_through_readonly_middle_throws() {
    let result = eval_many(&[
        "var holder = Object.freeze({})",
        "var o = { b: holder }",
        "(function() { 'use strict'; o.b.c = 1 })()",
    ]);
    assert_err_contains(result, "not extensible");
}

// ── 顶层脚本 sloppy 赋值只读全局属性（undefined）不抛错 ──
#[test]
fn sloppy_top_level_assign_to_readonly_global_no_throw() {
    // 值保持强断言：写被拦截后 typeof 读回 'undefined'，全局对象属性不被修改。
    let r = eval_many(&["undefined = 1", "typeof undefined"]).unwrap();
    assert!(r.is_string(), "sloppy 顶层对只读全局属性赋值不得抛错，实际 {:?}", r);
    let s = eval_str("undefined = 1; typeof undefined");
    assert_eq!(s, "undefined", "sloppy 顶层 undefined 赋值应静默 no-op，typeof 读回 'undefined'");
    let b = eval("undefined = 1; globalThis.undefined === undefined").unwrap();
    assert!(b.is_bool() && b.as_bool(), "全局对象 undefined 属性不应被赋值修改，实际 {:?}", b);
}
