//! 回归测试：si-0 哨兵键（"prototype"）枚举正确性。
//!
//! 根因：`builtin_labels` 将 "prototype" intern 为 si 0，但三处
//! `property_name != 0` 守卫将其排除在枚举之外，导致 delete 重建、
//! getOwnPropertyNames 等路径丢失 "prototype" 键。
//!
//! 修复：删除守卫，si 0 成为一等属性键。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

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
        Ok(result) => js_value_to_str(result),
        Err(e) => format!("vm error: {e}"),
    }
}

fn eval_json(source: &str) -> String {
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
        Ok(result) => js_value_to_str(result),
        Err(e) => format!("vm error: {e}"),
    }
}

/// 将 VM 返回值转为 JS 可见字符串：字符串值直接取文本，整型/布尔/undefined
/// 走 Debug display，对象走 Debug display（测试中不用于对象比较）。
fn js_value_to_str(v: JsValue) -> String {
    if v.is_string() {
        // SAFETY: 字符串值指向 VM arena（调用方保持 vm 存活）。
        unsafe { &*v.as_string_ptr() }.to_owned_string()
    } else {
        format!("{v}")
    }
}

// ── 机制 A：delete 重建后 "prototype" 不丢失 ──

#[test]
fn delete_others_preserves_prototype() {
    // {prototype:{m:1},x:2}; delete o.x; typeof o.prototype → "object"
    assert_eq!(eval("var o={prototype:{m:1},x:2}; delete o.x; typeof o.prototype"), "object");
}

#[test]
fn delete_class_prototype_method_preserves_prototype_prop() {
    // class C{prototype(){}x(){}} delete C.prototype.x; typeof C.prototype.prototype → "function"
    assert_eq!(
        eval("class C{prototype(){}x(){}} delete C.prototype.x; typeof C.prototype.prototype"),
        "function"
    );
}

// ── Object.keys / getOwnPropertyNames 包含 "prototype" ──

#[test]
fn object_keys_contains_prototype_key() {
    // Object.keys({prototype:1}) 应返回 ["prototype"]
    let r = eval("var o={prototype:1}; Object.keys(o).length");
    assert_eq!(r, "1", "Object.keys should include prototype key");
}

#[test]
fn get_own_property_names_class_constructor_has_prototype() {
    // Object.getOwnPropertyNames(class A{}) 应包含 "prototype"
    let r = eval("Object.getOwnPropertyNames(class A{}).length");
    assert_eq!(r, "3", "class constructor should have length, name, prototype");
}

#[test]
fn get_own_property_names_object_literal_has_prototype() {
    // Object.getOwnPropertyNames({prototype:1}) 应包含 "prototype"
    let r = eval("Object.getOwnPropertyNames({prototype:1}).length");
    assert_eq!(r, "1", "object literal should have prototype key");
}

// ── 类构造器属性序：[length, name, prototype] ──

#[test]
fn class_constructor_property_order() {
    // 类构造器属性序应为 [length, name, prototype]
    let r = eval("Object.getOwnPropertyNames(class A{}).join(',')");
    assert_eq!(r, "length,name,prototype", "class constructor property order");
}

// ── JSON.stringify 包含 "prototype" ──

#[test]
fn json_stringify_includes_prototype() {
    // JSON.stringify({prototype:1}) → "{\"prototype\":1}"
    let r = eval_json("JSON.stringify({prototype:1})");
    assert_eq!(r, "{\"prototype\":1}", "JSON.stringify should include prototype key");
}

// ── Reflect.ownKeys 包含 "prototype" ──

#[test]
fn reflect_own_keys_includes_prototype() {
    let r = eval("Reflect.ownKeys({prototype:1}).length");
    assert_eq!(r, "1", "Reflect.ownKeys should include prototype key");
}

// ── for-in 不产出 "prototype"（不可枚举，修后仍正确）──

#[test]
fn for_in_excludes_non_enumerable_prototype() {
    // 构造器 "prototype" 不可枚举，for-in 不应产出
    let r = eval("var c=0; class A{} for(var k in A) c++; c");
    assert_eq!(r, "0", "for-in should not enumerate non-enumerable prototype on class constructor");
}

// ── 存在性检查不受影响 ──

#[test]
fn has_own_property_prototype_true() {
    let r = eval("Object.getOwnPropertyDescriptor(class A{}, 'prototype') !== undefined");
    assert_eq!(r, "true", "hasOwnProperty should return true for prototype");
}

#[test]
fn in_operator_prototype_true() {
    let r = eval("'prototype' in function f(){}");
    assert_eq!(r, "true", "'prototype' in function should be true");
}

// ── 对象字面量 "prototype" 属性读写 ──

#[test]
fn object_literal_prototype_read_write() {
    let r = eval("var o={prototype:42}; o.prototype");
    assert_eq!(r, "42", "should read prototype property from object literal");
}

#[test]
fn object_literal_prototype_delete() {
    let r = eval("var o={prototype:42,x:1}; delete o.x; o.prototype");
    assert_eq!(r, "42", "delete other prop should not affect prototype property");
}
