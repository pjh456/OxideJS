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
    // 每次 eval 独立定义 P，两条断言互不干扰。
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
fn finally_subclass_count_resolve_side() {
    // finally ES2020 语义：子类 resolved 面共 7 次构造
    //（new + finally-then + 语料 then + 尾部 then + PromiseResolve 能力
    // + thunk-then + 委托任务 then）。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "var count = 0; var P = class extends Promise { constructor(exec) { count++; super(exec); } }; \
         P.resolve().finally(() => {}).then(() => count).then(c => c)",
    );
    assert!(ok);
    assert_eq!(val.as_int(), 7, "resolved-side finally should create 7 subclass promises");
}

#[test]
fn finally_subclass_count_reject_side() {
    // 子类 rejected 面镜像：计数同为 7，拒绝原因透传。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "var count = 0; var P = class extends Promise { constructor(exec) { count++; super(exec); } }; \
         P.reject('r').finally(() => {}).then(() => 'ok', e => 'caught:' + e + ':' + count).then(s => s)",
    );
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("caught:r:7"));
}

#[test]
fn finally_resolved_observable_sequence() {
    // resolved 面 then 可观测序列 [1,2,3,4,5]：finally 返回的 promise 被
    // 下游捕获，拒绝原因经 th 链路透传。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "var seq = []; var yes = Promise.resolve(1); \
         yes.then = function() { seq.push(1); return Promise.prototype.then.apply(this, arguments); }; \
         var no = Promise.reject(2); \
         no.then = function() { seq.push(4); return Promise.prototype.then.apply(this, arguments); }; \
         yes.then(x => { seq.push(2); return x; }).finally(() => { seq.push(3); return no; }) \
           .catch(e => { seq.push(5); return e; }) \
           .then(e => seq.join(','))",
    );
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("1,2,3,4,5"));
}

#[test]
fn finally_rejected_observable_sequence() {
    // rejected 面镜像序列 [1,2,3,4,5]：重抛经 reject 角色 thunk 原值透传。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "var seq = []; var no = Promise.reject('r'); \
         no.then = function() { seq.push(1); return Promise.prototype.then.apply(this, arguments); }; \
         var yes = Promise.resolve(1); \
         yes.then = function() { seq.push(4); return Promise.prototype.then.apply(this, arguments); }; \
         no.catch(e => { seq.push(2); throw e; }).finally(() => { seq.push(3); return yes; }) \
           .catch(e => { seq.push(5); return e; }) \
           .then(v => seq.join(','))",
    );
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("1,2,3,4,5"));
}

#[test]
fn finally_non_callable_on_finally_passed_through() {
    // onFinally 不可调用时，双处理器取 onFinally 自身原值传给 then。
    let mut vm = Vm::new();
    let (ok, val) = settled(
        &mut vm,
        "var seen; var p = Promise.resolve(1); \
         p.then = function(f, r) { seen = [f === 1, r === 1]; return Promise.prototype.then.call(this, f, r); }; \
         p.finally(1).then(v => seen.join(','))",
    );
    assert!(ok);
    assert_eq!(vm.lookup_str(val).as_deref(), Some("true,true"));
}

#[test]
fn then_species_override_used_for_derivation() {
    // constructor 上的 @@species 覆写优先于 constructor 本身：派生走
    // SpeciesConstructor 构造器且恰调用一次。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var callCount = 0; \
         var SpeciesCtor = class extends Promise { constructor(a) { super(a); callCount++; } }; \
         var p1 = new Promise(function() {}); \
         p1.constructor = function() {}; \
         p1.constructor[Symbol.species] = SpeciesCtor; \
         var d = p1.then(v => v); \
         [d.constructor === SpeciesCtor, callCount].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("true,1"));
}

#[test]
fn then_species_ctor_throw_preserves_original_value() {
    // %Promise%[Symbol.species] 覆写为抛错构造器：then 透传原异常值。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var boom = new Error('boom'); \
         var original = Object.getOwnPropertyDescriptor(Promise, Symbol.species); \
         Object.defineProperty(Promise, Symbol.species, { value: function() { throw boom; } }); \
         var thrown; \
         try { new Promise(r => r(1)).then(); } catch (e) { thrown = e; } \
         Object.defineProperty(Promise, Symbol.species, original); \
         thrown === boom",
    )
    .unwrap();
    assert!(result.as_bool(), "species getter throw should propagate the original value");
}

#[test]
fn finally_species_returning_promise_keeps_intrinsic() {
    // 子类 @@species getter 返回 %Promise%：finally 派生留在内建，不进子类。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var FooPromise = class extends Promise { static get [Symbol.species]() { return Promise; } }; \
         var p = Promise.resolve().finally(() => FooPromise.resolve()); \
         [p instanceof Promise, p instanceof FooPromise].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("true,false"));
}

#[test]
fn static_resolve_non_object_this_throws() {
    // this 非 Object 在 Type 守卫步抛 TypeError，先于同值快路径短路。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var promise = new Promise(function() {}); promise.constructor = undefined; \
         var name = 'no-throw'; \
         try { Promise.resolve.call(undefined, promise); } catch (e) { name = e.name; } \
         name",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("TypeError"));
}

#[test]
fn static_reject_non_object_this_throws() {
    // reject 镜像守卫：this 非 Object 抛 TypeError。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var name = 'no-throw'; \
         try { Promise.reject.call(null, 1); } catch (e) { name = e.name; } \
         name",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).as_deref(), Some("TypeError"));
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
