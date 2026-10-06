//! delete 已知 builtin 名后的复合/逻辑赋值与 update 表达式 LHS 旧值读：
//! 旧值读须经 A 侧全局对象属性在位判定，缺位（delete 真删后）抛
//! ReferenceError，与裸读同形；在位时语义不变（A 侧读 + 写回）。
//!
//! 镜像槽无法自区分「A 侧缺位」与「A 侧在位值 undefined」（两者槽值皆
//! undefined），delete 真删后旧值读走镜像槽会不抛且以计算值复活属性。
//! 本面只改旧值读路由（三读点前插 LOAD_GLOBAL），写回路径零改动。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval_js(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("parse: {e:?}"))?;
    let module = Arc::new(Compiler::new().compile(&program).map_err(|e| format!("compile: {e}"))?);
    let mut vm = Vm::new();
    let val = vm.run(&module)?;
    Ok((vm, val))
}

fn eval_str(source: &str) -> Result<String, String> {
    let (vm, val) = eval_js(source)?;
    Ok(vm.lookup_str(val).unwrap_or_else(|| format!("{val:?}")))
}

fn eval_truthy(source: &str) {
    let (_vm, val) = eval_js(source).expect("run");
    assert!(val.is_bool() && val.as_bool(), "expected true, got: {val:?}\nsource: {source}");
}

/// 删后 LHS 旧值读抛 ReferenceError 钉：运行期错误文本须含种类与名字。
fn expect_re(source: &str, name: &str) {
    let err = match eval_js(source) {
        Err(e) => e,
        Ok(_) => panic!("expected ReferenceError, got ok: {source}"),
    };
    let lower = err.to_lowercase();
    assert!(lower.contains("referenceerror"), "expected ReferenceError, got: {err}\nsource: {source}");
    assert!(lower.contains(name), "expected name in message, got: {err}\nsource: {source}");
}

// ── 红族：删后 LHS 旧值读抛 ReferenceError（修前不抛且以计算值复活属性）──

// 复合主形态：顶层 delete 后复合赋值旧值读。
#[test]
fn delete_array_compound_add_reference_error() {
    expect_re("delete Array; Array += 1", "array");
}

// 复合换算符：减。
#[test]
fn delete_array_compound_sub_reference_error() {
    expect_re("delete Array; Array -= 1", "array");
}

// 复合换算符：乘。
#[test]
fn delete_array_compound_mul_reference_error() {
    expect_re("delete Array; Array *= 2", "array");
}

// 逻辑主形态：||= 旧值读。
#[test]
fn delete_array_logical_or_reference_error() {
    expect_re("delete Array; Array ||= 2", "array");
}

// 逻辑短路面：&&= 旧值读（短路测试前抛）。
#[test]
fn delete_array_logical_and_reference_error() {
    expect_re("delete Array; Array &&= 2", "array");
}

// 逻辑空值面：??= 旧值读。
#[test]
fn delete_array_logical_nullish_reference_error() {
    expect_re("delete Array; Array ??= 2", "array");
}

// update 前缀主形态。
#[test]
fn delete_array_update_prefix_increment_reference_error() {
    expect_re("delete Array; Array++", "array");
}

// update 前缀换算符：递减。
#[test]
fn delete_array_update_prefix_decrement_reference_error() {
    expect_re("delete Array; --Array", "array");
}

// update 后缀面。
#[test]
fn delete_array_update_postfix_reference_error() {
    expect_re("delete Array; Array--", "array");
}

// 换名族：Promise 单名复合。
#[test]
fn delete_promise_compound_reference_error() {
    expect_re("delete Promise; Promise += 1", "promise");
}

// 帧读面：delete 在外层，嵌套函数内复合赋值旧值读缺失。
#[test]
fn delete_array_nested_fn_compound_reference_error() {
    expect_re("delete Array; (function(){Array += 1})()", "array");
}

// 成员复合守卫钉：成员面旧值读走对象属性（非镜像槽），零涉本面；
// 基引用 Math 裸读经 221.1 路由抛 ReferenceError，成员复合路径不改变该抛点。
#[test]
fn delete_math_member_compound_base_read_reference_error() {
    expect_re("delete Math; Math.max += 1", "math");
}

// with 复合回退面：with 对象无该属性时回退静态复合路径，旧值读缺失抛。
#[test]
fn delete_array_with_compound_fallback_reference_error() {
    expect_re("delete Array; with(globalThis){Array += 1}", "array");
}

// with update 回退面：同复合回退面，update 表达式。
#[test]
fn delete_array_with_update_fallback_reference_error() {
    expect_re("delete Array; with(globalThis){Array++}", "array");
}

// with 逻辑赋值无对象回退：with 体自由标识符逻辑赋值走静态路径，旧值读缺失抛。
#[test]
fn delete_array_with_logical_fallback_reference_error() {
    expect_re("delete Array; with({x: 1}){Array ||= 2}", "array");
}

// ── 绿族：修后须保真 ──

// 未删在位复合：A 侧读 + 写回，语义不变。
#[test]
fn builtin_compound_add_present_value() {
    assert_eq!(eval_str("Array = 1; Array += 1; String(Array)").unwrap(), "2");
}

// 局部 var 零涉：非 builtin 名复合赋值。
#[test]
fn local_var_compound_add_value() {
    assert_eq!(eval_str("var x = 1; x += 1; String(x)").unwrap(), "2");
}

// 遮蔽守卫：局部 var Array 遮蔽 builtin 槽，复合走局部槽。
#[test]
fn local_var_shadow_array_compound_value() {
    assert_eq!(eval_str("function g(){var Array=1; Array += 1; return Array}; String(g())").unwrap(), "2",);
}

// 只读三常量零涉：c:false 拒删保值，复合旧值读不抛。
#[test]
fn delete_nan_compound_no_throw() {
    eval_truthy("delete NaN === false && Number.isNaN(NaN += 1)");
}

// 删后重写再复合：A 侧在位，旧值读见新值。
#[test]
fn delete_array_rewrite_compound_value() {
    assert_eq!(eval_str("delete Array; Array = 42; Array += 1; String(Array)").unwrap(), "43");
}

// 成员面零涉：对象属性 delete 后成员复合不抛（GetBaseValue 缺位按 undefined）。
#[test]
fn member_compound_after_delete_no_throw() {
    eval_truthy("var obj = {p: 1}; delete obj.p; obj.p += 1; Number.isNaN(obj.p)");
}

// 221.1 保真：typeof 不抛。
#[test]
fn delete_array_typeof_undefined() {
    assert_eq!(eval_str("delete Array; typeof Array").unwrap(), "undefined");
}

// 221.1 保真：裸读抛 ReferenceError。
#[test]
fn delete_array_bare_read_reference_error() {
    expect_re("delete Array; Array", "array");
}
