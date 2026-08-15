//! 内置全局属性描述符测试：规范要求全局构造器/命名空间对象属性
//! enumerable=false（不泄漏进 Object.keys(globalThis) / for-in），
//! 全局 NaN/undefined/Infinity 三常量不可写不可枚举不可配置。

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval_truthy(source: &str) {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&module).expect("run");
    assert!(result.is_bool() && result.as_bool(), "expected true, got: {result:?}\nsource: {source}");
}

#[test]
fn global_constructors_are_non_enumerable() {
    for name in [
        "Array", "Object", "Math", "JSON", "Reflect", "Number", "String", "Date", "Map", "Set", "Error", "Promise",
        "BigInt",
    ] {
        let src = format!("Object.getOwnPropertyDescriptor(globalThis, '{name}').enumerable === false");
        eval_truthy(&src);
    }
}

#[test]
fn global_constructors_descriptor_full_attributes() {
    eval_truthy(
        "var d = Object.getOwnPropertyDescriptor(globalThis, 'Array'); \
         d.writable === true && d.enumerable === false && d.configurable === true",
    );
    eval_truthy(
        "var d = Object.getOwnPropertyDescriptor(globalThis, 'Math'); \
         d.writable === true && d.enumerable === false && d.configurable === true",
    );
}

#[test]
fn global_this_is_non_enumerable_data_property() {
    eval_truthy(
        "var d = Object.getOwnPropertyDescriptor(globalThis, 'globalThis'); \
         d.writable === true && d.enumerable === false && d.configurable === true",
    );
}

#[test]
fn keys_of_global_this_exclude_builtins() {
    eval_truthy("Object.keys(globalThis).indexOf('Math') === -1");
    eval_truthy("Object.keys(globalThis).indexOf('Array') === -1");
    eval_truthy("Object.keys(globalThis).indexOf('NaN') === -1");
    // 用户属性仍可枚举：内置属性收口不影响用户写入。
    eval_truthy("var x = 1; Object.keys(globalThis).indexOf('x') >= 0");
}

#[test]
fn for_in_global_this_excludes_builtins() {
    eval_truthy("var found = false; for (var k in globalThis) { if (k === 'Math') found = true; } found === false");
}

#[test]
fn nan_undefined_infinity_are_non_writable_non_configurable() {
    for name in ["NaN", "undefined", "Infinity"] {
        let src = format!(
            "var d = Object.getOwnPropertyDescriptor(globalThis, '{name}'); \
             d.writable === false && d.enumerable === false && d.configurable === false"
        );
        eval_truthy(&src);
    }
    eval_truthy("isNaN(globalThis.NaN) === true");
    eval_truthy("globalThis.undefined === undefined");
    eval_truthy("globalThis.Infinity === Infinity");
}

#[test]
fn math_constants_non_enumerable() {
    eval_truthy("Object.getOwnPropertyDescriptor(Math, 'PI').enumerable === false");
    eval_truthy("Object.getOwnPropertyDescriptor(Math, 'E').writable === false");
    eval_truthy("Object.getOwnPropertyDescriptor(Math, 'PI').configurable === false");
    eval_truthy("Object.keys(Math).length === 0");
    eval_truthy("Math.PI === 3.141592653589793");
}

#[test]
fn math_namespace_object_is_non_enumerable_on_global() {
    eval_truthy("Object.keys(globalThis).indexOf('Math') === -1");
}

#[test]
fn global_functions_non_enumerable() {
    for name in [
        "parseInt",
        "parseFloat",
        "isNaN",
        "isFinite",
        "encodeURI",
        "decodeURI",
        "escape",
        "unescape",
    ] {
        let src = format!("Object.getOwnPropertyDescriptor(globalThis, '{name}').enumerable === false");
        eval_truthy(&src);
    }
    eval_truthy("Object.keys(globalThis).indexOf('parseInt') === -1");
}

#[test]
fn user_global_properties_remain_enumerable() {
    eval_truthy("var x = 1; Object.getOwnPropertyDescriptor(globalThis, 'x').enumerable === true");
    eval_truthy("globalThis.y = 2; Object.getOwnPropertyDescriptor(globalThis, 'y').enumerable === true");
}

#[test]
fn builtin_globals_remain_readable() {
    eval_truthy("Array.isArray([]) === true");
    eval_truthy("JSON.stringify({a:1}) === '{\"a\":1}'");
    eval_truthy("Math.abs(-3) === 3");
    eval_truthy("typeof parseInt === 'function'");
    eval_truthy("Reflect.has({a:1}, 'a') === true");
    eval_truthy("Number('42') === 42");
}
