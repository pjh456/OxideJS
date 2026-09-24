//! WeakMap 构造体与四方法钉：NewTarget 抛面、iterable 协议步序、品牌三分枝
//! （set/delete 对非 WeakMap this 抛 TypeError，get/has 静默返 undefined/false）、
//! 键面（对象/symbol 可弱持，原始值 set 抛、get/has/delete 静默）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

/// 构造体身份钉：new 形态实例原型链落 WeakMap.prototype、instanceof 成立；
/// 普通调用（无 NewTarget）抛 TypeError。
#[test]
fn weakmap_ctor_instance_and_newtarget_throw() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
           var m = new WeakMap(); \
           if (Object.getPrototypeOf(m) !== WeakMap.prototype) { return false; } \
           if (!(m instanceof WeakMap)) { return false; } \
           if (WeakMap.name !== 'WeakMap' || WeakMap.length !== 0) { return false; } \
           try { WeakMap(); return false; } \
           catch (e) { if (!(e instanceof TypeError)) { return false; } } \
           try { WeakMap([]); return false; } \
           catch (e) { if (!(e instanceof TypeError)) { return false; } } \
           return true; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// iterable 协议钉：对象键与 symbol 键逐元素 set；空可迭代不调用原型 set
/// （覆写计数器探针）；对象形态 entry（{0,1} 属性）同命中。
#[test]
fn weakmap_ctor_iterable_protocol() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
           var k1 = {}, k2 = {}, ks = Symbol('key'); \
           var m = new WeakMap([[k1, 'a'], [k2, 'b'], [ks, 's']]); \
           if (m.get(k1) !== 'a' || m.get(k2) !== 'b' || m.get(ks) !== 's') { return false; } \
           var count = 0; \
           var saved = WeakMap.prototype.set; \
           WeakMap.prototype.set = function () { count += 1; }; \
           try { \
              new WeakMap([]); \
           } finally { \
              WeakMap.prototype.set = saved; \
           } \
           if (count !== 0) { return false; } \
           var m2 = new WeakMap([{0: k1, 1: 'c'}]); \
           return m2.get(k1) === 'c'; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// iterable 失败面钉：set 方法读取出抛原样透传；set 不可调用抛 TypeError；
/// 元素非对象抛 TypeError；set 抛错时迭代器经 IteratorClose 恰好关闭一次。
#[test]
fn weakmap_ctor_iterable_failure_branches() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
           var getter = Object.getOwnPropertyDescriptor(WeakMap.prototype, 'set'); \
           Object.defineProperty(WeakMap.prototype, 'set', { \
              get: function () { throw new RangeError('boom'); }, \
              configurable: true \
           }); \
           try { new WeakMap([]); return false; } \
           catch (e) { if (!(e instanceof RangeError)) { return false; } } \
           finally { Object.defineProperty(WeakMap.prototype, 'set', getter); } \
           WeakMap.prototype.set = 'not a function'; \
           try { new WeakMap([[{}, 1]]); return false; } \
           catch (e) { if (!(e instanceof TypeError)) { return false; } } \
           finally { delete WeakMap.prototype.set; } \
           try { new WeakMap([42]); return false; } \
           catch (e) { if (!(e instanceof TypeError)) { return false; } } \
           var closed = 0; \
           var k = {}; \
           var it = { \
              n: 0, \
              next: function () { \
                 this.n += 1; \
                 if (this.n === 1) { return { value: [k, 1], done: false }; } \
                 return { done: true }; \
              }, \
              return: function () { closed += 1; } \
           }; \
           var iterable = {}; \
           iterable[Symbol.iterator] = function () { return it; }; \
           WeakMap.prototype.set = function () { throw new Error('x'); }; \
           try { new WeakMap(iterable); } catch (e) {} \
           delete WeakMap.prototype.set; \
           return closed === 1; })()",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 品牌守卫钉：四方法对非 WeakMap this（plain 对象 / Map 对象 / 各原始 this）
/// 均抛 TypeError（`RequireInternalSlot` 语义）；合法 this 下未命中键静默返
/// undefined/false 且 set 返回 this。
#[test]
fn weakmap_method_brand_guard() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
           var bad = [{}, new Map(), undefined, null, true, 's', 5, Symbol('t')]; \
           for (var i = 0; i < bad.length; i++) { \
              try { WeakMap.prototype.set.call(bad[i], {}, 1); return 'set-' + i; } \
              catch (e) { if (!(e instanceof TypeError)) { return 'set-kind-' + i; } } \
              try { WeakMap.prototype.delete.call(bad[i], {}); return 'del-' + i; } \
              catch (e) { if (!(e instanceof TypeError)) { return 'del-kind-' + i; } } \
              try { WeakMap.prototype.get.call(bad[i], {}); return 'get-' + i; } \
              catch (e) { if (!(e instanceof TypeError)) { return 'get-kind-' + i; } } \
              try { WeakMap.prototype.has.call(bad[i], {}); return 'has-' + i; } \
              catch (e) { if (!(e instanceof TypeError)) { return 'has-kind-' + i; } } \
           } \
           var m = new WeakMap(); \
           if (WeakMap.prototype.set.call(m, {}, 'v') !== m) { return 'returns-this'; } \
           if (m.get({}) !== undefined) { return 'fresh-get'; } \
           if (m.has({}) !== false) { return 'fresh-has'; } \
           if (m.delete({}) !== false) { return 'fresh-delete'; } \
           return true; })()",
    )
    .unwrap();
    assert_eq!(result, JsValue::bool(true));
}

/// 键面钉：原始值键 set 抛 TypeError；symbol 键经 set/get/has/delete 全链
/// 成立（弱键 symbol 臂）；原始值键 get/has/delete 静默。
#[test]
fn weakmap_key_surface_object_and_symbol() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(function () { \
           var m = new WeakMap(); \
           var badKeys = [1, '', undefined, null, true]; \
           for (var i = 0; i < badKeys.length; i++) { \
              try { m.set(badKeys[i], 1); return 'set-' + i; } \
              catch (e) { if (!(e instanceof TypeError)) { return 'set-kind-' + i; } } \
              if (m.get(badKeys[i]) !== undefined) { return 'get-' + i; } \
              if (m.has(badKeys[i]) !== false) { return 'has-' + i; } \
              if (m.delete(badKeys[i]) !== false) { return 'del-' + i; } \
           } \
           var s = Symbol('k'); \
           m.set(s, 'v'); \
           if (m.get(s) !== 'v' || !m.has(s)) { return 'symbol-roundtrip'; } \
           m.set(s, 'v2'); \
           if (m.get(s) !== 'v2') { return 'symbol-overwrite'; } \
           if (m.delete(s) !== true || m.has(s)) { return 'symbol-delete'; } \
           return true; })()",
    )
    .unwrap();
    assert_eq!(result, JsValue::bool(true));
}
