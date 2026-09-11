use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::promise::promise_settled_value;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

/// 顶层表达式返回 Promise 时，run() 末尾 drain 已执行微任务，读其结算值。
fn settled(vm: &mut Vm, source: &str) -> (bool, JsValue) {
    let result = eval(vm, source).expect("run should succeed");
    assert!(result.is_object(), "expected a promise, got {result:?}");
    let obj = unsafe { &*result.as_js_object_ptr() };
    promise_settled_value(obj).expect("promise should be settled")
}

#[test]
fn resolve_then_drains_chain() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.resolve(42).then(v => v * 2)");
    assert!(ok);
    assert_eq!(val.as_double(), 84.0);
}

#[test]
fn chain_orders_fifo() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.resolve(1).then(v => v + 1).then(v => v * 10)");
    assert!(ok);
    assert_eq!(val.as_double(), 20.0);
}

#[test]
fn settle_only_once() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "new Promise((res, rej) => { res(1); res(2); rej(3); }).then(v => v)");
    assert!(ok);
    assert_eq!(val.as_int(), 1);
}

#[test]
fn throw_in_handler_goes_to_catch() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.resolve(1).then(v => { throw 'x' }).catch(e => e)");
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("x"));
}

#[test]
fn thenable_delegation() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.resolve({ then: function(r) { r(7) } }).then(v => v)");
    assert!(ok);
    assert_eq!(val.as_int(), 7);
}

#[test]
fn finally_passes_through() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "new Promise(r => { r(10) }).finally(() => {}).then(v => v)");
    assert!(ok);
    assert_eq!(val.as_int(), 10);
}

#[test]
fn reject_propagates_through_empty_then() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "Promise.reject('e').then(undefined, e => 'caught:' + e)");
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("caught:e"));
}

#[test]
fn ctor_receiver_used_for_capability() {
    let mut vm = Vm::new();
    // Promise.resolve.call(NotPromise) 应构造 NotPromise 并捕获其 executor。
    let result = eval(
        &mut vm,
        "var ef; function NotP(exec) { ef = exec; exec(function(){}, function(){}); } \
         Promise.resolve.call(NotP); ef.length",
    )
    .unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn custom_ctor_failure_is_type_error() {
    let mut vm = Vm::new();
    // executor 未捕获 resolve/reject → TypeError。
    let result = eval(
        &mut vm,
        "function NotP(exec) { exec(function(){}); } \
         try { Promise.resolve.call(NotP); 'no-throw' } catch (e) { e.name }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("TypeError"));
}

#[test]
fn resolve_thenable_then_reject_keeps_fulfilled() {
    // P3：resolve(thenable) 置位 alreadyResolved，随后同调用内 reject 不覆盖。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "new Promise((res, rej) => { res({ then(res2) { res2({ then(r3) { r3('final'); } }); } }); rej('oops'); }).then(v => v, e => 'rejected:' + e)",
    );
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("final"));
}

#[test]
fn double_resolve_second_noop() {
    let mut vm = Vm::new();
    let (ok, val) = settled(&mut vm, "new Promise((res) => { res(1); res(2); }).then(v => v)");
    assert!(ok);
    assert_eq!(val.as_int(), 1);
}

#[test]
fn reject_then_resolve_keeps_rejected() {
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "new Promise((res, rej) => { rej('e'); res(1); }).then(v => v, e => 'rejected:' + e)",
    );
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("rejected:e"));
}

#[test]
fn thenable_resolve_after_reject_noop() {
    // reject 先行置位，随后 resolve(thenable) 不得触发委托结算。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "new Promise((res, rej) => { rej('e'); res({ then(r) { r(1); } }); }).then(v => v, e => 'rejected:' + e)",
    );
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("rejected:e"));
}

#[test]
fn thenable_chain_after_already_resolved_noop() {
    // 双重 resolve(thenable)：第二次委托不结算，结果保持第一次。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "new Promise((res) => { res({ then(r) { r(5); } }); res({ then(r) { r(9); } }); }).then(v => v)",
    );
    assert!(ok);
    assert_eq!(val.as_int(), 5);
}

#[test]
fn subclass_then_derives_subclass_instance() {
    // 子类 then 派生：class P extends Promise {}，p.then 返回 P 实例（经子类构造器）。
    // 单次 eval 完成断言：跨 eval 时旧 module 的 bytecode 函数对象随 sub_modules
    // 替换失效（引擎既有局限），故不拆分执行。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var P = class extends Promise {}; var d = new P(r => r(1)).then(v => v); d.constructor === P",
    )
    .unwrap();
    assert!(result.as_bool(), "subclass then should derive via subclass ctor");
}

#[test]
fn subclass_catch_finally_derive_subclass() {
    // catch/finally 经 this.then 调用，自动获得子类派生语义。
    // 每次 eval 独立定义 P：跨 eval 复用旧 module 函数对象会随 sub_modules 替换失效。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var P = class extends Promise {}; var d = new P(r => r(1)).catch(()=>{}); d.constructor === P",
    )
    .unwrap();
    assert!(result.as_bool());
    let result = eval(
        &mut vm,
        "var Q = class extends Promise {}; var f = new Q(r => r(1)).finally(()=>{}); f.constructor === Q",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn subclass_chain_keeps_subclass_ctor() {
    // 链式 then 每次派生均保持子类构造器。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var P = class extends Promise {}; var d = new P(r => r(1)).then(v => v + 1).then(v => v); d.constructor === P",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn rewritten_prototype_constructor_used_for_derivation() {
    // P.prototype.constructor 改写后，then 用改写值派生。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var P = class extends Promise {}; function Alt(exec) { exec(function(){}, function(){}); } \
         P.prototype.constructor = Alt; var d = new P(r => r(1)).then(v => v); d.constructor === Alt",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn constructor_undefined_uses_intrinsic() {
    // constructor 为 undefined → 退回内置 %Promise% 派生。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var q = new Promise(r => r(1)); Object.defineProperty(q, 'constructor', { value: undefined }); \
         var d = q.then(v => v); d.constructor === Promise",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn constructor_non_object_throws_type_error() {
    // constructor 非对象 → then 抛 TypeError。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var r = new Promise(res => res(1)); Object.defineProperty(r, 'constructor', { value: 42 }); \
         try { r.then(()=>{}); 'no-throw' } catch (e) { e.name }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("TypeError"));
}

#[test]
fn constructor_getter_throw_preserves_original_value() {
    // constructor getter 抛错 → 透传原异常对象（catch 收到同一引用）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var boom = new Error('boom'); var r = new Promise(res => res(1)); \
         Object.defineProperty(r, 'constructor', { get() { throw boom; } }); \
         try { r.then(()=>{}); 'no-throw' } catch (e) { e === boom }",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn intrinsic_promise_then_regression() {
    // 回归：内置 promise 的 then 派生路径不变。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var d = new Promise(r => r(1)).then(v => v); d.constructor === Promise").unwrap();
    assert!(result.as_bool());
}

#[test]
fn await_subclass_promise_resumes_with_value() {
    // async function 内 await 子类 promise：值正确，额外派生仅来自 thenable
    // 委托（await 机制自身的能力恒为内置，不额外调用子类构造器）。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "var ctorCalls = 0; var P = class extends Promise { constructor(exec) { ctorCalls++; super(exec); } }; \
         async function f() { return await new P(r => r(1)); } \
         f().then(v => [v, ctorCalls])",
    );
    assert!(ok);
    let obj = unsafe { &*val.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(obj.get_prop_at(0).as_int(), 1, "await value should pass through");
    assert_eq!(obj.get_prop_at(1).as_int(), 2, "ctor calls: new once + thenable delegation once");
}

#[test]
fn async_gen_yield_subclass_promise_skips_species() {
    // 内部 await 能力不走 species：async generator yield 子类 promise 时，
    // yield 包装（await 展开）不得再经子类构造器派生。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "var ctorCalls = 0; var P = class extends Promise { constructor(exec) { ctorCalls++; super(exec); } }; \
         async function* g() { yield new P(r => r(1)); } \
         var it = g(); it.next().then(r => [r.value, ctorCalls])",
    );
    assert!(ok);
    let obj = unsafe { &*val.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
    assert_eq!(obj.get_prop_at(0).as_int(), 1, "yield value should pass through");
    assert_eq!(obj.get_prop_at(1).as_int(), 1, "internal await must not derive via subclass ctor");
}
