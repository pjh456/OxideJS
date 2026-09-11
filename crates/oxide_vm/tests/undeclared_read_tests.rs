//! 未声明标识符读的 ReferenceError 语义（B031 Step B 落地）。
//!
//! 覆盖：读未声明抛 ReferenceError；typeof 未声明返回 "undefined"；
//! sloppy 写后读正常；harness 全局 var 声明安全；内置标识符不误判。

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

/// 求值并取字符串结果内容（字符串值走 lookup_str，非字符串走 Display）。
fn eval_str(source: &str) -> Result<String, String> {
    let mut vm = Vm::new();
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let result = vm.run(&Arc::new(module))?;
    Ok(vm.lookup_str(result).unwrap_or_default())
}

#[test]
fn undeclared_read_throws_reference_error() {
    // 未声明标识符读取：ReferenceError 且消息含名字（与 V8 一致）。
    let err = eval("x").unwrap_err();
    assert!(err.contains("ReferenceError") && err.contains("x is not defined"), "got: {}", err);
}

#[test]
fn undeclared_read_throws_inside_block() {
    let err = eval("{ x; }").unwrap_err();
    assert!(err.contains("ReferenceError") && err.contains("x is not defined"), "got: {}", err);
}

#[test]
fn undeclared_read_throws_inside_function() {
    let err = eval("function f(){ return x; } f()").unwrap_err();
    assert!(err.contains("ReferenceError") && err.contains("x is not defined"), "got: {}", err);
}

#[test]
fn sloppy_write_then_read_ok() {
    // sloppy 写先登记全局槽：写后读命中符号表，G6 不破坏写读一致性。
    let result = eval("x = 5; x").unwrap();
    assert_eq!(result.as_int(), 5);
}

#[test]
fn typeof_undeclared_is_undefined() {
    let result = eval_str("typeof x").unwrap();
    assert_eq!(result, "undefined");
}

#[test]
fn typeof_undeclared_parenthesized_is_undefined() {
    // 括号不改变标识符引用语义，typeof (x) 同样返回 "undefined"。
    let result = eval_str("typeof (x)").unwrap();
    assert_eq!(result, "undefined");
}

#[test]
fn typeof_undeclared_inside_function_is_undefined() {
    let result = eval_str("function f(){ return typeof q; } f()").unwrap();
    assert_eq!(result, "undefined");
}

#[test]
fn typeof_before_var_decl_is_undefined() {
    // var 提升后声明点前读：槽值 undefined，typeof 返回 "undefined"。
    let result = eval_str("var x; typeof x").unwrap();
    assert_eq!(result, "undefined");
}

#[test]
fn typeof_undeclared_isolated_from_block_scope() {
    // 块内 let 不同作用域（已弹出），外层 typeof 仍视为未声明。
    let result = eval_str("{ let x; } typeof x").unwrap();
    assert_eq!(result, "undefined");
}

#[test]
fn typeof_tdz_binding_throws() {
    // G2 回归防护：typeof 对 TDZ 绑定抛 ReferenceError（非 "undefined"）。
    let err = eval("typeof x; let x;").unwrap_err();
    assert!(err.contains("ReferenceError"), "got: {}", err);
}

#[test]
fn typeof_after_sloppy_write_is_value_type() {
    // 写路径登记的全局槽走 LOAD_VAR，typeof 读到真实值。
    let result = eval_str("x = 5; typeof x").unwrap();
    assert_eq!(result, "number");
}

#[test]
fn typeof_captured_upvalue_ok() {
    // 闭包捕获的 upvalue 不走未声明分支，typeof 读到外层值。
    let result = eval_str("function outer(){ var x = 1; return function(){ return typeof x; }; } outer()()").unwrap();
    assert_eq!(result, "number");
}

#[test]
fn harness_global_var_decl_safe() {
    // harness 以 var 声明注入的全局（$DONE 等）不误抛。
    let result = eval("var $DONE = 1; $DONE").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn builtin_global_not_misjudged() {
    // 内置标识符（builtin_reg_map 命中 global）不误判为未声明。
    let result = eval_str("JSON.stringify({a:1})").unwrap();
    assert_eq!(result, r#"{"a":1}"#);
}

#[test]
fn builtin_undefined_nan_infinity_reads_ok() {
    // undefined/NaN/Infinity 是 global object 属性，读取不抛。
    let result = eval("undefined").unwrap();
    assert!(result.is_undefined());
    let result = eval("NaN").unwrap();
    assert!(result.is_double() && result.as_double().is_nan());
    let result = eval("Infinity").unwrap();
    assert!(result.is_double() && result.as_double().is_infinite());
}

#[test]
fn global_object_property_read_fresh() {
    // 读未声明标识符时运行期查 global object：属性后置创建也能读到（非帧入口快照）。
    let result = eval("globalThis.foo = 42; foo").unwrap();
    assert_eq!(result.as_int(), 42);
}

#[test]
fn block_shadow_after_implicit_read_ok() {
    // 块作用域同名新绑定持不同槽位，不被隐式全局集合误判（按寄存器记录）。
    let result = eval("globalThis.b = 42; b; { let b = 7; b; }").unwrap();
    assert_eq!(result.as_int(), 7);
}
