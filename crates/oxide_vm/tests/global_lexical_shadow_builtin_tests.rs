//! 顶层/模块词法声明遮蔽可配置全局内置名的合法化：let/const/class 声明名撞
//! 非受限 builtin 名（Math/Array/isNaN/Promise 等可配置全局属性）时，镜像槽
//! 不为其登记，声明合法建立遮蔽绑定——裸读读绑定槽、裸写/delete 不落全局
//! 对象（全局属性原值保留），声明点前裸读按 TDZ 运行期抛 ReferenceError。
//!
//! 回归约束：受限三常量（87 面）声明仍编译期拒绝；let+var / function+let
//! 撞 builtin 名的重复声明仍编译期拒绝（var 懒登记镜像与函数先登记撞点保留）；
//! 未遮蔽 builtin 读、var 内置名写全局、函数内遮蔽读不回退。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_emit::module::{ModuleSourceLoader, ResolvedModule};
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

fn eval_str(source: &str) -> Result<String, String> {
    let value = eval(source)?;
    if value.is_string() {
        // SAFETY: 值已确认字符串类别，指针指向 VM 存活期内的 JsString。
        Ok(unsafe { &*value.as_string_ptr() }.as_str().to_string())
    } else {
        Err(format!("expected string, got {value:?}"))
    }
}

/// 断言整程序编译失败且错消息为重复声明错形（声明实例化期拒绝）。
fn assert_compile_err(source: &str) {
    let err = eval(source).unwrap_err();
    assert!(err.contains("already been declared"), "got: {err}");
}

/// 断言编译 + 运行均成功，返回完成值。
fn assert_ok(source: &str) -> JsValue {
    eval(source).unwrap_or_else(|e| panic!("expected ok, got: {e}\nsource: {source}"))
}

struct Loader;

impl ModuleSourceLoader for Loader {
    fn resolve(
        &mut self, _base_dir: &str, _specifier: &str, _attributes: &[(&str, &str)],
    ) -> Result<ResolvedModule, String> {
        Err("no deps".into())
    }
}

fn eval_module(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse_module(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new()
        .compile_module(&program, "test.mjs", &mut Loader)
        .map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

// ── 声明形 × builtin 名矩阵：编译过 + 读回声明值（修前全格编译期拒） ──

#[test]
fn top_level_let_math_shadow() {
    let r = assert_ok("let Math=1; Math");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_const_math_shadow() {
    let r = assert_ok("const Math=1; Math");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_class_math_shadow() {
    assert_eq!(eval_str("class Math{} typeof Math").unwrap(), "function");
}

#[test]
fn top_level_let_isnan_shadow() {
    // isNaN 是可配置全局函数名（非受限三常量），合法遮蔽。
    let r = assert_ok("let isNaN=5; isNaN");
    assert_eq!(r.as_int(), 5);
}

#[test]
fn top_level_const_array_shadow() {
    // 语料钉 decl-lex-configurable-global.js 同形：可配置全局属性被遮蔽，
    // 全局对象属性原值保留。
    let r = assert_ok("const Array=7; Array");
    assert_eq!(r.as_int(), 7);
}

#[test]
fn top_level_class_promise_shadow() {
    assert_eq!(eval_str("class Promise{} typeof Promise").unwrap(), "function");
}

#[test]
fn top_level_let_destructured_builtin_shadow() {
    // 解构叶子同属声明名，排除集覆盖。
    let r = assert_ok("let {0: Math} = [3]; Math");
    assert_eq!(r.as_int(), 3);
}

// ── 写臂 / delete 臂：值落绑定槽，全局对象属性不动 ──

#[test]
fn top_level_let_shadow_write_not_global() {
    // 裸写与复合写都只更新绑定槽；globalThis.Math 保留原对象。
    let r = assert_ok("let Math=1; Math=2; Math");
    assert_eq!(r.as_int(), 2);
    let r = assert_ok("let Math=1; Math+=1; Math");
    assert_eq!(r.as_int(), 2);
    assert_eq!(eval_str("let Math=1; Math=2; typeof globalThis.Math").unwrap(), "object");
}

#[test]
fn top_level_let_shadow_delete_not_global() {
    // 词法绑定 delete 返 false（DeleteBinding 非属性引用），全局属性原值保留。
    let r = assert_ok("let Math=1; delete Math");
    assert!(r.is_bool() && !r.as_bool(), "实际 {r:?}");
    let r = assert_ok("let Math=1; delete Math; Math");
    assert_eq!(r.as_int(), 1);
    assert_eq!(eval_str("let Math=1; delete Math; typeof globalThis.Math").unwrap(), "object");
}

#[test]
fn nested_fn_write_top_level_let_shadow() {
    // 嵌套函数写顶层词法绑定经 upvalue cell，同样不落全局对象。
    let r = assert_ok("let Math=1; (function(){Math=2})(); Math");
    assert_eq!(r.as_int(), 2);
    assert_eq!(eval_str("let Math=1; (function(){Math=2})(); typeof globalThis.Math").unwrap(), "object");
}

// ── TDZ 相位：声明点前裸读是运行期 ReferenceError（修前编译期拒） ──

#[test]
fn top_level_let_shadow_tdz_read_throws() {
    let err = eval("Math; let Math=1;").unwrap_err();
    assert!(err.contains("before initialization"), "got: {err}");
}

#[test]
fn top_level_let_shadow_tdz_second_read_throws() {
    let err = eval("Math; let Math=1; Math").unwrap_err();
    assert!(err.contains("before initialization"), "got: {err}");
}

// ── 回归约束：受限名与重复声明拒面不翻 ──

#[test]
fn top_level_let_restricted_names_still_rejected() {
    // 受限三常量（87 面）：非受限名修面不动此面，声明仍编译期拒绝。
    assert_compile_err("let undefined;");
    assert_compile_err("let NaN=1; NaN");
}

#[test]
fn let_then_var_builtin_name_still_rejected() {
    // var 预声明懒登记镜像：排除集只挡预扫描臂，var 臂撞点保留。
    assert_compile_err("let Math=1; var Math;");
}

#[test]
fn function_then_let_builtin_name_still_rejected() {
    // 函数声明先登记（提升 var 绑定）：撞点保留。
    assert_compile_err("function Math(){} let Math=1;");
}

// ── 绿格回归：遮蔽不得回退未遮蔽面 ──

#[test]
fn closure_reads_top_level_let_shadow() {
    let r = assert_ok("let Math=1; (function(){return Math})()");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn let_shadow_without_reference_compiles() {
    let r = assert_ok("let Math=1; 42");
    assert_eq!(r.as_int(), 42);
}

#[test]
fn fn_decl_before_let_shadow_reads_via_capture() {
    let r = assert_ok("function f(){return Math} let Math=1; f()");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn var_builtin_name_local_read() {
    // var 内置名的本地读值不受本修影响（var 臂不置 lexical 标志）。
    let r = assert_ok("var Math=1; Math");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn unshadowed_builtin_read_unchanged() {
    // 一元负号经 ToNumber 落 double 标签，断言按数值比。
    let r = assert_ok("let x=1; Math.abs(-1)");
    assert_eq!(r.as_double(), 1.0);
    let r = assert_ok("let Math=1; parseInt('42')");
    assert_eq!(r.as_int(), 42);
}

// ── 模块面：同形声明合法化，全局对象属性不动 ──

#[test]
fn module_let_math_shadow_runs() {
    let r = eval_module("let Math=1; Math;").expect("module compile+run");
    assert!(r.is_undefined(), "顶层模块完成值应为 undefined，实际 {r:?}");
}

#[test]
fn module_let_shadow_keeps_global() {
    let r = eval_module("let Math=1; Math=2; Math;").expect("module compile+run");
    assert!(r.is_undefined(), "实际 {r:?}");
    assert_eq!(eval_str("typeof globalThis.Math").unwrap(), "object");
}

#[test]
fn module_const_array_shadow_runs() {
    let r = eval_module("const Array=7; Array;").expect("module compile+run");
    assert!(r.is_undefined(), "实际 {r:?}");
}

#[test]
fn module_class_promise_shadow_runs() {
    let r = eval_module("class Promise{} Promise;").expect("module compile+run");
    assert!(r.is_undefined(), "实际 {r:?}");
}

#[test]
fn module_let_shadow_tdz_read_throws() {
    let err = eval_module("Math; let Math=1;").unwrap_err();
    assert!(err.contains("before initialization"), "got: {err}");
}
