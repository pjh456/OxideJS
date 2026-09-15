//! `Iterator.from` / `Array.from` 的原始值迭代器面钉：String 臂经 GetMethod
//! 以原始值 receiver 读 `@@iterator`（用户覆盖 getter 观测 `typeof this`），
//! peek 对非字符串原始值装箱读 `@@iterator`，`Iterator.from` 对非字符串原始值
//! 非对象按 GetIteratorFlattenable 抛 TypeError（区别于 `Array.from` 的装箱路径）。

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
    // Array.from(5) 经 GetIterator 装箱后读 Number.prototype[Symbol.iterator]。
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
