//! TDZ 错误消息绑定名钉：运行时 cell 的四个 TDZ 抛点（own cell 读/写、upvalue
//! 读/写）消息须携带被访问绑定名（`Cannot access 'x' before initialization`，
//! 与 node 口径一致）；取不到名时回退通用文本。每站点各钉一例：闭包读未初始化
//! 捕获 let（LOAD_UPVALUE）、闭包写未初始化捕获 let（STORE_UPVALUE）、同函数块级
//! let 声明前读（CELL_GET）、同函数块级 let 声明前写（CELL_SET）。
//!
//! 块级 let 用例的被捕获绑定须被嵌套闭包引用（捕获名跳过静态 TDZ 守卫、留运行时
//! 判定），故源例均含一个引用该绑定的嵌套闭包。顶层直读（无函数对象）取不到名，
//! 回退通用文本，属已知残面，单独钉住。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_compiler::DefaultCompilerService;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("parse error: {}", e[0].message))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("compile error: {e}"))?;
    let mut vm = Vm::new();
    vm.set_compiler_service(Arc::new(DefaultCompilerService));
    vm.run(&Arc::new(module))
}

/// 断言程序抛错且未捕获错误消息含带名 TDZ 形态。
fn assert_tdz_message(source: &str, binding: &str) {
    let err = match eval(source) {
        Ok(v) => panic!("expected a TDZ throw, got completion value {v:?}\nsource: {source}"),
        Err(e) => e,
    };
    let expected = format!("Cannot access '{binding}' before initialization");
    assert!(err.contains(&expected), "expected {expected} in: {err}\nsource: {source}");
}

#[test]
fn tdz_upvalue_read_message_carries_binding_name() {
    // 闭包读未初始化捕获 let（LOAD_UPVALUE 站点）：消息带绑定名。
    assert_tdz_message("function f() { return x; } f(); let x = 1", "x");
}

#[test]
fn tdz_upvalue_write_message_carries_binding_name() {
    // 闭包写未初始化捕获 let（STORE_UPVALUE 站点）：消息带绑定名。
    assert_tdz_message("function f() { x = 1; } f(); let x = 1", "x");
}

#[test]
fn tdz_cell_read_message_carries_binding_name() {
    // 同函数块级 let 声明前读（CELL_GET 站点）：绑定被块内闭包捕获，静态守卫
    // 跳过，声明前读经运行时 cell 抛带名 TDZ。
    assert_tdz_message("function f() { const g = () => x; return x; let x; } f()", "x");
}

#[test]
fn tdz_cell_write_message_carries_binding_name() {
    // 同函数块级 let 声明前写（CELL_SET 站点）：同读例的捕获形态。
    assert_tdz_message("function f() { const g = () => x; x = 1; let x; } f()", "x");
}

#[test]
fn tdz_top_level_read_falls_back_to_generic_message() {
    // 顶层直读（无函数对象）取不到绑定名：回退通用文本（已知残面钉）。
    let err = match eval("function f() { return x; } x; let x = 1") {
        Ok(v) => panic!("expected a TDZ throw, got completion value {v:?}"),
        Err(e) => e,
    };
    assert!(
        err.contains("Cannot access variable before initialization"),
        "expected generic TDZ message in: {err}"
    );
}
