//! 脚本顶层 lexical 声明撞受限全局名的编译期拒绝：let/const/class 声明名撞
//! 全局对象不可配置自有属性（undefined/NaN/Infinity）→ SyntaxError（整程序
//! 编译失败）。局部作用域（函数体/块/try）与 eval 代码为合法遮蔽不拒；三常量
//! 值使用不受影响。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

/// 断言整程序编译失败且错消息为声明实例化期的重复声明错形（SyntaxError 族，
/// 消息实现自定，只断结构）。
fn assert_compile_err(source: &str) {
    let err = eval(source).unwrap_err();
    assert!(err.contains("already been declared"), "got: {err}");
}

/// 断言编译 + 运行均成功。
fn assert_ok(source: &str) -> JsValue {
    eval(source).unwrap_or_else(|e| panic!("expected ok, got: {e}\nsource: {source}"))
}

// ── 脚本顶层 let/const/class 撞三常量：编译期拒绝 ──

#[test]
fn top_level_let_undefined_rejected() {
    // 独立形（脚本内无裸 undefined 引用，无镜像槽）：规范 SyntaxError。
    assert_compile_err("let undefined;");
}

#[test]
fn top_level_let_nan_rejected() {
    assert_compile_err("let NaN;");
}

#[test]
fn top_level_let_infinity_rejected() {
    assert_compile_err("let Infinity;");
}

#[test]
fn top_level_const_nan_rejected() {
    assert_compile_err("const NaN = 1;");
}

#[test]
fn top_level_const_undefined_rejected() {
    assert_compile_err("const undefined = 1;");
}

#[test]
fn top_level_class_undefined_rejected() {
    // class 名同为 lexical 绑定（受限检查同形）；undefined 类名由解析器更早拒绝
    // （SyntaxError 族，整程序拒绝可观察行为同形），断言接受任一拒绝相位。
    assert!(eval("class undefined {}").is_err(), "class undefined 类名应被拒绝");
}

#[test]
fn top_level_class_infinity_rejected() {
    assert_compile_err("class Infinity {}");
}

#[test]
fn top_level_let_undefined_with_bare_use_rejected() {
    // 镜像形（脚本另有裸 undefined 引用）：拒绝行为不变，错误相位提前到
    // 声明预登记期（消息同形）。
    assert_compile_err("let undefined; undefined");
}

#[test]
fn top_level_let_destructured_nan_rejected() {
    // 解构叶子同属声明名，受限检查覆盖。
    assert_compile_err("let [NaN] = [1];");
}

#[test]
fn top_level_switch_case_let_nan_rejected() {
    // switch 不推作用域：case 内 lexical 声明属全局 lexical，仍受检。
    assert_compile_err("switch (0) { case 0: let NaN; }");
}

// ── eval 门控：eval 代码声明实例化无受限检查 ──

#[test]
fn eval_script_let_nan_allowed() {
    // eval 代码在独立 lexical 环境实例化，不受限检查约束；外层裸读仍是三常量值
    //（NaN 自反性用 isNaN 断言，=== 对 NaN 恒 false）。
    let r = assert_ok("eval('let NaN;'); isNaN(NaN)");
    assert!(r.is_bool() && r.as_bool(), "eval 内 let NaN 应为合法遮蔽，实际 {r:?}");
}

// ── 假阳性守卫：合法面不得误拒 ──

#[test]
fn top_level_let_uses_nan_value_allowed() {
    // 三常量值使用（非声明名）不受检查影响（isNaN 断言，=== 对 NaN 恒 false）。
    let r = assert_ok("let x = NaN; isNaN(x)");
    assert!(r.is_bool() && r.as_bool(), "let x = NaN 应正常执行，实际 {r:?}");
}

#[test]
fn top_level_let_eval_name_allowed() {
    // eval 是全局对象可配置自有属性：合法遮蔽，不在受限集。
    let r = assert_ok("let eval; 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn top_level_let_hasownproperty_allowed() {
    // hasOwnProperty 非全局对象自有属性（Object.prototype 继承）：合法遮蔽。
    let r = assert_ok("let hasOwnProperty; 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn block_level_let_nan_allowed() {
    // 顶层块内 lexical 声明是块局部绑定：不拒。
    let r = assert_ok("{ let NaN; } 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn try_block_let_nan_allowed() {
    // try 块 lexical 声明是 try 局部绑定：不拒。
    let r = assert_ok("try { let NaN; } catch (e) {} 1");
    assert_eq!(r.as_int(), 1);
}

#[test]
fn function_body_let_nan_allowed() {
    // 函数体 lexical 声明是局部绑定：不拒。
    let r = assert_ok("function f(){ let NaN; return 1; } f() === 1");
    assert!(r.is_bool() && r.as_bool(), "函数体 let NaN 应为合法局部，实际 {r:?}");
}
