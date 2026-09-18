//! `Iterator.from` / `Array.from` 的原始值迭代器面钉：String 臂经 GetMethod
//! 以原始值 receiver 读 `@@iterator`（用户覆盖 getter 观测 `typeof this`），
//! peek 与 get_iterator 对非字符串原始值经 ToObject 得查找起点、receiver 保持
//! 原始值（GetV / GetIteratorFromMethod 语义），`Iterator.from` 对非字符串原始值
//! 非对象按 GetIteratorFlattenable 抛 TypeError（区别于 `Array.from` 的装箱路径）。
//!
//! 观察者一律用 `'use strict'`：引擎对 sloppy 函数的原始 this 不做 ToObject
//! （既有债），sloppy 观察者不反映本组 GetV receiver 语义。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn run(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

/// 断言 IIFE 求值为 `true`（把多断言收敛为布尔，规避 JsValue Display 只暴露
/// number/bool 的限制）。
fn assert_truthy(name: &str, source: &str) {
    let result = run(source).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(result.as_bool(), "{name}: expected truthy, got {:?}", result);
}

#[test]
fn iterator_from_string_getter_sees_raw_string() {
    // 原始字符串 receiver：getter 的 typeof this 为 'string'（不装箱）；
    // 包装字符串对象 receiver 为 'object'。
    assert_truthy(
        "string getter receiver",
        "(function(){
           var orig = String.prototype[Symbol.iterator];
           var obs;
           Object.defineProperty(String.prototype, Symbol.iterator, {
             get(){ 'use strict'; obs = typeof this; return orig; }
           });
           Iterator.from('');
           var raw = obs;
           Iterator.from(new String(''));
           var boxed = obs;
           return raw === 'string' && boxed === 'object';
         })()",
    );
}

#[test]
fn array_from_number_boxes_to_read_number_iterator() {
    // Array.from(5) 经 GetIterator 以原始值 receiver 调用
    // Number.prototype[Symbol.iterator]（装箱对象只作查找起点）。
    assert_truthy(
        "Array.from(5)",
        "(function(){
           Number.prototype[Symbol.iterator] = function*(){
             var i = 0, t = this >>> 0;
             while (i < t) { yield i; ++i; }
           };
           var r = Array.from(5);
           return r.length === 5 && r[0] === 0 && r[1] === 1 && r[4] === 4;
         })()",
    );
}

#[test]
fn iterator_from_number_throws_type_error() {
    // Iterator.from(5)：非字符串原始值非对象，GetIteratorFlattenable 抛 TypeError
    // （不装箱），即便 Number.prototype 已覆盖。
    assert_truthy(
        "Iterator.from(5) throws",
        "(function(){
           Number.prototype[Symbol.iterator] = function*(){ yield 1; };
           try { Iterator.from(5); return false; }
           catch (e) { return e instanceof TypeError; }
         })()",
    );
}

#[test]
fn array_from_iterator_from_boxed_number() {
    // new Number(5) 是对象：Iterator.from 经 GetIteratorFlattenable 走覆盖迭代器。
    assert_truthy(
        "Array.from(Iterator.from(new Number(5)))",
        "(function(){
           Number.prototype[Symbol.iterator] = function*(){
             var i = 0, t = this >>> 0;
             while (i < t) { yield i; ++i; }
           };
           var r = Array.from(Iterator.from(new Number(5)));
           return r.length === 5 && r[0] === 0 && r[4] === 4;
         })()",
    );
}

#[test]
fn array_from_iterator_from_string_yields_chars() {
    // Iterator.from 原始字符串走 String 臂码元快速路径，产出字符序列。
    assert_truthy(
        "Array.from(Iterator.from('string'))",
        "(function(){
           var r = Array.from(Iterator.from('string'));
           return r.length === 6 && r[0] === 's' && r[5] === 'g';
         })()",
    );
}

#[test]
fn array_from_string_default_no_override() {
    // 无覆盖回归：默认字符串迭代器仍走码元快速路径，逐个字符产出。
    assert_truthy(
        "Array.from('abc') default",
        "(function(){
           var r = Array.from('abc');
           return r.length === 3 && r[0] === 'a' && r[1] === 'b' && r[2] === 'c';
         })()",
    );
}

#[test]
fn for_of_string_default_no_override() {
    // 无覆盖回归：for-of 字符串仍逐字符迭代（默认快速路径未被破坏）。
    assert_truthy(
        "for-of 'hix' default",
        "(function(){
           var n = 0;
           for (var c of 'hix') { n++; }
           return n === 3;
         })()",
    );
}

#[test]
fn array_from_number_strict_observer_sees_raw_receiver() {
    // Array.from(5) 同经 peek 与 get_iterator：strict getter 与 strict 方法的
    // this 均观测原始 number（装箱对象只作查找起点，receiver 保持原始值）。
    assert_truthy(
        "Array.from(5) raw receiver",
        "(function(){
           var getThis, callThis;
           function iter(){ 'use strict'; callThis = typeof this; return [1][Symbol.iterator](); }
           Object.defineProperty(Number.prototype, Symbol.iterator, {
             configurable: true,
             get(){ 'use strict'; getThis = typeof this; return iter; }
           });
           Array.from(5);
           return getThis === 'number' && callThis === 'number';
         })()",
    );
}

#[test]
fn for_of_number_strict_observer_sees_raw_receiver() {
    // for-of 只经 get_iterator：strict getter 与被调方法的 this 均为原始 number。
    assert_truthy(
        "for-of 5 raw receiver",
        "(function(){
           var getThis, callThis;
           function iter(){ 'use strict'; callThis = typeof this; return [1][Symbol.iterator](); }
           Object.defineProperty(Number.prototype, Symbol.iterator, {
             configurable: true,
             get(){ 'use strict'; getThis = typeof this; return iter; }
           });
           var n = 0;
           for (var x of 5) { n++; }
           return getThis === 'number' && callThis === 'number' && n === 1;
         })()",
    );
}

#[test]
fn spread_number_strict_observer_sees_raw_receiver() {
    // 数组展开 `[...5]` 只经 get_iterator：strict getter 与方法的 this 均为原始 number。
    assert_truthy(
        "[...5] raw receiver",
        "(function(){
           var getThis, callThis;
           function iter(){ 'use strict'; callThis = typeof this; return [1][Symbol.iterator](); }
           Object.defineProperty(Number.prototype, Symbol.iterator, {
             configurable: true,
             get(){ 'use strict'; getThis = typeof this; return iter; }
           });
           var r = [...5];
           return getThis === 'number' && callThis === 'number' && r.length === 1 && r[0] === 1;
         })()",
    );
}

#[test]
fn array_from_primitive_types_strict_observer_sees_raw_receiver() {
    // boolean/bigint/symbol 原始值：getter 与被调方法的 this 观测各自原始类型。
    assert_truthy(
        "Array.from(true/5n/Symbol()) raw receivers",
        "(function(){
           function observe(proto, value){
             var getThis, callThis;
             function iter(){ 'use strict'; callThis = typeof this; return [1][Symbol.iterator](); }
             Object.defineProperty(proto, Symbol.iterator, {
               configurable: true,
               get(){ 'use strict'; getThis = typeof this; return iter; }
             });
             Array.from(value);
             return getThis + '|' + callThis;
           }
           return observe(Boolean.prototype, true) === 'boolean|boolean'
               && observe(BigInt.prototype, 5n) === 'bigint|bigint'
               && observe(Symbol.prototype, Symbol()) === 'symbol|symbol';
         })()",
    );
}

#[test]
fn typed_array_from_number_strict_observer_sees_raw_receiver() {
    // %TypedArray%.from 亦经 peek + get_iterator：strict this 观测原始 number。
    assert_truthy(
        "Int8Array.from(5) raw receiver",
        "(function(){
           var getThis, callThis;
           function iter(){ 'use strict'; callThis = typeof this; return [1][Symbol.iterator](); }
           Object.defineProperty(Number.prototype, Symbol.iterator, {
             configurable: true,
             get(){ 'use strict'; getThis = typeof this; return iter; }
           });
           var r = Int8Array.from(5);
           return getThis === 'number' && callThis === 'number' && r.length === 1 && r[0] === 1;
         })()",
    );
}

#[test]
fn array_from_boxed_objects_strict_observer_sees_object_receiver() {
    // 装箱对象（new Number / new String）的 receiver 仍为对象：装箱面不回归。
    assert_truthy(
        "Array.from(new Number(5))/new String('ab') object receivers",
        "(function(){
           function observe(proto, value){
             var getThis, callThis;
             function iter(){ 'use strict'; callThis = typeof this; return [1][Symbol.iterator](); }
             Object.defineProperty(proto, Symbol.iterator, {
               configurable: true,
               get(){ 'use strict'; getThis = typeof this; return iter; }
             });
             Array.from(value);
             return getThis + '|' + callThis;
           }
           return observe(Number.prototype, new Number(5)) === 'object|object'
               && observe(String.prototype, new String('ab')) === 'object|object';
         })()",
    );
}

#[test]
fn primitive_iterator_default_paths_regression() {
    // 无覆盖时非字符串原始值不可迭代：for-of 抛 TypeError、Array.from 走
    // array-like 空结果；数组与字符串的默认路径保持不变。
    assert_truthy(
        "primitive default paths",
        "(function(){
           var threw = false;
           try { for (var x of 5) { } } catch (e) { threw = e instanceof TypeError; }
           if (!threw) return false;
           if (Array.from(5).length !== 0) return false;
           var a = Array.from([1,2]);
           if (a.length !== 2 || a[0] !== 1 || a[1] !== 2) return false;
           var n = 0;
           for (var c of 'ab') { n++; }
           if (n !== 2) return false;
           var s = Array.from('ab');
           if (s.length !== 2 || s[0] !== 'a' || s[1] !== 'b') return false;
           return true;
         })()",
    );
}
