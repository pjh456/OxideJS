//! sloppy 未声明标识符写（隐式全局创建）语义测试：sloppy 未解析引用写在全局对象
//! 建可写/可枚举/可配置数据属性；严格模式同一写抛 ReferenceError（编译期拦截）。
//! 覆盖脚本顶层、函数体（全局对象经 session 解析，不依赖 this）、解构目标、
//! 更新式/逻辑赋值、for-in/for-of 左侧与不可扩展全局对象分支。

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
    let result = vm.run(&Arc::new(module)).expect("run");
    vm.lookup_str(result).unwrap_or_else(|| format!("{result}"))
}

// ── sloppy 顶层未声明写：全局对象新建可写/可枚举/可配置数据属性 ──
#[test]
fn sloppy_undeclared_write_creates_global_property() {
    let r = eval_many(&["x = 1", "globalThis.x === 1"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "sloppy 未声明写应建全局属性，实际 {:?}", r);
}

#[test]
fn sloppy_undeclared_write_descriptor_writable_enumerable_configurable() {
    let s = eval_str(
        "y = 2; var d = Object.getOwnPropertyDescriptor(globalThis, 'y'); \
         d.writable + ':' + d.enumerable + ':' + d.configurable",
    );
    assert_eq!(s, "true:true:true", "隐式全局属性描述符应全 true，实际 {s}");
}

// ── 可配置：隐式全局属性经 Object.defineProperty 可再描述（var 绑定不可配置） ──
#[test]
fn sloppy_undeclared_write_property_reconfigurable() {
    let r =
        eval_many(&["x = 1", "Object.defineProperty(globalThis, 'x', { value: 9 })", "globalThis.x === 9"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "可配置隐式全局属性应可再定义，实际 {:?}", r);
}

// ── 函数体内未声明写：全局对象经 session 解析（不依赖 this） ──
#[test]
fn sloppy_function_body_undeclared_write_lands_on_global() {
    let r = eval_many(&["function f() { x = 7 }", "f()", "globalThis.x === 7"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "函数体内未声明写应落全局对象，实际 {:?}", r);
}

// ── 解构赋值目标：未声明标识符走同一隐式全局路径 ──
#[test]
fn sloppy_destructuring_undeclared_write_lands_on_global() {
    let r = eval_many(&["({ q } = { q: 5 })", "globalThis.q === 5"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "解构未声明目标应落全局对象，实际 {:?}", r);
}

// ── 更新式赋值：未声明名 RMW 旧值 undefined，落全局 NaN（undefined + 1） ──
#[test]
fn sloppy_update_operator_undeclared_write_lands_on_global() {
    let r = eval_many(&["z++", "Number.isNaN(globalThis.z)"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "z++ 未声明写应落全局 NaN（undefined+1），实际 {:?}", r);
}

#[test]
fn sloppy_compound_operator_undeclared_write_is_nan() {
    let s = eval_str("w += 1; typeof w");
    assert_eq!(s, "number", "w += 1 未声明名应为 NaN（typeof number），实际 {s}");
}

// ── 逻辑赋值：未声明名 ||= 短路建全局 ──
#[test]
fn sloppy_logical_assign_undeclared_write_lands_on_global() {
    let r = eval_many(&["p ||= 5", "globalThis.p === 5"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "p ||= 5 未声明写应落全局为 5，实际 {:?}", r);
}

// ── for-in 左侧未声明标识符：首个键后建全局 ──
#[test]
fn sloppy_for_in_undeclared_write_lands_on_global() {
    let r = eval_many(&["for (k in { a: 1 }) {}", "globalThis.k === 'a'"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "for-in 未声明左侧应落全局为 'a'，实际 {:?}", r);
}

// ── for-of 左侧未声明标识符：末次迭代值落全局 ──
#[test]
fn sloppy_for_of_undeclared_write_lands_on_global() {
    let r = eval_many(&["for (v of [1, 2]) {}", "globalThis.v === 2"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "for-of 未声明左侧应落全局为 2，实际 {:?}", r);
}

// ── 严格模式：顶层未声明写抛 ReferenceError（消息与读侧一致） ──
#[test]
fn strict_top_level_undeclared_write_throws() {
    let result = eval("'use strict'; x = 1");
    assert_err_contains(result, "x is not defined");
}

// ── 严格模式：函数体内未声明写抛 ReferenceError ──
#[test]
fn strict_function_body_undeclared_write_throws() {
    let result = eval("function f() { 'use strict'; y = 2 }; f()");
    assert_err_contains(result, "y is not defined");
}

// ── 严格 for-in：空对象无键不抛（PutValue 仅首键后发生） ──
#[test]
fn strict_for_in_empty_object_does_not_throw() {
    let r = eval("'use strict'; for (k in {}) {}; true").unwrap();
    assert!(r.is_bool() && r.as_bool(), "严格空 for-in 不应抛错，实际 {:?}", r);
}

// ── 严格 for-of：空迭代器不抛 ──
#[test]
fn strict_for_of_empty_does_not_throw() {
    let r = eval("'use strict'; for (v of []) {}; true").unwrap();
    assert!(r.is_bool() && r.as_bool(), "严格空 for-of 不应抛错，实际 {:?}", r);
}

// ── 全局对象不可扩展 + 属性缺失：sloppy 未声明写抛 TypeError ──
#[test]
fn sloppy_undeclared_write_on_non_extensible_global_throws() {
    let result = eval("Object.preventExtensions(globalThis); t = 1");
    assert_err_contains(result, "not extensible");
}

// ── 全局对象不可扩展 + 属性缺失：严格未声明写先抛 ReferenceError ──
#[test]
fn strict_undeclared_write_on_non_extensible_global_throws_reference() {
    let result = eval("'use strict'; Object.preventExtensions(globalThis); t = 1");
    assert_err_contains(result, "t is not defined");
}

// ── 既有不可写非可配置全局属性：sloppy 未声明写静默 no-op 不抛错 ──
#[test]
fn sloppy_undeclared_write_to_readonly_global_property_silent() {
    let r = eval_many(&[
        "Object.defineProperty(globalThis, 'gw', { value: 0, writable: false, configurable: false })",
        "gw = 1",
        "globalThis.gw === 0",
    ])
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "sloppy 对既有不可写全局属性写应静默 no-op，实际 {:?}", r);
}

// ── 读写混合解析：未声明名由首次引用侧登记全局槽（读侧 LOAD_GLOBAL 登记，
// 写侧新建登记），后的写须同样穿透全局对象——先读后写各形态回归钉 ──
#[test]
fn undeclared_read_then_assignment_write_reaches_global() {
    let r = eval_many(&["globalThis.n = 0", "var seen = n", "n = 7", "globalThis.n === 7"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "先读后写的未声明名应穿透全局对象，实际 {:?}", r);
}

#[test]
fn undeclared_read_then_update_write_reaches_global() {
    let r = eval_many(&["globalThis.c = 0", "var seen = c", "c++", "globalThis.c === 1"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "先读后更新式写应穿透全局对象，实际 {:?}", r);
}

#[test]
fn undeclared_read_then_compound_write_reaches_global() {
    let r = eval_many(&["globalThis.m = 0", "var seen = m", "m += 2", "globalThis.m === 2"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "先读后复合赋值应穿透全局对象，实际 {:?}", r);
}

#[test]
fn undeclared_read_then_destructuring_write_reaches_global() {
    let r = eval_many(&["globalThis.d = 0", "var seen = d", "[d] = [9]", "globalThis.d === 9"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "先读后解构写应穿透全局对象，实际 {:?}", r);
}

// ── for-in/for-of 左侧先读后写：全局槽由读侧登记，LHS 写同样须穿透全局对象 ──
#[test]
fn undeclared_read_then_for_in_write_reaches_global() {
    let r = eval_many(&["globalThis.k = 0", "var seen = k", "for (k in { a: 1 }) {}", "globalThis.k === 'a'"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "先读后 for-in 左侧写应穿透全局对象，实际 {:?}", r);
}

#[test]
fn undeclared_read_then_for_of_write_reaches_global() {
    let r = eval_many(&["globalThis.v = 0", "var seen = v", "for (v of [1, 2]) {}", "globalThis.v === 2"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "先读后 for-of 左侧写应穿透全局对象，实际 {:?}", r);
}

// ── 严格模式：读侧已登记的全局槽同属未解析引用，后写抛 ReferenceError ──
#[test]
fn strict_undeclared_read_then_write_throws_reference() {
    let result = eval("'use strict'; globalThis.s = 0; var seen = s; s = 5");
    assert_err_contains(result, "s is not defined");
}

#[test]
fn strict_undeclared_read_then_for_in_write_throws_reference() {
    let result = eval("'use strict'; globalThis.k = 0; var seen = k; for (k in { a: 1 }) {}");
    assert_err_contains(result, "k is not defined");
}
