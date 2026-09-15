//! delete 成员表达式基为原始值：ToObject 基求值——null/undefined 抛
//! TypeError，其余原始基装箱后按自身属性面判删除成败（字符串自身属性为
//! 规范下标 0..len-1 与 "length"，均不可配置；其它原始装箱体无自身属性）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

fn truthy(source: &str) {
    let result = eval(source).unwrap_or_else(|e| panic!("{source} -> {e}"));
    assert!(result.as_bool(), "{source} -> 期望 true，得 {:?}", result);
}

fn falsy(source: &str) {
    let result = eval(source).unwrap_or_else(|e| panic!("{source} -> {e}"));
    assert!(!result.as_bool(), "{source} -> 期望 false，得 {:?}", result);
}

#[test]
fn delete_string_index_beyond_length_is_true() {
    // 计算面：字符串基越界下标无自身属性，删除成功。
    truthy("delete 'Test262'[100]");
}

#[test]
fn delete_string_index_within_length_is_false() {
    // 计算面：字符串自身下标属性不可配置，删除失败。
    falsy("delete 'ab'[0]");
    falsy("delete 'ab'[1]");
    truthy("delete 'ab'[2]");
}

#[test]
fn delete_string_length_is_false() {
    // 静态面：字符串 "length" 自身属性不可配置。
    falsy("delete 'abc'.length");
}

#[test]
fn delete_number_base_keys_are_true() {
    // number 装箱体无自身属性（原型链属性不算），静态/计算键均成功。
    truthy("delete (2).c");
    truthy("delete (2).toString");
    truthy("delete (2).constructor");
    truthy("delete (0)['0']");
}

#[test]
fn delete_boolean_base_keys_are_true() {
    truthy("delete true.foo");
}

#[test]
fn delete_bigint_base_keys_are_true() {
    truthy("delete (1n).x");
    truthy("delete (1n)[0]");
}

#[test]
fn delete_symbol_base_keys_are_true() {
    truthy("delete Symbol().a");
}

#[test]
fn delete_non_object_base_symbol_key_is_true() {
    // 计算面 symbol 键：字符串装箱体无 symbol 自身属性。
    truthy("delete 'ab'[Symbol.iterator]");
}

#[test]
fn delete_null_base_throws_type_error() {
    truthy("try { delete null.a; false } catch (e) { e instanceof TypeError }");
}

#[test]
fn delete_undefined_base_throws_type_error() {
    truthy("try { delete undefined.a; false } catch (e) { e instanceof TypeError }");
}

#[test]
fn delete_nan_base_keys_are_true() {
    truthy("delete NaN.x");
}
