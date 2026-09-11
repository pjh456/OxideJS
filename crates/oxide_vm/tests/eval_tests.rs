use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

/// 数值断言：整数运算保 int，容忍 int/double 两种表示。
fn assert_num(result: JsValue, expected: f64) {
    let actual = if result.is_int() { result.as_int() as f64 } else { result.as_double() };
    assert!((actual - expected).abs() < 0.0001, "expected {expected}, got {actual}");
}

#[test]
fn eval_string_expression_completion_value() {
    let mut vm = Vm::new();
    // 脚本模式完成值保留：`eval('1+2')` 返回 3（档 2 起，不再被函数模式 body wrap 吞掉）。
    let result = eval(&mut vm, "eval('1+2')").unwrap();
    assert_num(result, 3.0);
}

#[test]
fn eval_string_number_completion_value() {
    let mut vm = Vm::new();
    // 脚本模式完成值保留：`eval('42')` 返回 42。
    let result = eval(&mut vm, "eval('42')").unwrap();
    assert_num(result, 42.0);
}

#[test]
fn eval_string_var_declaration_inside() {
    let mut vm = Vm::new();
    // 档 2：eval 内 var 声明落全局对象，完成值保留为末表达式值。
    let result = eval(&mut vm, "eval('var y = 1; y')").unwrap();
    assert_num(result, 1.0);
    let result = eval(&mut vm, "eval('var y = 1; y') && this.y === 1").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_non_string_returns_as_is() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "eval(123)").unwrap();
    assert_num(result, 123.0);
}

#[test]
fn eval_object_identity() {
    let mut vm = Vm::new();
    // 非字符串实参原样返回：同一对象引用。
    let result = eval(&mut vm, "var o = {}; eval(o) === o").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_new_string_not_tostring() {
    let mut vm = Vm::new();
    // new String 是非字符串对象：不 ToString，原样返回同一对象（非原始值）。
    let result = eval(&mut vm, "var s = new String('1+1'); eval(s) === s").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_throw_primitive_rethrown() {
    let mut vm = Vm::new();
    // eval 内 throw 1 重抛原始值 1，可被外层 catch 捕获（非 Error 包装）。
    let result = eval(&mut vm, "try { eval('throw 1') } catch(e) { e }").unwrap();
    assert_num(result, 1.0);
}

#[test]
fn eval_syntax_error_throws() {
    let mut vm = Vm::new();
    // 换行分隔的 `x` 与 `++` 为非法语法，eval 抛 SyntaxError。
    let result = eval(&mut vm, "try { eval('x\\u000A++') } catch(e) { e.name }").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "SyntaxError");
}

#[test]
fn eval_length_is_one() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "eval.length").unwrap();
    assert_num(result, 1.0);
}

#[test]
fn eval_name_is_eval() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "eval.name").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "eval");
}

#[test]
fn eval_global_descriptor() {
    let mut vm = Vm::new();
    // 全局 eval 属性描述符：{writable:true, enumerable:false, configurable:true}。
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(this, 'eval'); \
         d.writable && !d.enumerable && d.configurable",
    )
    .unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_new_throws_type_error() {
    let mut vm = Vm::new();
    // eval 不是构造器：new eval() 抛 TypeError。
    let result = eval(&mut vm, "try { new eval() } catch(e) { e.name }").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "TypeError");
}

#[test]
fn eval_no_arg_and_undefined_are_undefined() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "eval() === undefined && eval(undefined) === undefined").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_typeof_is_function() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "typeof eval").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "function");
}

#[test]
fn eval_survives_full_reset_rebuild() {
    let mut vm = Vm::new();
    // 仅 global 脏：full_reset 走 bind_global_functions 重建路径，eval 须保留。
    let g_ptr = vm.session().global_object().as_ptr() as *mut JsObject;
    unsafe { (&mut *g_ptr).bump_generation() };
    vm.full_reset();

    let result = eval(&mut vm, "typeof eval").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "function");
    let result = eval(&mut vm, "eval.length === 1").unwrap();
    assert_eq!(result, JsValue::bool(true));
    let result = eval(&mut vm, "eval(123)").unwrap();
    assert_num(result, 123.0);
    assert!(!vm.session().is_dirty_since_snapshot());
}

#[test]
fn eval_script_var_lands_on_global() {
    let mut vm = Vm::new();
    // 间接 eval 脚本模式：var 声明落全局对象，外层 this 可见。
    let result = eval(&mut vm, "(0,eval)('var q = 9'); this.q === 9").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_script_function_decl_lands_on_global() {
    let mut vm = Vm::new();
    // 脚本模式：顶层函数声明落全局对象。
    let result = eval(&mut vm, "(0,eval)('function f(){}'); typeof f === 'function'").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_script_let_is_isolated() {
    let mut vm = Vm::new();
    // let/const 词法隔离：不落全局，不泄漏到外层。
    let result = eval(&mut vm, "(0,eval)('let z = 1'); typeof z === 'undefined'").unwrap();
    assert_eq!(result, JsValue::bool(true));
    // 词法声明自身仍参与脚本完成值。
    let result = eval(&mut vm, "(0,eval)('let z2 = 2; z2')").unwrap();
    assert_num(result, 2.0);
}

#[test]
fn eval_script_this_is_global() {
    let mut vm = Vm::new();
    // 脚本模式：this 与 globalThis 恒等（inline 调用 receiver 传 global）。
    let result = eval(&mut vm, "(0,eval)('this === globalThis')").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_nested_eval() {
    let mut vm = Vm::new();
    // 嵌套 eval：内层完成值透传为外层脚本值。
    let result = eval(&mut vm, "eval(\"eval('1+1')\")").unwrap();
    assert_num(result, 2.0);
}

#[test]
fn eval_script_completion_value() {
    let mut vm = Vm::new();
    // 间接 eval 脚本模式完成值：var 初始化 + 表达式。
    let result = eval(&mut vm, "(0,eval)('var a = 1; a')").unwrap();
    assert_num(result, 1.0);
    let result = eval(&mut vm, "(0,eval)('1 + 2')").unwrap();
    assert_num(result, 3.0);
}

#[test]
fn eval_script_empty_and_decl_only() {
    let mut vm = Vm::new();
    // 空脚本 / 纯声明脚本：完成值为 undefined。
    let result = eval(&mut vm, "eval('') === undefined").unwrap();
    assert_eq!(result, JsValue::bool(true));
    let result = eval(&mut vm, "eval('var x;') === undefined").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn eval_script_throw_still_rethrows() {
    let mut vm = Vm::new();
    // 脚本模式异常路径回归：eval 内 throw 原始值，外层 catch 捕获同一值。
    let result = eval(&mut vm, "try { (0,eval)('throw 5') } catch(e) { e }").unwrap();
    assert_num(result, 5.0);
}
