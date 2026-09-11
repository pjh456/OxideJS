//! AsyncDisposableStack 内置对象测试：构造器/原型形状、use 读取顺序与入栈语义、
//! disposeAsync 状态机（微任务序/错误链/防重入/reject 而非抛）、move、GC 交叉。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_parser::Allocator;
use oxide_vm::promise::promise_settled_value;
use oxide_vm::vm::Vm;

/// 执行源码并格式化顶层结果；Promise 结果取其 drain 后的 settled 值。
fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&Arc::new(module)) {
        Ok(result) => format_value(&vm, result),
        Err(e) => format!("vm error: {e}"),
    }
}

fn format_value(vm: &Vm, val: oxide_vm::JsValue) -> String {
    if val.is_string() {
        format!("\"{}\"", vm.lookup_str(val).unwrap_or_default())
    } else if val.is_object() {
        let obj = unsafe { &*val.as_js_object_ptr() };
        if obj.is_promise_obj() {
            match promise_settled_value(obj) {
                Some((true, v)) => format_value(vm, v),
                Some((false, v)) => format!("<rejected {}>", format_value(vm, v)),
                None => "<pending>".to_string(),
            }
        } else if obj.is_array() {
            "[array]".to_string()
        } else {
            "[object]".to_string()
        }
    } else {
        format!("{val}")
    }
}

/// 构造器基本形状：new 建对象、disposed=false、proto/instanceof、原型链到 Object.prototype。
#[test]
fn constructor_shapes() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); \
             s.disposed + '|' + (Object.getPrototypeOf(s) === AsyncDisposableStack.prototype) + '|' + \
             (s instanceof AsyncDisposableStack) + '|' + \
             (Object.getPrototypeOf(AsyncDisposableStack.prototype) === Object.prototype)",
        ),
        "\"false|true|true|true\"",
    );
}

/// 构造器 length/name 与 prototype 槽描述符 {f,f,f}、proto.constructor={t,f,t}、
/// 普通调用抛 TypeError。
#[test]
fn constructor_identity() {
    assert_eq!(
        eval(
            "var p = Object.getOwnPropertyDescriptor(AsyncDisposableStack, 'prototype'); \
             var c = Object.getOwnPropertyDescriptor(AsyncDisposableStack.prototype, 'constructor'); \
             AsyncDisposableStack.length + '|' + AsyncDisposableStack.name + '|' + \
             p.writable + '|' + p.enumerable + '|' + p.configurable + '|' + \
             c.writable + '|' + c.enumerable + '|' + c.configurable + '|' + \
             (AsyncDisposableStack.prototype.constructor === AsyncDisposableStack) + '|' + \
             (function(){ try { AsyncDisposableStack(); return 'no' } catch (e) { return e.constructor.name } }())",
        ),
        "\"0|AsyncDisposableStack|false|false|false|true|false|true|true|TypeError\"",
    );
}

/// 原型形状：@@asyncDispose 别名同一函数对象、@@toStringTag 值+描述符 {f,f,t}、
/// 方法 length/name/{t,f,t}、disposed 为访问器 getter。
#[test]
fn prototype_shape() {
    assert_eq!(
        eval(
            "var p = AsyncDisposableStack.prototype; \
             var tag = Object.getOwnPropertyDescriptor(p, Symbol.toStringTag); \
             var use = Object.getOwnPropertyDescriptor(p, 'use'); \
             var da = Object.getOwnPropertyDescriptor(p, 'disposeAsync'); \
             var acc = Object.getOwnPropertyDescriptor(p, 'disposed'); \
             (p[Symbol.asyncDispose] === p.disposeAsync) + '|' + \
             Object.prototype.toString.call(new AsyncDisposableStack()) + '|' + \
             tag.value + '|' + tag.writable + '|' + tag.enumerable + '|' + tag.configurable + '|' + \
             use.value.length + '|' + use.value.name + '|' + use.writable + '|' + use.enumerable + '|' + use.configurable + '|' + \
             da.value.length + '|' + da.value.name + '|' + \
             (typeof acc.get === 'function') + '|' + (acc.set === undefined)",
        ),
        "\"true|[object AsyncDisposableStack]|AsyncDisposableStack|false|false|true|1|use|true|false|true|0|disposeAsync|true|true\"",
    );
}

/// use：@@asyncDispose 优先读取、缺失回落 @@dispose（读取顺序断言）。
#[test]
fn use_reads_async_dispose_then_dispose() {
    assert_eq!(
        eval(
            "var order = []; \
             var o = { \
               get [Symbol.asyncDispose]() { order.push('asyncDispose'); return undefined; }, \
               get [Symbol.dispose]() { order.push('dispose'); return function() {}; } \
             }; \
             var s = new AsyncDisposableStack(); s.use(o); s.disposeAsync().then(function(){}); \
             order.join(',')",
        ),
        "\"asyncDispose,dispose\"",
    );
}

/// use：@@asyncDispose getter 只读一次。
#[test]
fn use_reads_async_dispose_once() {
    assert_eq!(
        eval(
            "var count = 0; \
             var o = { get [Symbol.asyncDispose]() { count++; return function() {}; } }; \
             var s = new AsyncDisposableStack(); s.use(o); s.disposeAsync().then(function(){}); count + ''",
        ),
        "\"1\"",
    );
}

/// use：键 11 缺失（null/undefined）且键 12 也缺失 → TypeError；键 11 非 callable
/// → TypeError 且不回落；原始值 TypeError。
#[test]
fn use_invalid_dispose_throws() {
    assert_eq!(
        eval(
            "function kind(fn) { try { var s = new AsyncDisposableStack(); fn(s); s.disposeAsync(); return 'no' } catch (e) { return e.constructor.name } } \
             var noSym = function(s) { s.use({}); }; \
             var nullBoth = function(s) { s.use({ [Symbol.asyncDispose]: null }); }; \
             var nonCallable11 = function(s) { s.use({ [Symbol.asyncDispose]: 42 }); }; \
             var nonCallable12 = function(s) { s.use({ [Symbol.dispose]: 42 }); }; \
             var primitive = function(s) { s.use(42); }; \
             [kind(noSym), kind(nullBoth), kind(nonCallable11), kind(nonCallable12), kind(primitive)].join(',')",
        ),
        "\"TypeError,TypeError,TypeError,TypeError,TypeError\"",
    );
}

/// use(null)/use(undefined)：async 栈必须入栈（返回原值、不抛；disposeAsync 后
/// 资源求值被记录——disposed 翻转，且触发末尾 Await）。
#[test]
fn use_null_undefined_pushed() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); \
             var r1 = s.use(null); var r2 = s.use(undefined); \
             var first = (r1 === null) + '|' + (r2 === undefined) + '|' + s.disposed + '|'; \
             s.disposeAsync().then(function() { return first + s.disposed; })",
        ),
        "\"true|true|false|true\"",
    );
}

/// use：@@asyncDispose 方法被调（this=value）；disposeAsync 结果 promise resolve。
#[test]
fn use_async_method_called() {
    assert_eq!(
        eval(
            "var log = []; \
             var o = { v: 7, async [Symbol.asyncDispose]() { log.push(this.v); } }; \
             var s = new AsyncDisposableStack(); s.use(o); \
             s.disposeAsync().then(function() { return log.join(','); })",
        ),
        "\"7\"",
    );
}

/// use：回落 @@dispose 的同步方法返回值不被 await（计数断言），调用仍发生。
#[test]
fn use_sync_dispose_return_not_awaited() {
    assert_eq!(
        eval(
            "var log = []; \
             var o = { [Symbol.dispose]() { log.push('called'); return Promise.resolve(); } }; \
             var s = new AsyncDisposableStack(); s.use(o); \
             s.disposeAsync().then(function() { return log.join(','); })",
        ),
        "\"called\"",
    );
}

/// adopt/defer：arg-style 调用约定（value 作首参、this=undefined）、返回原值/undefined、
/// 非 callable TypeError。
#[test]
fn adopt_defer_semantics() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); var log = []; \
             var r1 = s.adopt('v', function(a) { 'use strict'; log.push('adopt:' + a + ':' + (this === undefined)); }); \
             var r2 = s.defer(function() { 'use strict'; log.push('defer:' + (this === undefined)); }); \
             var first = (r1 === 'v') + '|' + (r2 === undefined) + '|'; \
             s.disposeAsync().then(function() { return first + log.join(','); })",
        ),
        "\"true|true|defer:true,adopt:v:true\"",
    );
}

/// adopt/defer：非 callable 回调抛 TypeError。
#[test]
fn adopt_defer_non_callable_throws() {
    assert_eq!(
        eval(
            "function k(fn) { try { var s = new AsyncDisposableStack(); fn(s); return 'no' } catch (e) { return e.constructor.name } } \
             k(function(s) { s.adopt(1, 42); }) + ',' + k(function(s) { s.defer(null); })",
        ),
        "\"TypeError,TypeError\"",
    );
}

/// disposeAsync：use/adopt/defer 混栈逆序执行（仿 disposes-resources-in-reverse-order）。
#[test]
fn dispose_async_reverse_order() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); var disposed = []; \
             var r1 = { id: 'r1', async [Symbol.asyncDispose]() { disposed.push(this.id); } }; \
             var r2 = { id: 'r2', [Symbol.dispose]() { disposed.push(this.id); } }; \
             var r3 = { id: 'r3' }; async function d3(res) { disposed.push(res.id); } \
             var r4 = { id: 'r4' }; function d4(res) { disposed.push(res.id); } \
             async function d5() { disposed.push('d5'); } \
             function d6() { disposed.push('d6'); } \
             s.use(r1); s.use(r2); s.adopt(r3, d3); s.adopt(r4, d4); s.defer(d5); s.defer(d6); \
             s.disposeAsync().then(function() { return disposed.join(','); })",
        ),
        "\"d6,d5,r4,r3,r2,r1\"",
    );
}

/// disposeAsync：返回 promise、resolve undefined；已 disposed 后二次调用仍 resolve
/// undefined 且不重入（计数=1）。
#[test]
fn dispose_async_idempotent() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); var n = 0; \
             s.defer(function() { n++; }); \
             var p1 = s.disposeAsync(); var p2 = s.disposeAsync(); \
             var r = (p1 instanceof Promise) + '|' + (p2 instanceof Promise) + '|'; \
             Promise.all([p1, p2]).then(function(v) { return r + (v[0] === undefined) + '|' + (v[1] === undefined) + '|' + n; })",
        ),
        "\"true|true|true|true|1\"",
    );
}

/// disposeAsync：非对象/无槽/同步栈 this → promise reject（而非同步抛）。
#[test]
fn dispose_async_incompatible_receiver_rejects() {
    assert_eq!(
        eval(
            "var da = AsyncDisposableStack.prototype.disposeAsync; \
             var syncStack = new DisposableStack(); \
             function kind(t) { \
               var p = da.call(t); \
               if (!(p instanceof Promise)) return Promise.resolve('not-promise'); \
               return p.then(function() { return 'resolved'; }, function(e) { return e.constructor.name; }); \
             } \
             Promise.all([kind(undefined), kind(42), kind({}), kind(AsyncDisposableStack.prototype), kind(syncStack)]) \
               .then(function(v) { return v.join('|'); })",
        ),
        "\"TypeError|TypeError|TypeError|TypeError|TypeError\"",
    );
}

/// disposeAsync：sets-state-to-disposed——调用后同步置位、await 后保持。
#[test]
fn dispose_async_sets_state_to_disposed() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); \
             var before = s.disposed; \
             var p = s.disposeAsync(); \
             var during = s.disposed; \
             p.then(function() { return before + '|' + during + '|' + s.disposed; })",
        ),
        "\"false|true|true\"",
    );
}

/// disposeAsync：单错原样 reject（同一错误对象引用）。
#[test]
fn dispose_async_single_error_rejects_as_is() {
    assert_eq!(
        eval(
            "var err = new Error('boom'); \
             var s = new AsyncDisposableStack(); s.defer(function() { throw err; }); \
             s.disposeAsync().then(function() { return 'resolved' }, function(e) { return String(e === err); })",
        ),
        "\"true\"",
    );
}

/// disposeAsync：三错合并为 SuppressedError 链（error=后抛、suppressed=前值嵌套）。
#[test]
fn dispose_async_multiple_errors_suppressed_chain() {
    assert_eq!(
        eval(
            "var e1 = new Error('e1'); var e2 = new Error('e2'); var e3 = new Error('e3'); \
             var s = new AsyncDisposableStack(); \
             s.defer(function() { throw e1; }); \
             s.defer(function() { throw e2; }); \
             s.defer(function() { throw e3; }); \
             s.disposeAsync().then(function() { return 'resolved' }, function(e) { \
               return (e instanceof SuppressedError) + '|' + (e.error === e1) + '|' + \
               (e.suppressed.error === e2) + '|' + (e.suppressed.suppressed === e3); })",
        ),
        "\"true|true|true|true\"",
    );
}

/// disposeAsync：真实 async 方法返回 rejected promise → 拒绝原因合并进链。
/// 逆序处理：后入栈的 berr 条目先抛（completion=berr），再遇 aerr 条目 →
/// error=后抛(aerr)、suppressed=前值(berr)。
#[test]
fn dispose_async_rejected_async_method_merged() {
    assert_eq!(
        eval(
            "var aerr = new Error('a'); var berr = new Error('b'); \
             var s = new AsyncDisposableStack(); \
             s.defer(async function() { throw aerr; }); \
             s.defer(function() { return Promise.reject(berr); }); \
             s.disposeAsync().then(function() { return 'resolved' }, function(e) { \
               return (e.error === aerr) + '|' + (e.suppressed === berr); })",
        ),
        "\"true|true\"",
    );
}

/// 微任务序：空栈同步 settle——dispose 恰好排在两个单跳 job 之间。
#[test]
fn explicit_await_skipped_when_empty() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); var seq = []; \
             Promise.all([ \
               Promise.resolve().then(function() { seq.push('job 1'); }), \
               s.disposeAsync().then(function() { seq.push('dispose'); }), \
               Promise.resolve().then(function() { seq.push('job 2'); }) \
             ]).then(function() { return seq.join(','); })",
        ),
        "\"job 1,dispose,job 2\"",
    );
}

/// 微任务序：单 use(null) 栈末尾单次 Await（恰一跳延迟），序仍 job1/dispose/job2。
#[test]
fn explicit_await_for_null() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); s.use(null); var seq = []; \
             Promise.all([ \
               Promise.resolve().then(function() { return 0; }).then(function() { seq.push('job 1'); }), \
               s.disposeAsync().then(function() { seq.push('dispose'); }), \
               Promise.resolve().then(function() { return 0; }).then(function() { seq.push('job 2'); }) \
             ]).then(function() { return seq.join(','); })",
        ),
        "\"job 1,dispose,job 2\"",
    );
}

/// 微任务序：单 use(undefined) 与 use(null) 同语义（末尾补 Await，两跳 job 与
/// test262 explicit-await-for-undefined 逐字等价）。
#[test]
fn explicit_await_for_undefined() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); s.use(undefined); var seq = []; \
             Promise.all([ \
               Promise.resolve().then(function() { return 0; }).then(function() { seq.push('job 1'); }), \
               s.disposeAsync().then(function() { seq.push('dispose'); }), \
               Promise.resolve().then(function() { return 0; }).then(function() { seq.push('job 2'); }) \
             ]).then(function() { return seq.join(','); })",
        ),
        "\"job 1,dispose,job 2\"",
    );
}

/// 微任务序：wrap_sync 条目（use 回落 @@dispose）经 await 记 hasAwaited，随后
/// async-null 条目置 needsAwait 但末尾补 await 被抑制（已 await 过）——顶层同步
/// 阶段即调用方法，dispose 恰在 job 之间。
#[test]
fn explicit_await_wrap_sync_counts_as_awaited() {
    assert_eq!(
        eval(
            "var seq = []; \
             var s = new AsyncDisposableStack(); \
             s.use(null); \
             s.use({ [Symbol.dispose]() { seq.push('sync-res'); } }); \
             var p = s.disposeAsync().then(function() { seq.push('dispose'); }); \
             Promise.resolve().then(function() { seq.push('job 1'); }); \
             p.then(function() { return seq.join(','); })",
        ),
        "\"sync-res,job 1,dispose\"",
    );
}

/// move：entries 转移、源置 Disposed、新栈 Pending 且新栈 disposeAsync 逆序执行。
#[test]
fn move_transfers_entries() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); var log = []; \
             s.defer(function() { log.push('A'); }); \
             var m = s.move(); \
             var first = String(s.disposed) + '|' + String(m.disposed) + '|'; \
             m.disposeAsync().then(function() { return first + log.join(','); })",
        ),
        "\"true|false|A\"",
    );
}

/// move：子类实例 move 结果 proto 固定为 AsyncDisposableStack.prototype（非子类）。
#[test]
fn move_subclass_proto_fixed() {
    assert_eq!(
        eval(
            "class S extends AsyncDisposableStack {} \
             var s = new S(); s.defer(function(){}); \
             var m = s.move(); \
             (m instanceof AsyncDisposableStack) + '|' + (m instanceof S) + '|' + \
             (Object.getPrototypeOf(m) === AsyncDisposableStack.prototype)",
        ),
        "\"true|false|true\"",
    );
}

/// move：disposed 源栈 move 抛 ReferenceError；use/adopt/defer/move 在 disposed 后
/// 均抛 ReferenceError。
#[test]
fn mutators_after_dispose_throw() {
    assert_eq!(
        eval(
            "var s = new AsyncDisposableStack(); \
             var p = s.disposeAsync().then(function() { \
               function k(fn) { try { fn(); return 'no' } catch (e) { return e.constructor.name } } \
               return k(function() { s.use({ [Symbol.asyncDispose]: function(){} }); }) + ',' + \
                      k(function() { s.adopt(1, function(){}); }) + ',' + \
                      k(function() { s.defer(function(){}); }) + ',' + \
                      k(function() { s.move(); }); \
             }); p",
        ),
        "\"ReferenceError,ReferenceError,ReferenceError,ReferenceError\"",
    );
}

/// disposed getter：非对象/无槽/同步栈 this 抛 TypeError。
#[test]
fn disposed_getter_incompatible_throws() {
    assert_eq!(
        eval(
            "function k(t) { try { AsyncDisposableStack.prototype.disposed.call(t); return 'no' } catch (e) { return e.constructor.name } } \
             k({}) + ',' + k(42) + ',' + k(new DisposableStack())",
        ),
        "\"TypeError,TypeError,TypeError\"",
    );
}

/// 低阈值执行期 GC：async 栈 use 资源对象后触发移动式清扫，disposeAsync 仍按
/// entries 正确释放（log 挂 globalThis 属性跨 run 存活）。
#[test]
fn gc_runtime_collection_keeps_entries() {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(4096);
    let mut vm = Vm::with_kernel_core(KernelCore::new(config));
    let allocator = Allocator::default();
    let program = oxide_parser::parse(
        &allocator,
        "globalThis.log = []; \
         for (var i = 0; i < 200; i++) { \
           var s = new AsyncDisposableStack(); \
           s.defer(function() { globalThis.log.push('d'); }); \
           globalThis['k' + i] = s; \
         } \
         for (var i = 0; i < 200; i++) { globalThis['k' + i].disposeAsync().then(function(){}); } \
         0",
    )
    .expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    vm.run(&Arc::new(module)).expect("run1");
    let allocator = Allocator::default();
    let program2 = oxide_parser::parse(&allocator, "globalThis.log.length + ''").expect("parse");
    let module2 = Compiler::new().compile(&program2).expect("compile");
    let result = vm.run(&Arc::new(module2)).expect("run2");
    let text = vm.lookup_str(result).unwrap_or_default().to_string();
    assert_eq!(text, "200");
    assert!(vm.session_gc_stats().total_collections > 0, "低阈值应触发执行期 GC");
}

/// promote 交叉：栈的 entries 引用 epoch 对象（释放函数），写 global 触发 promote，
/// reset 释放 epoch 后状态盒存活：disposed 可读、move 可转移 entries。
#[test]
fn promote_then_reset_keeps_entries_alive() {
    let mut vm = Vm::new();
    let allocator = Allocator::default();
    let program = oxide_parser::parse(
        &allocator,
        "globalThis.log = []; \
         var s = new AsyncDisposableStack(); \
         s.defer(function() { globalThis.log.push('x'); }); \
         globalThis.keep = s; 0",
    )
    .expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    vm.run(&Arc::new(module)).expect("run1");
    vm.reset();
    let allocator = Allocator::default();
    let program2 = oxide_parser::parse(
        &allocator,
        "var m = globalThis.keep.move(); \
         String(globalThis.keep.disposed) + '|' + String(m.disposed)",
    )
    .expect("parse");
    let module2 = Compiler::new().compile(&program2).expect("compile");
    let result = vm.run(&Arc::new(module2)).expect("run2");
    let text = vm.lookup_str(result).unwrap_or_default().to_string();
    assert_eq!(text, "true|false");
}

/// drop 记账：无引用的 async 栈对象被收集时释放状态盒（freed 字节增长）。
#[test]
fn drop_accounts_capability_bytes() {
    let mut vm = Vm::new();
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, "for (var i = 0; i < 500; i++) { new AsyncDisposableStack(); } 0")
        .expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    vm.run(&Arc::new(module)).expect("run");
    let before = vm.session_gc_stats().total_bytes_freed;
    vm.reset();
    let after = vm.session_gc_stats().total_bytes_freed;
    assert!(after > before, "reset 应回收栈对象状态盒，before={before} after={after}");
}

/// full_reset（dirty 重建）：global 与 object 家族重建后 AsyncDisposableStack 仍
/// 完整可用——全局槽、proto.constructor、原型方法、instanceof 与原型链全部存活，
/// disposeAsync 可再次执行。
#[test]
fn full_reset_rebuilds_async_disposable_stack() {
    let mut vm = Vm::new();
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, "new AsyncDisposableStack().disposeAsync(); 0").expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    vm.run(&Arc::new(module)).expect("run1");
    vm.full_reset();
    let allocator = Allocator::default();
    let program2 = oxide_parser::parse(
        &allocator,
        "var s = new AsyncDisposableStack(); var log = []; \
         s.defer(function() { log.push('A'); }); \
         var r = (s instanceof AsyncDisposableStack) + '|' + \
                 (AsyncDisposableStack.prototype.constructor === AsyncDisposableStack) + '|' + \
                 (AsyncDisposableStack.prototype[Symbol.asyncDispose] === AsyncDisposableStack.prototype.disposeAsync) + '|' + \
                 (AsyncDisposableStack.prototype[Symbol.toStringTag] === 'AsyncDisposableStack') + '|'; \
         s.disposeAsync().then(function() { return r + log.join(','); })",
    )
    .expect("parse");
    let module2 = Compiler::new().compile(&program2).expect("compile");
    let result = vm.run(&Arc::new(module2)).expect("run2");
    let text = match promise_settled_value(unsafe { &*result.as_js_object_ptr() }) {
        Some((true, v)) => vm.lookup_str(v).unwrap_or_default().to_string(),
        Some((false, v)) => format!("<rejected {}>", vm.lookup_str(v).unwrap_or_default()),
        None => "<pending>".to_string(),
    };
    assert_eq!(text, "true|true|true|true|A");
}
