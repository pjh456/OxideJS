use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

/// 五 stub 构造器原型链钉：prototype 为对象、原型链落 Object.prototype、
/// constructor 回指构造器本体。
#[test]
fn stub_ctor_prototype_chain_all_five() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var names = ['Proxy', 'WeakMap', 'WeakSet', 'WeakRef', 'FinalizationRegistry']; \
         for (var i = 0; i < names.length; i++) { \
            var x = globalThis[names[i]]; \
            if (typeof x.prototype !== 'object') { return false; } \
            if (Object.getPrototypeOf(x.prototype) !== Object.prototype) { return false; } \
            if (x.prototype.constructor !== x) { return false; } \
         } \
         return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 五 stub 构造器 length/name 值钉 + 非枚举钉：构造器面三属性与原型对象自身
/// 属性均不泄漏进 Object.keys。
#[test]
fn stub_ctor_length_name_identity_non_enumerable() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var names = ['Proxy', 'WeakMap', 'WeakSet', 'WeakRef', 'FinalizationRegistry']; \
         var expect = { Proxy: 2, WeakMap: 0, WeakSet: 0, WeakRef: 1, FinalizationRegistry: 1 }; \
         for (var i = 0; i < names.length; i++) { \
            var name = names[i]; \
            var x = globalThis[name]; \
            if (x.length !== expect[name] || x.name !== name) { return false; } \
            var keys = Object.keys(x); \
            for (var k = 0; k < keys.length; k++) { \
               if (keys[k] === 'length' || keys[k] === 'name' || keys[k] === 'prototype') { return false; } \
            } \
            if (Object.keys(x.prototype).length !== 0) { return false; } \
         } \
         return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 四弱族原型对象 @@toStringTag 钉；Proxy 原型无规范 tag，不钉。
#[test]
fn weak_family_proto_to_string_tag() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         var names = ['WeakMap', 'WeakSet', 'WeakRef', 'FinalizationRegistry']; \
         for (var i = 0; i < names.length; i++) { \
            if (globalThis[names[i]].prototype[Symbol.toStringTag] !== names[i]) { return false; } \
         } \
         return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 原型成员读不抛钉：缺失方法读返 undefined（非对象 receiver 抛错形态消除）。
#[test]
fn stub_proto_member_read_returns_undefined() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         return typeof WeakMap.prototype.get === 'undefined' \
            && typeof WeakSet.prototype.add === 'undefined' \
            && typeof WeakRef.prototype.deref === 'undefined' \
            && typeof FinalizationRegistry.prototype.register === 'undefined' \
            && typeof Proxy.prototype.nonexistent === 'undefined'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 构造体不回归钉：五 stub 构造调用仍抛 TypeError（消息含 "… is not implemented"）。
#[test]
fn stub_ctor_call_still_throws_not_implemented() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         function probe(ctor, args, name) { \
            try { Reflect.construct(ctor, args, ctor); } \
            catch (e) { return e instanceof TypeError && e.message.indexOf(name + ' is not implemented') === 0; } \
            return false; \
         } \
         return probe(WeakMap, [], 'WeakMap') \
            && probe(WeakSet, [], 'WeakSet') \
            && probe(WeakRef, [{}], 'WeakRef') \
            && probe(FinalizationRegistry, [function () {}], 'FinalizationRegistry') \
            && probe(Proxy, [ {}, {} ], 'Proxy'); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// isConstructor 探测臂钉：Reflect.construct 以 stub 作 newTarget 经真 prototype
/// 走通（构造体抛错由空函数体规避），结果恒 true。
#[test]
fn stub_ctor_is_constructor_probe_true() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
         function isCtor(f) { \
            try { Reflect.construct(function () {}, [], f); } catch (e) { return false; } \
            return true; \
         } \
         return isCtor(Proxy) && isCtor(WeakMap) && isCtor(WeakSet) \
            && isCtor(WeakRef) && isCtor(FinalizationRegistry); })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// GC 边钉：低阈值 + 长循环触发执行期回收后，stub 原型对象经 stub 属性边存活，
/// 原型链与 constructor 回指身份不变。
#[test]
fn stub_proto_survives_runtime_gc() {
    let mut config = KernelConfig::minimal();
    config.set_session_gc_threshold(4096);
    let mut vm = Vm::with_kernel_core(KernelCore::new(config));
    let result = eval(
        &mut vm,
        "(function () { \
         var p = WeakMap.prototype; \
         var c = p.constructor; \
         var s; \
         for (var i = 0; i < 500000; i++) { s = 'str' + i; } \
         return p === WeakMap.prototype && c === WeakMap \
            && Object.getPrototypeOf(p) === Object.prototype; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}
