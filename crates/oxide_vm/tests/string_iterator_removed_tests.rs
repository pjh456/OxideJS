//! String 臂默认迭代器删除/null/undefined 形钉：`String.prototype[Symbol.iterator]`
//! 缺失时 for-of/spread/解构/`Iterator.from`/`new Set` 按不可迭代抛 TypeError，
//! `Array.from`/`%TypedArray%.from` 落 array-like 按 UTF-16 单元产出；默认迭代器
//! 在位的正常路径不回归。

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
fn for_of_string_throws_after_delete() {
    // 原始串：删除默认迭代器后不再码元步进，按不可迭代抛 TypeError。
    assert_truthy(
        "for-of primitive delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            try { for (var c of "ab") {} return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
    // 装箱串同形。
    assert_truthy(
        "for-of boxed delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            try { for (var c of new String("ab")) {} return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
}

#[test]
fn spread_and_destructure_string_throw_after_delete() {
    assert_truthy(
        "spread primitive delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            try { [..."ab"]; return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
    assert_truthy(
        "destructure primitive delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            try { var [a, b] = "ab"; return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
}

#[test]
fn iterator_from_string_throws_after_delete() {
    // GetIteratorFlattenable：原始串与装箱串均按不可迭代抛 TypeError。
    assert_truthy(
        "Iterator.from primitive delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            try { Iterator.from("ab"); return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
    assert_truthy(
        "Iterator.from boxed delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            try { Iterator.from(new String("ab")); return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
}

#[test]
fn set_constructor_string_throws_after_delete() {
    assert_truthy(
        "new Set primitive delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            try { new Set("ab"); return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
    assert_truthy(
        "new Set boxed delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            try { new Set(new String("ab")); return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
}

#[test]
fn null_and_undefined_forms_throw() {
    // null 与 undefined 两形与删除同判：GetMethod 解析为空 → 不可迭代。
    assert_truthy(
        "for-of null",
        r#"(function(){
            String.prototype[Symbol.iterator] = null;
            try { for (var c of "ab") {} return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
    assert_truthy(
        "for-of undefined",
        r#"(function(){
            String.prototype[Symbol.iterator] = undefined;
            try { for (var c of "ab") {} return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
}

#[test]
fn array_from_string_falls_back_to_array_like_after_delete() {
    // 删除默认迭代器后 Array.from 走 array-like，按 UTF-16 码元逐个产出，不抛。
    assert_truthy(
        "Array.from primitive delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            var r = Array.from("ab");
            return r.length === 2 && r[0] === "a" && r[1] === "b";
        })()"#,
    );
    assert_truthy(
        "Array.from boxed delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            var r = Array.from(new String("ab"));
            return r.length === 2 && r[0] === "a" && r[1] === "b";
        })()"#,
    );
}

#[test]
fn array_from_string_null_and_undefined_fall_back() {
    assert_truthy(
        "Array.from null",
        r#"(function(){
            String.prototype[Symbol.iterator] = null;
            var r = Array.from("ab");
            return r.length === 2 && r[0] === "a" && r[1] === "b";
        })()"#,
    );
    assert_truthy(
        "Array.from undefined",
        r#"(function(){
            String.prototype[Symbol.iterator] = undefined;
            var r = Array.from("ab");
            return r.length === 2 && r[0] === "a" && r[1] === "b";
        })()"#,
    );
}

#[test]
fn array_from_string_falls_back_to_code_units_after_delete() {
    // array-like 口径是 UTF-16 码元：代理对拆成两个孤立 surrogate，不合并成码点。
    assert_truthy(
        "Array.from gclef delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            var r = Array.from("\uD834\uDD1E");
            return r.length === 2 && r[0] === "\uD834" && r[1] === "\uDD1E";
        })()"#,
    );
}

#[test]
fn array_from_string_mapfn_on_array_like_path() {
    // array-like 回退仍逐元素应用 mapFn，索引按码元位置。
    assert_truthy(
        "Array.from mapFn delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            var r = Array.from("ab", function(c, i){ return c + i; });
            return r.length === 2 && r[0] === "a0" && r[1] === "b1";
        })()"#,
    );
}

#[test]
fn array_from_generic_array_like_unaffected() {
    // 非字符串 array-like 源不受字符串分支影响。
    assert_truthy(
        "Array.from object",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            var r = Array.from({ length: 2, 0: "a", 1: "b" });
            return r.length === 2 && r[0] === "a" && r[1] === "b";
        })()"#,
    );
}

#[test]
fn typed_array_from_string_falls_back_after_delete() {
    // %TypedArray%.from 同前置门：删除后落 array-like，按单元取值再 ToNumber。
    assert_truthy(
        "Uint8Array.from delete",
        r#"(function(){
            delete String.prototype[Symbol.iterator];
            var u = Uint8Array.from("ab");
            return u.length === 2 && u[0] === 0 && u[1] === 0;
        })()"#,
    );
}

#[test]
fn non_callable_iterator_method_throws_for_optional_entry() {
    // 非空不可调用方法是 GetMethod 步 4 的 TypeError，不得落 array-like。
    assert_truthy(
        "42 for-of",
        r#"(function(){
            String.prototype[Symbol.iterator] = 42;
            try { for (var c of "ab") {} return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
    assert_truthy(
        "42 Array.from",
        r#"(function(){
            String.prototype[Symbol.iterator] = 42;
            try { Array.from("ab"); return false; }
            catch (e) { return e instanceof TypeError; }
        })()"#,
    );
}

#[test]
fn peek_getter_sees_primitive_receiver() {
    // Array.from 的可迭代性探测经 String.prototype 读 @@iterator，receiver 为原始值。
    assert_truthy(
        "Array.from getter receiver",
        r#"(function(){
            var orig = String.prototype[Symbol.iterator];
            var obs;
            Object.defineProperty(String.prototype, Symbol.iterator, {
                get: function(){ "use strict"; obs = typeof this; return orig; }
            });
            var r = Array.from("ab");
            return obs === "string" && r.length === 2 && r[0] === "a";
        })()"#,
    );
}

#[test]
fn array_from_string_object_ignores_duck_next_after_delete() {
    // 装箱串 @@iterator 缺失时按 array-like（规范只查 @@iterator）：即便对象自身有
    // 可调用 next，也不走鸭子迭代路径、不因 get_iterator 无 duck 回退而误抛。
    assert_truthy(
        "Array.from boxed duck-next delete",
        r#"(function(){
            var s = new String("zz");
            s.next = function(){ return { value: "DUCK", done: false }; };
            delete String.prototype[Symbol.iterator];
            var r = Array.from(s);
            return r.length === 2 && r[0] === "z" && r[1] === "z";
        })()"#,
    );
}

#[test]
fn default_string_iterator_regression() {
    // 默认迭代器在位：码点快速路径不变（抽象字符单个元素），各消费者逐字符。
    assert_truthy(
        "default string iterator",
        r#"(function(){
            var chars = "";
            for (var c of "ab") chars += c;
            var spread = [..."ab"].join("");
            var destructured = (function(){ var [a, b] = "ab"; return a + b; })();
            var from = Array.from("ab").join("");
            var fromIterator = Array.from(Iterator.from("ab")).join("");
            var gclef = Array.from("\uD834\uDD1E");
            var direct = String.prototype[Symbol.iterator].call("ab");
            return chars === "ab" && spread === "ab" && destructured === "ab"
                && from === "ab" && fromIterator === "ab"
                && gclef.length === 1 && gclef[0] === "\uD834\uDD1E"
                && direct.next().value === "a" && direct.next().value === "b";
        })()"#,
    );
}
