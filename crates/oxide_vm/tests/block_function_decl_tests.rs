//! 块内函数声明：块级绑定预声明后，声明点前后引用均可编译并执行。
//!
//! 覆盖：普通块内函数、块内后续调用、提前调用（阶段 1 现状）、与 let 同名冲突、
//! 捕获路径、嵌套块作用域隔离、重复声明（Annex B 最后生效）、单语句块体。

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

#[test]
fn block_function_decl_compiles_and_runs() {
    // 块内函数声明可编译执行，无返回值。
    let result = eval("{ function g(){} }").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn block_function_decl_call_after_decl() {
    // 块内声明点之后调用：正常返回函数值。
    let result = eval("{ function g(){return 1;} g(); }").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn block_function_decl_hoisted_call_before_decl_is_not_callable() {
    // 阶段 1：声明点之前调用读到未初始化槽 undefined，报 not callable
    // （比修复前的编译错好；完整 hoisting 属后续阶段）。
    let err = eval("{ g(); function g(){return 1;} }").unwrap_err();
    assert!(err.contains("not callable") || err.contains("not an object"), "got: {err}");
}

#[test]
fn block_function_decl_conflicts_with_let() {
    // 块内函数与块内 let 同名：早期错误（重复声明），顺序无关。
    let err = eval("{ let g; function g(){} }").unwrap_err();
    assert!(err.contains("already been declared"), "got: {err}");
    let err = eval("{ function g(){} let g; }").unwrap_err();
    assert!(err.contains("already been declared"), "got: {err}");
}

#[test]
fn block_function_decl_capture_escape() {
    // 块内函数被外层变量引用（捕获路径 MAKE_CELL），块外可调用。
    let result = eval("var f; { function g(){return 1;} f = g; } f()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn block_function_decl_scope_isolated() {
    // 嵌套块内函数是块级绑定：块外不可见（typeof 未声明标识符返回 undefined）。
    let result = eval("{ { function g(){return 2;} } } typeof g === 'undefined'").unwrap();
    assert!(result.as_bool());
}

#[test]
fn block_function_decl_duplicate_last_wins() {
    // 重复块内函数声明（sloppy）：最后声明生效（Annex B 语义）。
    let result = eval("{ function g(){} function g(){return 3;} g(); }").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn block_function_decl_in_if_body() {
    // 单语句块体（if consequent 为块）：递归预声明生效。
    let result = eval("if (true) { function g(){return 8;} g(); }").unwrap();
    assert_eq!(result.as_int(), 8);
}

#[test]
fn block_function_decl_nested_outer_var_capture() {
    // 嵌套块内函数引用外层变量，赋值逃逸后取值正确。
    let result = eval("var r; { { function g(){return 4;} r = g(); } } r").unwrap();
    assert_eq!(result.as_int(), 4);
}
