//! sloppy 普通函数 `this` 绑定语义测试：非箭头 && 非 strict && this 为 null/undefined
//! 时替换为全局对象；箭头/严格/method/构造/显式 receiver 均不替换。
//! 覆盖普通调用、call/apply/bind、builtin 回调、构造、生成器、async、derived 构造。

use oxide_compiler::compiler::Compiler;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

/// 断言表达式结果为 `true`。
fn assert_true(vm: &mut Vm, source: &str) {
    let result = eval(vm, source).unwrap_or_else(|e| panic!("eval 失败: {} \n source: {}", e, source));
    assert_eq!(result, JsValue::bool(true), "期望 true，source: {}", source);
}

fn global_ptr(vm: &Vm) -> usize {
    vm.session().global_object().as_ptr() as *mut JsObject as usize
}

// ── 普通调用 ──

#[test]
fn sloppy_direct_call_binds_global_this() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "(function(){ return this; })()").unwrap();
    assert_eq!(result.as_js_object_ptr() as usize, global_ptr(&vm), "sloppy 普通调用 this 应为全局对象");
}

#[test]
fn strict_direct_call_keeps_undefined_this() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "(function(){ 'use strict'; return this; })()").unwrap();
    assert!(result.is_undefined(), "strict 函数普通调用 this 保持 undefined，got: {:?}", result);
}

#[test]
fn strict_inside_sloppy_inherits_directive() {
    let mut vm = Vm::new();
    // sloppy 外层调用 strict 内层：内层自身 directive 使其 this 保持 undefined。
    let result = eval(
        &mut vm,
        "function outer(){ return (function(){ 'use strict'; return this; })(); } outer()",
    )
    .unwrap();
    assert!(result.is_undefined(), "strict 内层 this 应为 undefined，got: {:?}", result);
}

#[test]
fn sloppy_top_level_script_this_is_global() {
    let mut vm = Vm::new();
    assert_true(&mut vm, "this === globalThis");
}

// ── 箭头 / method / 构造 ──

#[test]
fn arrow_keeps_lexical_this() {
    let mut vm = Vm::new();
    assert_true(&mut vm, "var f = () => this; f() === globalThis");
}

#[test]
fn method_call_keeps_receiver() {
    let mut vm = Vm::new();
    assert_true(&mut vm, "var o = { m(){ return this === o; } }; o.m()");
}

#[test]
fn detached_method_call_binds_global() {
    let mut vm = Vm::new();
    // 方法解绑后无 receiver 调用：sloppy 函数 this 应为全局对象。
    assert_true(&mut vm, "var o = { m: function(){ return this === globalThis; } }; var m = o.m; m()");
}

#[test]
fn constructor_this_is_new_instance() {
    let mut vm = Vm::new();
    assert_true(&mut vm, "var a = new (function(){ this.self = this; })(); a.self === a");
}

#[test]
fn derived_constructor_this_is_instance() {
    let mut vm = Vm::new();
    assert_true(
        &mut vm,
        "class B {} class A extends B { constructor(){ super(); this.self = this; } } var a = new A(); a.self === a",
    );
}

// ── call / apply / bind ──

#[test]
fn call_apply_bind_with_nullish_this_binds_global() {
    let mut vm = Vm::new();
    assert_true(&mut vm, "function f(){ return this === globalThis; } f.call(undefined)");
    assert_true(&mut vm, "function f(){ return this === globalThis; } f.call(null)");
    assert_true(&mut vm, "function f(){ return this === globalThis; } f.apply(null)");
    assert_true(&mut vm, "function f(){ return this === globalThis; } f.bind(undefined)()");
}

#[test]
fn explicit_object_receiver_not_replaced() {
    let mut vm = Vm::new();
    assert_true(&mut vm, "var o = {}; function f(){ return this === o; } f.call(o)");
}

#[test]
fn strict_call_with_nullish_keeps_undefined() {
    let mut vm = Vm::new();
    assert_true(&mut vm, "function f(){ 'use strict'; return this === undefined; } f.call(undefined)");
}

// ── builtin 回调 / 生成器 / async ──

#[test]
fn builtin_callback_sloppy_this_binds_global() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "[1,2].map(function(){ return this === globalThis; }).join(',')").unwrap();
    let rendered = vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "true,true");
}

#[test]
fn generator_this_binds_global() {
    let mut vm = Vm::new();
    assert_true(&mut vm, "(function*(){ return this === globalThis; })().next().value");
}

#[test]
fn async_function_this_binds_global() {
    let mut vm = Vm::new();
    // async 函数 body 同步执行到首个 await：无 await 时整个 body 立即完成。
    assert_true(&mut vm, "var ok = false; (async function(){ ok = (this === globalThis); })(); ok");
}

// ── this 写路径回归 ──

#[test]
fn sloppy_this_write_lands_on_global() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "function f(){ this.this_marker = 42; } f(); globalThis.this_marker").unwrap();
    assert_eq!(result.as_int(), 42, "sloppy `this.x =` 应写入全局对象");
}

#[test]
fn strict_this_write_throws() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "function f(){ 'use strict'; this.strict_marker = 1; } f()").unwrap_err();
    assert!(
        err.contains("TypeError"),
        "strict 函数 this 为 undefined，写属性应抛 TypeError，got: {err}"
    );
}
