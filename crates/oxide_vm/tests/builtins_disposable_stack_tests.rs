use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm::Vm;

fn make_vm() -> Vm {
    Vm::new()
}

fn eval_in(vm: &mut Vm, source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    vm.run(&module)
}

/// 断言 eval 返回字符串值（session 串在 Vm 内解析）。
fn eval_str(vm: &mut Vm, source: &str) -> String {
    let result = eval_in(vm, source).expect("run");
    vm.lookup_str(result).unwrap_or_default().to_string()
}

fn eval_str_fresh(source: &str) -> String {
    let mut vm = make_vm();
    eval_str(&mut vm, source)
}

/// 构造器基本形状：new 建对象、disposed=false、proto/instanceof、原型链到 Object.prototype。
#[test]
fn constructor_shapes() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); \
             s.disposed + '|' + (Object.getPrototypeOf(s) === DisposableStack.prototype) + '|' + \
             (s instanceof DisposableStack) + '|' + \
             (Object.getPrototypeOf(DisposableStack.prototype) === Object.prototype)",
        ),
        "false|true|true|true",
    );
}

/// 构造器 length/name 与 prototype 槽描述符 {f,f,f}、proto.constructor={t,f,t}。
#[test]
fn constructor_identity() {
    assert_eq!(
        eval_str_fresh(
            "var p = Object.getOwnPropertyDescriptor(DisposableStack, 'prototype'); \
             var c = Object.getOwnPropertyDescriptor(DisposableStack.prototype, 'constructor'); \
             DisposableStack.length + '|' + DisposableStack.name + '|' + \
             p.writable + '|' + p.enumerable + '|' + p.configurable + '|' + \
             c.writable + '|' + c.enumerable + '|' + c.configurable + '|' + \
             (DisposableStack.prototype.constructor === DisposableStack)",
        ),
        "0|DisposableStack|false|false|false|true|false|true|true",
    );
}

/// 普通调用（无 new）抛 TypeError。
#[test]
fn constructor_without_new_throws() {
    assert_eq!(
        eval_str_fresh("try { DisposableStack(); 'no-throw' } catch (e) { e.constructor.name }",),
        "TypeError",
    );
}

/// use：push + dispose 以 receiver=value、无参调用；返回原 value。
#[test]
fn use_pushes_and_disposes_with_receiver() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             var o = { v: 7 }; \
             o[Symbol.dispose] = function() { log.push(this.v); }; \
             var r = s.use(o); \
             (r === o) + '|' + s.dispose() + '|' + log.join(',')",
        ),
        "true|undefined|7",
    );
}

/// use：@@dispose getter 只读一次（不得重复读）。
#[test]
fn use_reads_dispose_once() {
    assert_eq!(
        eval_str_fresh(
            "var count = 0; \
             var o = { get [Symbol.dispose]() { count++; return function() {}; } }; \
             var s = new DisposableStack(); s.use(o); s.dispose(); count + ''",
        ),
        "1",
    );
}

/// use(null)/use(undefined)：返回原值且不入栈（dispose 无任何调用）。
#[test]
fn use_null_undefined_not_pushed() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             var r1 = s.use(null); var r2 = s.use(undefined); \
             s.dispose(); log.join(',') + '|' + (r1 === null) + '|' + (r2 === undefined)",
        ),
        "|true|true",
    );
}

/// use：原始值 / 缺 / null / undefined / 非 callable 的 @@dispose 一律 TypeError。
#[test]
fn use_invalid_dispose_throws() {
    assert_eq!(
        eval_str_fresh(
            "function kind(fn) { try { var s = new DisposableStack(); fn(s); s.dispose(); return 'no' } catch (e) { return e.constructor.name } } \
             var noSym = function(s) { s.use({}); }; \
             var nullSym = function(s) { s.use({ [Symbol.dispose]: null }); }; \
             var undefSym = function(s) { s.use({ [Symbol.dispose]: undefined }); }; \
             var nonCallable = function(s) { s.use({ [Symbol.dispose]: 42 }); }; \
             var primitive = function(s) { s.use(42); }; \
             [kind(noSym), kind(nullSym), kind(undefSym), kind(nonCallable), kind(primitive)].join(',')",
        ),
        "TypeError,TypeError,TypeError,TypeError,TypeError",
    );
}

/// use/adopt/defer 在 disposed 后调用抛 ReferenceError。
#[test]
fn mutators_after_dispose_throw() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); s.dispose(); \
             function k(fn) { try { fn(); return 'no' } catch (e) { return e.constructor.name } } \
             k(function() { s.use({ [Symbol.dispose]: function(){} }); }) + ',' + \
             k(function() { s.adopt(1, function(){}); }) + ',' + \
             k(function() { s.defer(function(){}); }) + ',' + \
             k(function() { s.move(); })",
        ),
        "ReferenceError,ReferenceError,ReferenceError,ReferenceError",
    );
}

/// use 在非栈 this（普通对象/原始值）上调用抛 TypeError。
#[test]
fn use_incompatible_receiver_throws() {
    assert_eq!(
        eval_str_fresh(
            "function k(t) { try { DisposableStack.prototype.use.call(t, { [Symbol.dispose]: function(){} }); return 'no' } catch (e) { return e.constructor.name } } \
             k({}) + ',' + k(42)",
        ),
        "TypeError,TypeError",
    );
}

/// adopt：任意 value（含原始值）接受并返回；dispose 时 arg-style 调用
/// （value 作首参、this 保持 undefined——严格模式可观察）。
#[test]
fn adopt_arg_style_semantics() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             var r = s.adopt('v', function(a) { 'use strict'; log.push(a + ':' + (this === undefined)); }); \
             (r === 'v') + '|' + s.dispose() + '|' + log.join(',')",
        ),
        "true|undefined|v:true",
    );
}

/// adopt：非 callable 回调抛 TypeError。
#[test]
fn adopt_non_callable_throws() {
    assert_eq!(
        eval_str_fresh("try { var s = new DisposableStack(); s.adopt(1, 42); 'no' } catch (e) { e.constructor.name }",),
        "TypeError",
    );
}

/// defer：返回 undefined、dispose 时以 undefined receiver 无参调用。
#[test]
fn defer_semantics() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             var r = s.defer(function() { 'use strict'; log.push('this:' + (this === undefined)); }); \
             (r === undefined) + '|' + s.dispose() + '|' + log.join(',')",
        ),
        "true|undefined|this:true",
    );
}

/// defer：非 callable 抛 TypeError。
#[test]
fn defer_non_callable_throws() {
    assert_eq!(
        eval_str_fresh("try { var s = new DisposableStack(); s.defer(null); 'no' } catch (e) { e.constructor.name }",),
        "TypeError",
    );
}

/// dispose：use/adopt/defer 混栈逆序执行（后入先释放）。
#[test]
fn dispose_reverse_order() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             s.defer(function() { log.push('A'); }); \
             s.adopt(1, function() { log.push('B'); }); \
             s.use({ [Symbol.dispose]: function() { log.push('C'); } }); \
             s.dispose(); log.join(',')",
        ),
        "C,B,A",
    );
}

/// dispose：二次调用幂等（回调不重入），disposed 变 true。
#[test]
fn dispose_idempotent() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var n = 0; \
             s.defer(function() { n++; }); \
             s.dispose(); s.dispose(); n + '|' + s.disposed",
        ),
        "1|true",
    );
}

/// dispose：单错原样抛（同一错误对象引用）。
#[test]
fn dispose_single_error_preserved() {
    assert_eq!(
        eval_str_fresh(
            "var err = new Error('boom'); \
             var s = new DisposableStack(); s.defer(function() { throw err; }); \
             try { s.dispose(); 'no' } catch (e) { String(e === err) }",
        ),
        "true",
    );
}

/// dispose：三错合并为 SuppressedError 链（error=后抛、suppressed=前值嵌套）。
#[test]
fn dispose_multiple_errors_suppressed_chain() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); \
             s.defer(function() { throw new Error('e1'); }); \
             s.defer(function() { throw new Error('e2'); }); \
             s.defer(function() { throw new Error('e3'); }); \
             try { s.dispose(); 'no' } catch (e) { \
               (e instanceof SuppressedError) + '|' + e.error.message + '|' + \
               e.suppressed.error.message + '|' + e.suppressed.suppressed.message }",
        ),
        "true|e1|e2|e3",
    );
}

/// dispose：错误不中断循环，剩余条目全部执行。
#[test]
fn dispose_error_does_not_abort_loop() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             s.defer(function() { log.push('A'); }); \
             s.defer(function() { log.push('B'); throw new Error('x'); }); \
             try { s.dispose(); 'no' } catch (e) { log.join(',') + ':' + e.message }",
        ),
        "B,A:x",
    );
}

/// dispose：执行中调用 use/adopt/defer/move 均 ReferenceError。
#[test]
fn dispose_reentrancy_guard() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             s.defer(function() { \
               try { s.use({ [Symbol.dispose]: function(){} }); log.push('use:no'); } catch (e) { log.push(e.constructor.name); } \
               try { s.adopt(1, function(){}); log.push('adopt:no'); } catch (e) { log.push(e.constructor.name); } \
               try { s.defer(function(){}); log.push('defer:no'); } catch (e) { log.push(e.constructor.name); } \
               try { s.move(); log.push('move:no'); } catch (e) { log.push(e.constructor.name); } \
             }); \
             s.dispose(); log.join(',')",
        ),
        "ReferenceError,ReferenceError,ReferenceError,ReferenceError",
    );
}

/// move：entries 转移、源置 Disposed、新栈 Pending 且可继续 dispose。
#[test]
fn move_transfers_entries() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             s.defer(function() { log.push('A'); }); \
             var m = s.move(); \
             var first = String(s.disposed) + '|' + String(m.disposed) + '|'; \
             m.dispose(); first + log.join(',')",
        ),
        "true|false|A",
    );
}

/// move：源栈 dispose 幂等（不执行资源回调、不抛错），新栈独占执行。
#[test]
fn move_source_disposed() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             s.defer(function() { log.push('A'); }); \
             var m = s.move(); \
             s.dispose(); log.push('src:ok'); \
             m.dispose(); log.join(',')",
        ),
        "src:ok,A",
    );
}

/// move：子类实例 move 结果 proto 固定为 DisposableStack.prototype（非子类）。
#[test]
fn move_subclass_proto_fixed() {
    assert_eq!(
        eval_str_fresh(
            "class S extends DisposableStack {} \
             var s = new S(); s.defer(function(){}); \
             var m = s.move(); \
             (m instanceof DisposableStack) + '|' + (m instanceof S) + '|' + \
             (Object.getPrototypeOf(m) === DisposableStack.prototype)",
        ),
        "true|false|true",
    );
}

/// disposed getter：非对象/无槽 this 抛 TypeError。
#[test]
fn disposed_getter_incompatible_throws() {
    assert_eq!(
        eval_str_fresh(
            "function k(t) { try { DisposableStack.prototype.disposed.call(t); return 'no' } catch (e) { return e.constructor.name } } \
             k({}) + ',' + k(42)",
        ),
        "TypeError,TypeError",
    );
}

/// 原型形状：@@dispose 别名同一函数对象、@@toStringTag 值+描述符 {f,f,t}、
/// 方法 length/name/{t,f,t}、disposed 为访问器 getter。
#[test]
fn prototype_shape() {
    assert_eq!(
        eval_str_fresh(
            "var p = DisposableStack.prototype; \
             var tag = Object.getOwnPropertyDescriptor(p, Symbol.toStringTag); \
             var use = Object.getOwnPropertyDescriptor(p, 'use'); \
             var disp = Object.getOwnPropertyDescriptor(p, 'dispose'); \
             var acc = Object.getOwnPropertyDescriptor(p, 'disposed'); \
             (p[Symbol.dispose] === p.dispose) + '|' + \
             Object.prototype.toString.call(new DisposableStack()) + '|' + \
             tag.value + '|' + tag.writable + '|' + tag.enumerable + '|' + tag.configurable + '|' + \
             use.value.length + '|' + use.value.name + '|' + use.writable + '|' + use.enumerable + '|' + use.configurable + '|' + \
             disp.value.length + '|' + disp.value.name + '|' + \
             (typeof acc.get === 'function') + '|' + (acc.set === undefined)",
        ),
        "true|[object DisposableStack]|DisposableStack|false|false|true|1|use|true|false|true|0|dispose|true|true",
    );
}

/// 原型形状：Symbol.dispose 键读取命中资源方法（use 读 @@dispose 的实际路径）。
#[test]
fn use_reads_symbol_dispose_key() {
    assert_eq!(
        eval_str_fresh(
            "var s = new DisposableStack(); var log = []; \
             var o = {}; \
             Object.defineProperty(o, Symbol.dispose, { value: function() { log.push('via-symbol'); } }); \
             s.use(o); s.dispose(); log.join(',')",
        ),
        "via-symbol",
    );
}

/// 低阈值执行期 GC：栈 use 资源对象后触发移动式清扫，dispose 仍按 entries 正确释放。
#[test]
fn gc_runtime_collection_keeps_entries() {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(4096);
    let mut vm = Vm::with_kernel_core(KernelCore::new(config));
    let text = eval_str(
        &mut vm,
        "var log = []; \
         for (var i = 0; i < 300; i++) { \
           var s = new DisposableStack(); \
           s.use({ [Symbol.dispose]: function() { log.push('d'); } }); \
           globalThis['k' + i] = s; \
         } \
         for (var i = 0; i < 300; i++) { globalThis['k' + i].dispose(); } \
         log.length + ''",
    );
    assert_eq!(text, "300");
    assert!(vm.session_gc_stats().total_collections > 0, "低阈值应触发执行期 GC");
}

/// promote 交叉：栈的 entries 引用 epoch 对象（value 对象 + 释放函数），写 global
/// 触发 promote，reset 释放 epoch 后状态盒存活：disposed 可读、move 可转移 entries。
/// （释放函数对象跨 run 调用受引擎 sub_module 索引边界限制，不在此断言。）
#[test]
fn promote_then_reset_keeps_entries_alive() {
    let mut vm = make_vm();
    eval_in(
        &mut vm,
        "globalThis.log = []; \
         var s = new DisposableStack(); \
         s.use({ v: 1, [Symbol.dispose]: function() { globalThis.log.push('d'); } }); \
         s.defer(function() { globalThis.log.push('x'); }); \
         globalThis.keep = s; 0",
    )
    .expect("run1");
    vm.reset();
    let text = eval_str(
        &mut vm,
        "var m = globalThis.keep.move(); \
         String(globalThis.keep.disposed) + '|' + String(m.disposed)",
    );
    assert_eq!(text, "true|false");
}

/// drop 记账：无引用的栈对象被收集时释放状态盒（freed 字节增长）。
#[test]
fn drop_accounts_capability_bytes() {
    let mut vm = make_vm();
    eval_in(&mut vm, "for (var i = 0; i < 500; i++) { new DisposableStack(); } 0").expect("run");
    let before = vm.session_gc_stats().total_bytes_freed;
    vm.reset();
    let after = vm.session_gc_stats().total_bytes_freed;
    assert!(after > before, "reset 应回收栈对象状态盒，before={before} after={after}");
}
