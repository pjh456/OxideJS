//! 裸 return 完成值测试：无值 return 物化 undefined，防 rd 槽残留
//! （如最近一次调用的结果）冒充完成值泄漏；覆盖函数体臂与派生类
//! 隐式构造器臂两条发射路径。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::promise::promise_settled_value;
use oxide_vm::vm::Vm;

/// parse → compile → run，返回 VM 与顶层执行结果。
fn run(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module)).map_err(|e| format!("vm error: {}", e))?;
    Ok((vm, result))
}

/// 断言顶层结果为真值布尔。
fn assert_true(source: &str) {
    let (_vm, result) = run(source).unwrap();
    assert!(result.is_bool() && result.as_bool(), "expected true, got: {result:?}");
}

/// 断言 async 函数完成值为 undefined（微任务已在 run 收尾 drain）。
fn assert_async_undefined(source: &str) {
    let (_vm, result) = run(source).unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_promise_obj(), "expected promise, got: {result:?}");
    let settled = promise_settled_value(obj).expect("promise should be settled after drain").1;
    assert_eq!(settled, JsValue::undefined());
}

#[test]
fn bare_return_after_call_materializes_undefined() {
    // 裸 return 前最后一次求值为调用：槽内残留值不得冒充完成值。
    assert_true("function g(){return true} function f(){if(1){g();return;}return 'END'} f() === undefined");
}

#[test]
fn async_bare_return_after_call_materializes_undefined() {
    // async 函数体与同步函数体共用同一条裸 return 发射臂。
    assert_async_undefined("function g(){return true} async function h(){if(1){g();return;}return 'END'} h()");
}

#[test]
fn bare_return_escaping_for_of_materializes_undefined() {
    // 裸 return 逃出 for-of：迭代器关闭计数透传臂同样物化 undefined。
    assert_true(
        "function g(){return true} function f(){for(var x of [1,2]){g();return;}return 'END'} f() === undefined",
    );
}

#[test]
fn derived_class_implicit_constructor_returns_instance() {
    // 隐式派生构造器尾裸 return：字段初始化器返回对象的调用结果
    // 不得漏出为构造结果。
    assert_true("class A{} class B extends A { x = (function(){return {mark:1};})() } new B() instanceof B");
}
