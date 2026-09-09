//! 不可写全局内置（undefined/NaN/Infinity）写路径拦截测试：写命中全局不可写
//! 内置绑定时编译期拦截——sloppy 静默 no-op（后续读回原值，全局对象不变），
//! strict 抛 TypeError。覆盖简单/复合/更新赋值、声明（var 无初始化与带初始化）、
//! 解构赋值、for-in/for-of 标识符目标与嵌套函数形态；局部遮蔽绑定（var/let/
//! 参数）与其他可写内置（Math）不受影响。

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

// ── sloppy 顶层：三个不可写内置赋值后读回原值（静默 no-op） ──
#[test]
fn sloppy_assign_undefined_silently_keeps_value() {
    let s = eval_str("undefined = 5; '' + undefined");
    assert_eq!(s, "undefined", "sloppy 顶层 undefined 赋值应静默 no-op，读回原值");
}

#[test]
fn sloppy_assign_nan_silently_keeps_value() {
    let s = eval_str("NaN = 1; '' + NaN");
    assert_eq!(s, "NaN", "sloppy 顶层 NaN 赋值应静默 no-op，读回原值");
}

#[test]
fn sloppy_assign_infinity_silently_keeps_value() {
    let s = eval_str("Infinity = 2; '' + Infinity");
    assert_eq!(s, "Infinity", "sloppy 顶层 Infinity 赋值应静默 no-op，读回原值");
}

// ── strict 顶层：赋值抛 TypeError（不可写属性 put 失败） ──
#[test]
fn strict_assign_undefined_throws_type_error() {
    let result = eval("'use strict'; undefined = 5");
    assert_err_contains(result, "read-only");
}

#[test]
fn strict_assign_nan_throws_type_error() {
    let result = eval("'use strict'; NaN = 1");
    assert_err_contains(result, "read-only");
}

// ── 可写内置（Math）重定义放行：不得被过度拦截 ──
#[test]
fn writable_builtin_reassignment_still_works() {
    let r = eval("Math = 5; Math === 5").unwrap();
    assert!(r.is_bool() && r.as_bool(), "Math 可写属性重定义应保持可用，实际 {:?}", r);
}

// ── var 无初始化：声明不抹槽值（保留内置原值） ──
#[test]
fn var_no_init_undefined_keeps_value() {
    let s = eval_str("var undefined; '' + undefined");
    assert_eq!(s, "undefined", "顶层 var undefined 无初始化应保留原值");
}

#[test]
fn var_no_init_nan_keeps_value() {
    let s = eval_str("var NaN; '' + NaN");
    assert_eq!(s, "NaN", "顶层 var NaN 无初始化应保留原值");
}

// ── 带初始化 var 撞全局不可写内置：sloppy 静默、strict 抛错 ──
#[test]
fn var_with_init_sloppy_silently_keeps_value() {
    let s = eval_str("var undefined = 3; '' + undefined");
    assert_eq!(s, "undefined", "sloppy var undefined = 3 应静默 no-op，读回原值");
}

#[test]
fn var_with_init_strict_throws_type_error() {
    let result = eval("'use strict'; var undefined = 3");
    assert_err_contains(result, "read-only");
}

// ── 函数内无遮蔽写：命中继承的全局槽，sloppy 静默、strict 抛错 ──
#[test]
fn function_scoped_write_silently_keeps_value() {
    let s = eval_str("function g(){ undefined = 1; return undefined; } '' + g()");
    assert_eq!(s, "undefined", "函数内未遮蔽的 undefined 写应静默 no-op，读回原值");
}

#[test]
fn function_scoped_write_strict_throws_type_error() {
    let r = eval_many(&[
        "var threw = false",
        "function g(){ 'use strict'; try { undefined = 1 } catch (e) { threw = true } }",
        "g()",
        "threw",
    ])
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "严格函数内 undefined 写应抛 TypeError，实际 {:?}", r);
}

// ── 全局对象属性不变：寄存器镜像写不得穿透到 global object ──
#[test]
fn global_object_property_untouched() {
    let r = eval("undefined = 5; globalThis.undefined === undefined").unwrap();
    assert!(r.is_bool() && r.as_bool(), "寄存器镜像写不应修改全局对象属性，实际 {:?}", r);
}

// ── 局部遮蔽绑定保持可写（var/参数同名不命中全局拦截） ──
#[test]
fn local_var_shadow_writable() {
    let r = eval("function g(){ var undefined = 3; undefined = 5; return undefined; } g()").unwrap();
    assert!(r.is_int() && r.as_int() == 5, "函数内 var undefined 局部遮蔽应可写，实际 {:?}", r);
}

#[test]
fn param_shadow_writable() {
    let r = eval("function g(undefined){ undefined = 1; return undefined; } g(9)").unwrap();
    assert!(r.is_int() && r.as_int() == 1, "参数名 undefined 局部遮蔽应可写，实际 {:?}", r);
}

// ── 复合/更新赋值：槽值保留，表达式值按规范计算 ──
#[test]
fn compound_assign_keeps_value() {
    let s = eval_str("undefined += 1; '' + undefined");
    assert_eq!(s, "undefined", "sloppy undefined += 1 应静默 no-op，读回原值");
}

#[test]
fn update_assign_keeps_value() {
    let s = eval_str("undefined++; '' + undefined");
    assert_eq!(s, "undefined", "sloppy undefined++ 应静默 no-op，读回原值");
}

#[test]
fn postfix_update_returns_old_value() {
    let r = eval("var v = Infinity--; isFinite(v)").unwrap();
    assert!(r.is_bool() && !r.as_bool(), "Infinity-- 后缀形式应返回旧值 Infinity，实际 {:?}", r);
}

// ── 解构赋值目标：命中项静默跳过，strict 抛错 ──
#[test]
fn destructure_assign_keeps_value() {
    let s = eval_str("({undefined} = {undefined: 9}); '' + undefined");
    assert_eq!(s, "undefined", "sloppy 解构赋值命中 undefined 应静默 no-op，读回原值");
}

#[test]
fn destructure_assign_strict_throws_type_error() {
    let result = eval("'use strict'; ({undefined} = {undefined: 9})");
    assert_err_contains(result, "read-only");
}

// ── for-in/for-of 标识符目标：迭代写不污染槽值 ──
#[test]
fn for_of_target_keeps_value() {
    let s = eval_str("for (undefined of [1, 2]) {} '' + undefined");
    assert_eq!(s, "undefined", "sloppy for-of 目标 undefined 应静默 no-op，读回原值");
}

#[test]
fn for_of_target_strict_throws_type_error() {
    let result = eval("'use strict'; for (undefined of [1]) {}");
    assert_err_contains(result, "read-only");
}

#[test]
fn for_of_target_strict_empty_no_throw() {
    let r = eval("'use strict'; for (undefined of []) {} true").unwrap();
    assert!(r.is_bool() && r.as_bool(), "空集合 for-of 不发生 put，strict 不应抛错，实际 {:?}", r);
}

// ── for 头 var 无初始化：不抹预载槽值（NaN 保留） ──
#[test]
fn for_header_var_no_init_keeps_slot_value() {
    let s = eval_str("for (var NaN; false;) {} '' + NaN");
    assert_eq!(s, "NaN", "for 头 var NaN 无初始化应保留槽值");
}
