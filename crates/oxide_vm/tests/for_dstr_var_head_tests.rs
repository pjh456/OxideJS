//! for-in/for-of 解构 `var` 头叶名预声明：解构声明的绑定名须全部进入
//! 函数/全局 var 预声明集合，入口实例化到达叶名，零迭代后读见 undefined，
//! 不得误写外层同名继承槽、不得取到寄存器残留。
//!
//! 覆盖：顶层误写主形、外层函数域同名、for-of/for-in 主形与孪生、同 scope
//! 复用绑定、1 迭代覆写、默认值叶、嵌套叶、捕获叶、顶层形、数组/对象 rest、
//! 深层嵌套叶。

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

fn truthy(source: &str) {
    let result = eval(source).unwrap_or_else(|e| panic!("{source} -> {e}"));
    assert!(result.as_bool(), "{source} -> 期望 true，得 {:?}", result);
}

#[test]
fn top_level_same_name_not_clobbered() {
    // 主红形：函数内解构 var 头叶名与顶层 var 同名，零迭代后顶层值不被误写。
    truthy("var x = 5; function f(){ for (var {x} of []) {} } f(); x === 5");
}

#[test]
fn outer_function_scope_same_name_is_undefined() {
    // 外层函数域同名（仅内层声明、未赋值）：叶名建本地槽，读见 undefined。
    truthy("function f(){ function g(){ var x = 5; } for (var {x} of []) {} return typeof x } f() === 'undefined'");
}

#[test]
fn for_of_dstr_object_head_zero_iterations_is_undefined() {
    // for-of 对象解构头零迭代：叶名本地槽入口实例化，读见 undefined。
    truthy("function f(){ for (var {x} of []) {} return typeof x } f() === 'undefined'");
}

#[test]
fn for_in_dstr_array_head_zero_iterations_is_undefined() {
    // for-in 数组解构头零迭代（空可枚举）：孪生形，读见 undefined。
    truthy("function f(){ for (var [x] in {}) {} return typeof x } f() === 'undefined'");
}

#[test]
fn same_scope_reuse_binding_keeps_value() {
    // 同 scope 既有 var 同名：叶名经幂等复用同槽，声明点值保留。
    truthy("function f(){ var x = 5; for (var {x} of []) {} return typeof x } f() === 'number'");
}

#[test]
fn single_iteration_overwrites_entry_init() {
    // 1 迭代覆写：绑定点写入覆盖入口 undefined，读见迭代值。
    truthy("function f(){ for (var {x} of [{x:1}]) {} return typeof x } f() === 'number'");
}

#[test]
fn default_value_leaf_is_undefined() {
    // 默认值叶零迭代：默认值不触发，读见 undefined。
    truthy("function f(){ for (var {x = 1} of []) {} return typeof x } f() === 'undefined'");
}

#[test]
fn nested_leaf_is_undefined() {
    // 嵌套对象解构叶（递归深度）：读见 undefined。
    truthy("function f(){ for (var {a: {b}} of []) {} return typeof b } f() === 'undefined'");
}

#[test]
fn captured_leaf_is_undefined() {
    // 捕获叶：闭包经 cell 入口实例化，读见 undefined。
    truthy(
        "function f(){ for (var {x} of []) {} var g = function(){ return typeof x }; return g() } f() === 'undefined'",
    );
}

#[test]
fn top_level_dstr_head_not_clobbered() {
    // 顶层形：顶层解构 var 头零迭代，顶层值保留。
    truthy("var x = 5; for (var {x} of []) {} x === 5");
}

#[test]
fn array_rest_leaf_is_undefined() {
    // 数组解构 rest 叶：元素与 rest 均见 undefined。
    truthy("function f(){ for (var [a, ...b] of []) {} return typeof a + typeof b } f() === 'undefinedundefined'");
}

#[test]
fn object_rest_leaf_is_undefined() {
    // 对象解构 rest 叶：具名与 rest 均见 undefined。
    truthy("function f(){ for (var {x, ...r} of []) {} return typeof x + typeof r } f() === 'undefinedundefined'");
}

#[test]
fn deep_nested_array_leaf_is_undefined() {
    // 深层嵌套数组解构叶：内外叶均见 undefined。
    truthy("function f(){ for (var [[a], b] of []) {} return typeof a + typeof b } f() === 'undefinedundefined'");
}

#[test]
fn for_in_object_head_zero_iterations_is_undefined() {
    // for-in 对象解构头零迭代（空可枚举）：读见 undefined。
    truthy("function f(){ for (var {x} in {}) {} return typeof x } f() === 'undefined'");
}

#[test]
fn for_of_array_head_zero_iterations_is_undefined() {
    // for-of 数组解构头零迭代：读见 undefined。
    truthy("function f(){ for (var [x] of []) {} return typeof x } f() === 'undefined'");
}
