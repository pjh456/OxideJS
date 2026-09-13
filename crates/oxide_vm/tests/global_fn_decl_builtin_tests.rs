//! 顶层函数声明撞不可写全局内置（undefined/NaN/Infinity）：脚本面声明不更新绑定
//! （A 侧保留原常量，读/调/typeof 面收敛全局对象，用户名与模块面不受影响）；
//! eval 代码面——sloppy 函数声明在建立全局绑定前抛 TypeError（程序首指令 abrupt，
//! 无部分创建，A 侧保留），strict 函数声明为局部绑定不抛、不落全局。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_emit::module::{ModuleSourceLoader, ResolvedModule};
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

fn assert_err_contains(result: Result<JsValue, String>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error containing '{expected}', got Ok"),
        Err(e) => assert!(e.contains(expected), "expected error containing '{expected}', got: {e}"),
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

/// 同一 VM 续跑脚本读 A 侧并提取结果字符串（模块完成值非末语句值，局部
/// 绑定面经 global 属性旁路观测）。
fn script_str(vm: &mut Vm, source: &str) -> Option<String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let r = vm.run(&Arc::new(module)).expect("run");
    vm.lookup_str(r)
}

/// 无依赖模块加载器：本组测试模块源无 import，resolve 不被调用。
struct NoopLoader;

impl ModuleSourceLoader for NoopLoader {
    fn resolve(
        &mut self, _base_dir: &str, _specifier: &str, _attributes: &[(&str, &str)],
    ) -> Result<ResolvedModule, String> {
        Err("test module has no imports".into())
    }
}

// ── 脚本面：三常量函数声明静默 no-op，绑定不更新 ──

#[test]
fn script_fn_decl_three_constants_sloppy_typeof_keeps_original_type() {
    assert_eq!(eval_str("function undefined(){} typeof undefined"), "undefined");
    assert_eq!(eval_str("function NaN(){} typeof NaN"), "number");
    assert_eq!(eval_str("function Infinity(){} typeof Infinity"), "number");
}

#[test]
fn script_fn_decl_three_constants_strict_no_throw_typeof_keeps_original_type() {
    assert_eq!(eval_str("'use strict'; function undefined(){} typeof undefined"), "undefined");
    assert_eq!(eval_str("'use strict'; function NaN(){} typeof NaN"), "number");
    assert_eq!(eval_str("'use strict'; function Infinity(){} typeof Infinity"), "number");
}

#[test]
fn script_fn_decl_nan_infinity_self_equality_preserved() {
    let r = eval("'use strict'; function NaN(){}; NaN !== NaN").unwrap();
    assert!(r.is_bool() && r.as_bool(), "声明后 NaN 自反不等性应保留，实际 {r:?}");
    let r = eval("'use strict'; function Infinity(){}; Infinity === Infinity").unwrap();
    assert!(r.is_bool() && r.as_bool(), "声明后 Infinity 自相等性应保留，实际 {r:?}");
}

#[test]
fn script_fn_decl_global_descriptor_value_and_writability_preserved() {
    // undefined：值保留原常量（=== undefined），writable 保持 false。
    let r = eval(
        "function undefined(){} var d = Object.getOwnPropertyDescriptor(globalThis, 'undefined'); d.value === undefined && d.writable === false",
    )
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "undefined 描述符应保留原常量值与不可写，实际 {r:?}");
    // NaN：值保留原常量（自反不等），writable 保持 false。
    let r = eval(
        "function NaN(){} var d = Object.getOwnPropertyDescriptor(globalThis, 'NaN'); d.value !== d.value && d.writable === false",
    )
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "NaN 描述符应保留原常量值与不可写，实际 {r:?}");
    // Infinity：值保留原常量，writable 保持 false。
    let r = eval(
        "function Infinity(){} var d = Object.getOwnPropertyDescriptor(globalThis, 'Infinity'); d.value === Infinity && d.writable === false",
    )
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "Infinity 描述符应保留原常量值与不可写，实际 {r:?}");
    // 声明不改变描述符：三常量属性不可枚举，Object.keys 不新增名字。
    let r = eval(
        "function undefined(){} function NaN(){} function Infinity(){} \
         Object.keys(globalThis).filter(function (k) { return k === 'undefined' || k === 'NaN' || k === 'Infinity'; }).length === 0",
    )
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "声明不应新增可枚举全局名，实际 {r:?}");
}

#[test]
fn script_fn_decl_undefined_call_throws_type_error() {
    // 裸调用经 A 侧原常量（不可调用）抛 TypeError，不进入声明函数体返回 1。
    let result = eval("function undefined(){ return 1; } undefined()");
    assert_err_contains(result, "TypeError");
}

#[test]
fn script_fn_decl_nested_capture_call_throws_type_error() {
    // 嵌套捕获面不改变调用面：声明体内闭包不暴露，裸调用仍解析全局原常量
    //（不可调用）抛 TypeError。
    let result = eval("function undefined(){ const g = () => undefined; return g; } undefined()");
    assert_err_contains(result, "TypeError");
}

#[test]
fn script_fn_decl_user_name_binding_still_works() {
    assert_eq!(eval_str("function foo(){ return 42; } typeof foo"), "function");
    let r = eval("function foo(){ return 42; } foo()").unwrap();
    assert!(r.is_int() && r.as_int() == 42, "用户名函数声明应可调用，实际 {r:?}");
    let r = eval("function foo(){ return 42; } globalThis.foo === foo").unwrap();
    assert!(r.is_bool() && r.as_bool(), "用户名函数声明应同步 A 侧，实际 {r:?}");
}

#[test]
fn module_fn_decl_undefined_keeps_global_constant() {
    // 模块内函数声明是模块环境绑定：不命中全局门禁，模块内 typeof 是函数本体。
    // 模块完成值非末语句值，局部面经 global 属性旁路观测；全局对象 undefined
    // 仍是原常量。
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse_module(
        &allocator,
        "function undefined(){ return 1; } globalThis.__probe = typeof undefined; 0",
    )
    .expect("parse module");
    let module = Compiler::new()
        .compile_module(&program, "test.mjs", &mut NoopLoader)
        .expect("compile module");
    let mut vm = Vm::new();
    vm.run(&Arc::new(module)).expect("module run");

    assert_eq!(
        script_str(&mut vm, "globalThis.__probe"),
        Some("function".to_string()),
        "模块内 typeof undefined 应为模块环境绑定（function）"
    );
    assert_eq!(
        script_str(&mut vm, "typeof globalThis.undefined"),
        Some("undefined".to_string()),
        "模块声明不应改变全局 undefined 常量"
    );
}

// ── eval 代码面：sloppy 函数声明撞不可写全局绑定抛 TypeError ──

#[test]
fn eval_fn_decl_sloppy_throws_type_error_a_side_preserved() {
    // 撞名 eval 整体抛 TypeError（首指令 abrupt，无 var 绑定副作用），抛后
    // typeof undefined 仍是原常量（A 侧保留）。
    let s = eval_str(
        "var r = 'no-throw'; try { eval('function undefined(){}'); } \
         catch (e) { r = e instanceof TypeError ? 'TypeError' : 'other'; } r + ':' + typeof undefined",
    );
    assert_eq!(s, "TypeError:undefined");
    // generator 声明同属该门禁（同 AST 节点）：同样抛 TypeError。
    let s = eval_str(
        "var r = 'no-throw'; try { eval('function* undefined(){}'); } \
         catch (e) { r = e instanceof TypeError ? 'TypeError' : 'other'; } r",
    );
    assert_eq!(s, "TypeError");
}

#[test]
fn eval_fn_decl_with_variable_no_partial_binding_creation() {
    // 带 var 形：throw 先于 GDI 序言到达，shouldNotBeDefined 无部分创建。
    let s = eval_str(
        "var r = 'no-throw'; try { eval('var shouldNotBeDefined; function NaN(){}'); } \
         catch (e) { r = e instanceof TypeError ? 'TypeError' : 'other'; } r + ':' + \
         (Object.getOwnPropertyDescriptor(globalThis, 'shouldNotBeDefined') === undefined ? 'absent' : 'created')",
    );
    assert_eq!(s, "TypeError:absent");
}

#[test]
fn eval_fn_decl_with_function_no_partial_binding_creation() {
    // 带函数形：撞名抛错后另一声明的 A 侧写不可达，shouldNotBeDefined 不存在。
    let s = eval_str(
        "var r = 'no-throw'; try { eval('function shouldNotBeDefined(){} function NaN(){}'); } \
         catch (e) { r = e instanceof TypeError ? 'TypeError' : 'other'; } r + ':' + \
         (Object.getOwnPropertyDescriptor(globalThis, 'shouldNotBeDefined') === undefined ? 'absent' : 'created')",
    );
    assert_eq!(s, "TypeError:absent");
}

#[test]
fn eval_fn_decl_a_side_value_and_descriptor_preserved() {
    // 抛后 A 侧保留：Infinity 仍为原常量（自相等），描述符不可写位不变。
    let s = eval_str(
        "try { eval('function Infinity(){}'); } catch (e) {} \
         (Infinity === Infinity) + ':' + \
         (Object.getOwnPropertyDescriptor(globalThis, 'Infinity').writable === false ? 'w-false' : 'w-true')",
    );
    assert_eq!(s, "true:w-false");
}

#[test]
fn eval_fn_decl_strict_no_throw_a_side_preserved() {
    // strict eval 代码函数声明是局部绑定：不抛、不落全局对象，typeof 仍读全局
    // 原常量。
    let s = eval_str(
        "var r = 'threw'; try { eval(\"'use strict'; function undefined(){}\"); \
         r = 'no-throw'; } catch (e) { r = 'threw:' + e; } r + ':' + typeof undefined",
    );
    assert_eq!(s, "no-throw:undefined");
}
