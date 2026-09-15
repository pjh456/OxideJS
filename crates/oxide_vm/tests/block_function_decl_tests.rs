//! 块内函数声明：块级绑定预声明后，声明点前后引用均可编译并执行。
//!
//! 覆盖：普通块内函数、块内后续调用、提前调用（阶段 1 现状）、与 let 同名冲突、
//! 捕获路径、嵌套块作用域隔离、重复声明（Annex B 最后生效）、单语句块体、
//! if 支臂声明（块入口不物化、仅被求值支执行期生效、同名直接子的双绑定面）、
//! 标签直接子（块入口可见、外层泄漏）。

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
fn block_function_decl_call_before_decl() {
    // 声明点之前调用：块绑定块入口即持有函数对象，提前调用正常返回。
    let result = eval("{ g(); function g(){return 1;} }").unwrap();
    assert_eq!(result.as_int(), 1);
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
fn block_function_decl_leaks_outer_in_sloppy_script() {
    // sloppy 块级函数声明名泄漏外层（web-compat 外层 var 绑定 + 求值写回）：
    // 块后读见函数对象而非未声明名；块内声明前读同样为函数对象（块入口
    // 初始化），块内声明后读为函数。
    let result = eval("{ { function g(){return 2;} } } typeof g === 'function'").unwrap();
    assert!(result.as_bool(), "sloppy 块函数名应泄漏外层，got {:?}", result);
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

#[test]
fn block_fn_if_arm_decl_no_entry_init() {
    // 支臂声明不做块入口初始化：声明点前读命中未初始化的块槽，typeof 为
    // undefined 而非 function（与直接子声明的块入口可见性区分）。
    let result = eval("{ var t = typeof g; if (true) function g(){return 1}; t }").unwrap();
    assert!(
        result.is_string() && unsafe { &*result.as_string_ptr() }.as_str() == "undefined",
        "支臂声明前读应为 undefined，得 {result:?}"
    );
}

#[test]
fn block_fn_if_arm_decl_runs_on_taken_branch() {
    // 仅被求值支的声明生效：true 支物化 return 1 的闭包，else 支不求值。
    let result = eval("{ if (true) function g(){return 1} else function g(){return 2}; g() }").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn block_fn_if_arm_decl_leaks_outer_from_taken_branch() {
    // 被求值支的声明经外层 var 写回泄漏块外：块后读见该支的闭包。
    let result = eval("{ if (true) function g(){return 1} else function g(){return 2} } g()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn block_fn_if_arm_call_before_decl_throws() {
    // 声明点前调用：块槽未物化，调用未初始化值抛错（入口不预物化支臂）。
    let err = eval("{ g(); if (true) function g(){return 1} }").unwrap_err();
    assert!(err.contains("not callable"), "got: {err}");
}

#[test]
fn block_fn_arm_same_name_as_direct_child_keeps_block_binding() {
    // 直接子 + 支臂同名：支臂声明只更新外层 var 绑定、块绑定不动——块内读
    // 恒见直接子的闭包（V8 双绑定面在引擎块槽侧的投影）。
    let result = eval("{ function g(){return 1}; if (true) function g(){return 2}; g() }").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn block_fn_arm_same_name_as_direct_child_updates_outer_var() {
    // 同名形块外读：支臂声明的外层 var 写回生效，块后读见支臂闭包。
    let result = eval("{ function g(){return 1}; if (true) function g(){return 2} } g()").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn block_fn_read_between_direct_child_and_arm() {
    // 直接子声明与 if 之间的读：支臂尚未求值，读命中直接子的块入口闭包。
    let result = eval("{ function g(){return 1}; var r = g(); if (true) function g(){return 2}; r }").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn block_fn_if_arm_decl_readable_after_decl() {
    // 块内声明点后读：被求值支的闭包已物化入块槽，可正常调用。
    let result = eval("{ if (true) function g(){return 1}; g() }").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn block_fn_else_arm_decl_evaluated() {
    // else 支被求值：生效的是 else 支的闭包（执行期物化，与源序末位无关）。
    let result = eval("{ if (false) function g(){return 1} else function g(){return 2}; g() }").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn block_fn_labeled_direct_body_entry_visible() {
    // 标签直接子体声明保持块入口可见：声明点前读命中入口物化的闭包。
    let result = eval("{ var t = typeof g; l: function g(){return 1}; t }").unwrap();
    assert!(
        result.is_string() && unsafe { &*result.as_string_ptr() }.as_str() == "function",
        "标签声明前读应为 function，得 {result:?}"
    );
}

#[test]
fn block_fn_labeled_direct_body_leaks_outer() {
    // 标签直接子体声明经外层 var 写回泄漏块外：块后读可调用。
    let result = eval("{ l: function g(){return 1} } g()").unwrap();
    assert_eq!(result.as_int(), 1);
}
