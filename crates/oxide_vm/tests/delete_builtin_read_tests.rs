//! delete 已知 builtin 名后的读侧路由：裸读/typeof/调用/new/成员基/with 读/
//! eval 读须经 A 侧全局对象属性在位判定。
//!
//! 镜像槽无法自区分「A 侧缺位」与「A 侧在位值 undefined」（两者槽值皆
//! undefined），故 delete 真删后读缺位抛 ReferenceError、typeof 求值
//! "undefined"；在位 undefined 不抛（判别钉 U1）。删除侧（裸名/成员形/
//! with 形/Reflect）已收口，本面零改动，仅读路由。

use std::sync::Arc;

use oxide_bytecode::module::CompiledModule;
use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn compile_module(source: &str) -> Arc<CompiledModule> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    Arc::new(Compiler::new().compile(&program).expect("compile"))
}

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

/// 删后读抛 ReferenceError 钉：运行期错误文本须含种类与名字。
fn expect_re(source: &str, name: &str) {
    let err = match eval_js(source) {
        Err(e) => e,
        Ok(_) => panic!("expected ReferenceError, got ok: {source}"),
    };
    let lower = err.to_lowercase();
    assert!(lower.contains("referenceerror"), "expected ReferenceError, got: {err}\nsource: {source}");
    assert!(lower.contains(name), "expected name in message, got: {err}\nsource: {source}");
}

// ── 红族：删后读抛 ReferenceError（修前全部回 undefined/TypeError 种类错）──

// 主形态：顶层 delete 后裸读。
#[test]
fn delete_array_bare_read_reference_error() {
    expect_re("delete Array; Array", "array");
}

// 帧读面：删后嵌套函数读外层 builtin 名。
#[test]
fn delete_array_nested_fn_read_reference_error() {
    expect_re("delete Array; (function(){return Array})()", "array");
}

// 帧内删/外层读：delete 在帧内真删，外层裸读可见缺失。
#[test]
fn delete_array_in_fn_outer_read_reference_error() {
    expect_re("(function(){delete Array;return 1})(); Array", "array");
}

// eval 面：eval 程序读 A 侧缺失名。
#[test]
fn delete_array_eval_read_reference_error() {
    expect_re("delete Array; eval(\"Array\")", "array");
}

// 种类面：删后调用应抛 ReferenceError 而非 TypeError（修前 CALL_NATIVE 种类错）。
#[test]
fn delete_array_call_reference_error_not_type_error() {
    expect_re("delete Array; Array()", "array");
}

// 种类面：删后构造同抛 ReferenceError（修前 NEW_EXPRESSION 种类错）。
#[test]
fn delete_array_new_expression_reference_error() {
    expect_re("delete Array; new Array()", "array");
}

// 换名族：Promise 单名。
#[test]
fn delete_promise_bare_read_reference_error() {
    expect_re("delete Promise; Promise", "promise");
}

// 换名族：Math 成员基形态。
#[test]
fn delete_math_member_base_reference_error() {
    expect_re("delete Math; Math.max(1, 2)", "math");
}

// 换名族：Object 主形态。
#[test]
fn delete_object_bare_read_reference_error() {
    expect_re("delete Object; Object", "object");
}

// with 读面：with 对象有该属性命中对象读，缺失回退 A 侧。
#[test]
fn delete_array_with_read_reference_error() {
    expect_re("delete Array; with(globalThis){Array}", "array");
}

// with 删面：with 对象属性经 DELETE_PROP_DYNAMIC 真删后外层读缺失。
#[test]
fn delete_array_with_delete_then_read_reference_error() {
    expect_re("with(globalThis){delete Array} Array", "array");
}

// Reflect 面：Reflect.deleteProperty 真删后裸读缺失。
#[test]
fn delete_array_reflect_then_read_reference_error() {
    expect_re("Reflect.deleteProperty(globalThis, \"Array\"); Array", "array");
}

// 重删后读：删→重写→再删，读仍缺失。
#[test]
fn delete_array_rewrite_redelete_read_reference_error() {
    expect_re("delete Array; Array = 42; delete Array; Array", "array");
}

// 可选链面：?. 基引用解析抛 ReferenceError。
#[test]
fn delete_array_optional_chain_reference_error() {
    expect_re("delete Array; Array?.length", "array");
}

// in LHS 面：in 操作数按引用解析抛 ReferenceError。
#[test]
fn delete_array_in_lhs_reference_error() {
    expect_re("delete Array; Array in globalThis", "array");
}

// 比较操作数面：同根主形态的另一钉。
#[test]
fn delete_array_comparison_operand_reference_error() {
    expect_re("delete Array; Array === undefined", "array");
}

// 跨 run 面：同一 VM 两轮（run 入口重载镜像槽 + A 侧读），第二轮裸读缺失。
#[test]
fn delete_array_across_runs_reference_error() {
    let (mut vm, _first) = eval_js("delete Array").expect("first run should succeed");
    let module = compile_module("Array");
    let err = vm.run(&module).expect_err("expected ReferenceError");
    assert!(err.to_lowercase().contains("referenceerror"), "expected ReferenceError, got: {err}");
}

// ── 绿族：修后须保真（17 钉）──

// 严格模式 delete 标识符是早期错误（解析期，双侧同文）。
#[test]
fn strict_delete_unqualified_syntax_error() {
    let allocator = Allocator::default();
    let res = oxide_parser::parse(&allocator, "\"use strict\"; function f() { delete Array; }");
    let err = res.expect_err("expected parse-time SyntaxError");
    let text = format!("{err:?}").to_lowercase();
    assert!(text.contains("strict"), "expected strict-mode syntax error, got: {err:?}");
}

// typeof 面：删后 typeof 求值 "undefined"（不抛）。
#[test]
fn delete_array_typeof_undefined() {
    assert_eq!(eval_str("delete Array; typeof Array").unwrap(), "undefined");
}

// typeof 帧面：删后嵌套函数内 typeof 同 "undefined"。
#[test]
fn delete_array_nested_typeof_undefined() {
    assert_eq!(eval_str("delete Array; (function(){return typeof Array})()").unwrap(), "undefined");
}

// in 面：in 走 A 侧，删后 false。
#[test]
fn delete_array_in_global_this_false() {
    eval_truthy("delete Array; !(\"Array\" in globalThis)");
}

// 二次 delete：属性缺失仍返 true。
#[test]
fn delete_array_second_delete_true() {
    eval_truthy("delete Array; delete Array");
}

// 反射面：globalThis.Array 成员读回 undefined。
#[test]
fn delete_array_global_this_reflection_undefined() {
    eval_truthy("delete Array; globalThis.Array === undefined");
}

// 删后重写：DEFINE 缺失分支新建 c:true 属性，裸读见新值。
#[test]
fn delete_array_rewrite_reads_new_value() {
    assert_eq!(eval_str("delete Array; Array = 42; String(Array)").unwrap(), "42");
}

// 三常量 c:false：拒删且保值。
#[test]
fn delete_three_constants_refused_values_kept() {
    eval_truthy(
        "delete NaN === false && delete Infinity === false \
         && String(NaN) === \"NaN\" && Number.isNaN(NaN) && !Number.isFinite(Infinity)",
    );
}

// 顶层 var c:false：拒删保值。
#[test]
fn delete_top_level_var_refused_value_kept() {
    assert_eq!(eval_str("var x = 1; delete x; String(x)").unwrap(), "1");
}

// 未声明名面（既有绿）：删后读抛 ReferenceError。
#[test]
fn delete_undeclared_name_bare_read_reference_error() {
    expect_re("globalThis.y = 1; delete globalThis.y; y", "y");
}

// typeof 换名族：JSON 多名。
#[test]
fn delete_json_typeof_undefined() {
    assert_eq!(eval_str("delete JSON; typeof JSON").unwrap(), "undefined");
}

// 遮蔽守卫：局部 var Array 遮蔽 builtin 槽，遮蔽读不受路由影响。
#[test]
fn local_var_shadow_array_read_value() {
    assert_eq!(eval_str("function g(){var Array=1;return Array}; String(g())").unwrap(), "1");
}

// 遮蔽 typeof 守卫。
#[test]
fn local_var_shadow_array_typeof_number() {
    assert_eq!(eval_str("function g(){var Array=1;return typeof Array}; g()").unwrap(), "number");
}

// globalThis c:true 族：typeof 预删前形态。
#[test]
fn global_this_typeof_object() {
    assert_eq!(eval_str("typeof globalThis").unwrap(), "object");
}

// globalThis 真删返 true。
#[test]
fn delete_global_this_returns_true() {
    eval_truthy("delete globalThis");
}

// globalThis 删后 typeof 求值 "undefined"。
#[test]
fn delete_global_this_typeof_undefined() {
    assert_eq!(eval_str("delete globalThis; typeof globalThis").unwrap(), "undefined");
}

// 关键判别钉：在位 undefined 不抛（防「槽==undefined 即抛」误修）。
#[test]
fn builtin_assigned_undefined_bare_read_no_throw() {
    eval_truthy("Array = undefined; Array === undefined");
}
