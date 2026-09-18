use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

// 返回保活的 Vm：结果可能含对象指针（数组/描述符），读取前必须保持 Vm 存活，
// 否则 epoch arena 随 drop 释放后指针悬垂（use-after-free）。
fn eval_keep_vm(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module))?;
    Ok((vm, result))
}

fn eval_many(lines: &[&str]) -> Result<JsValue, String> {
    let source = lines.join("; ");
    eval(&source)
}

fn assert_err_contains(result: Result<JsValue, String>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error containing '{}', got Ok", expected),
        Err(e) => assert!(e.contains(expected), "expected error containing '{}', got: {}", expected, e),
    }
}

// ── 严格模式向不可写自身属性赋值抛 TypeError ──
#[test]
fn assign_to_non_writable_throws_in_strict() {
    let result = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, writable: false})",
        "(function() { 'use strict'; obj.x = 2 })()",
    ]);
    assert_err_contains(result, "read-only");
}

// ── sloppy 向不可写自身属性赋值静默失败（值不变、不抛） ──
#[test]
fn assign_to_non_writable_silently_fails_in_sloppy() {
    let r = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, writable: false})",
        "obj.x = 2",
        "obj.x",
    ])
    .unwrap();
    assert!(r.is_int() && r.as_int() == 1, "sloppy 只读属性写应静默 no-op，值保持 1，实际 {:?}", r);
}

// ── 向可写自身属性赋值成功 ──
#[test]
fn assign_to_writable_succeeds() {
    let r = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, writable: true})",
        "obj.x = 2",
        "obj.x",
    ])
    .unwrap();
    assert!(r.is_int() && r.as_int() == 2, "assign to writable should set value to 2, got {:?}", r);
}

// ── 严格模式向原型上不可写的继承属性赋值抛错 ──
#[test]
fn assign_to_inherited_non_writable_throws_in_strict() {
    let result = eval_many(&[
        "var proto = {}",
        "Object.defineProperty(proto, 'x', {value: 1, writable: false, enumerable: true, configurable: true})",
        "var child = Object.create(proto)",
        "(function() { 'use strict'; child.x = 2 })()",
    ]);
    assert_err_contains(result, "read-only");
}

// ── 继承只读数据属性：sloppy 赋值静默失败，不创建自有属性（不遮蔽） ──
#[test]
fn assign_to_inherited_non_writable_silently_fails_in_sloppy() {
    let (_vm, r) = eval_keep_vm(
        &[
            "var proto = {}",
            "Object.defineProperty(proto, 'x', {value: 1, writable: false, enumerable: true, configurable: true})",
            "var child = Object.create(proto)",
            "child.x = 99",
            "[child.x, child.hasOwnProperty('x')]",
        ]
        .join("; "),
    )
    .unwrap();
    let arr = unsafe { &*r.as_js_object_ptr() };
    assert!(
        arr.get_prop_at(0).is_int() && arr.get_prop_at(0).as_int() == 1,
        "sloppy 写只读继承属性应静默 no-op，值保持 1"
    );
    assert!(
        arr.get_prop_at(1).is_bool() && !arr.get_prop_at(1).as_bool(),
        "静默失败不得在子对象上创建自有属性"
    );
}

// ── 严格模式：原型只读可配置属性赋值抛错（不可写数据属性不可遮蔽） ──
#[test]
fn assign_non_writable_proto_configurable_prop_throws_in_strict() {
    let result = eval_many(&[
        "var proto = {}",
        "Object.defineProperty(proto, 'x', {value: 1, writable: false, enumerable: true, configurable: true})",
        "var child = Object.create(proto)",
        "(function() { 'use strict'; child.x = 99 })()",
    ]);
    assert_err_contains(result, "read-only");
}

// ── 严格模式 Object.freeze 后写已有属性抛 TypeError ──
#[test]
fn frozen_write_existing_prop_throws_in_strict() {
    let result = eval_many(&["var o = Object.freeze({a: 1})", "(function() { 'use strict'; o.a = 2 })()"]);
    assert_err_contains(result, "read-only");
}

// ── sloppy Object.freeze 后写已有属性静默失败 ──
#[test]
fn frozen_write_existing_prop_silently_fails_in_sloppy() {
    let r = eval_many(&["var o = Object.freeze({a: 1})", "o.a = 2", "o.a"]).unwrap();
    assert!(r.is_int() && r.as_int() == 1, "sloppy 冻结对象写应静默 no-op，值保持 1，实际 {:?}", r);
}

// ── 严格模式 Object.freeze 后写新属性抛 TypeError ──
#[test]
fn frozen_write_new_prop_throws_in_strict() {
    let result = eval_many(&["var o = Object.freeze({a: 1})", "(function() { 'use strict'; o.b = 2 })()"]);
    assert_err_contains(result, "not extensible");
}

// ── sloppy Object.freeze 后写新属性静默失败（属性不创建） ──
#[test]
fn frozen_write_new_prop_silently_fails_in_sloppy() {
    let r = eval_many(&["var o = Object.freeze({a: 1})", "o.b = 2", "o.b === undefined"]).unwrap();
    assert!(
        r.is_bool() && r.as_bool(),
        "sloppy 冻结对象写新属性应静默 no-op，属性不创建，实际 {:?}",
        r
    );
}

// ── Object.freeze 后 defineProperty 抛 TypeError ──
#[test]
fn frozen_define_property_throws() {
    let result = eval_many(&["var o = Object.freeze({})", "Object.defineProperty(o, 'x', {value: 1})"]);
    assert_err_contains(result, "not extensible");
}

// ── 严格模式 Object.seal 后写新属性抛错、删属性返回 false、改值生效 ──
#[test]
fn sealed_semantics() {
    let (_vm, r) = eval_keep_vm(
        &[
            "var o = Object.seal({a: 1})",
            "var new_throws = false",
            "(function() { 'use strict'; try { o.b = 2 } catch (e) { new_throws = true } })()",
            "var del = delete o.a",
            "o.a = 5",
            "[new_throws, del, o.a, o.hasOwnProperty('a')]",
        ]
        .join("; "),
    )
    .unwrap();
    let arr = unsafe { &*r.as_js_object_ptr() };
    assert!(
        arr.get_prop_at(0).is_bool() && arr.get_prop_at(0).as_bool(),
        "strict new prop write should throw"
    );
    assert!(arr.get_prop_at(1).is_bool() && !arr.get_prop_at(1).as_bool(), "delete should be false");
    assert!(arr.get_prop_at(2).is_int() && arr.get_prop_at(2).as_int() == 5, "value write should work");
    assert!(arr.get_prop_at(3).is_bool() && arr.get_prop_at(3).as_bool(), "prop should remain own");
}

// ── Object.preventExtensions 后新属性写静默失效（sloppy），已有属性可写 ──
#[test]
fn prevent_extensions_semantics() {
    let (_vm, r) = eval_keep_vm(
        &[
            "var o = Object.preventExtensions({a: 1})",
            "var new_threw = false; try { o.b = 3 } catch (e) { new_threw = true }",
            "o.a = 2",
            "[new_threw, o.a, o.b === undefined]",
        ]
        .join("; "),
    )
    .unwrap();
    let arr = unsafe { &*r.as_js_object_ptr() };
    assert!(
        arr.get_prop_at(0).is_bool() && !arr.get_prop_at(0).as_bool(),
        "sloppy new prop write should silently fail"
    );
    assert!(
        arr.get_prop_at(1).is_int() && arr.get_prop_at(1).as_int() == 2,
        "existing prop write should work"
    );
    assert!(arr.get_prop_at(2).is_bool() && arr.get_prop_at(2).as_bool(), "new prop should be absent");
}

// ── 严格模式 Object.freeze 数组：元素写与 length 收缩均抛错 ──
#[test]
fn frozen_array_write_and_length_throws_in_strict() {
    let (_vm, r) = eval_keep_vm(
        &[
            "var a = Object.freeze([1, 2])",
            "var e1 = false, e2 = false",
            "(function() { 'use strict'; try { a[0] = 9 } catch (e) { e1 = true } })()",
            "(function() { 'use strict'; try { a.length = 0 } catch (e) { e2 = true } })()",
            "[e1, e2, a[0], a.length, Object.getOwnPropertyDescriptor(a, '0').writable]",
        ]
        .join("; "),
    )
    .unwrap();
    let arr = unsafe { &*r.as_js_object_ptr() };
    assert!(arr.get_prop_at(0).is_bool() && arr.get_prop_at(0).as_bool(), "element write should throw");
    assert!(arr.get_prop_at(1).is_bool() && arr.get_prop_at(1).as_bool(), "length shrink should throw");
    assert!(arr.get_prop_at(2).is_int() && arr.get_prop_at(2).as_int() == 1, "element value unchanged");
    assert!(arr.get_prop_at(3).is_int() && arr.get_prop_at(3).as_int() == 2, "length unchanged");
    assert!(
        arr.get_prop_at(4).is_bool() && !arr.get_prop_at(4).as_bool(),
        "descriptor writable should be false"
    );
}

// ── sloppy Object.freeze 数组：元素写与 length 写均静默失败（严格模式才抛） ──
#[test]
fn frozen_array_element_write_silently_fails_in_sloppy() {
    let (_vm, r) = eval_keep_vm(
        &[
            "var a = Object.freeze([1, 2])",
            "var e1 = false",
            "try { a[0] = 9 } catch (e) { e1 = true }",
            "[e1, a[0]]",
        ]
        .join("; "),
    )
    .unwrap();
    let arr = unsafe { &*r.as_js_object_ptr() };
    assert!(
        arr.get_prop_at(0).is_bool() && !arr.get_prop_at(0).as_bool(),
        "sloppy 冻结数组元素写应静默 no-op"
    );
    assert!(arr.get_prop_at(1).is_int() && arr.get_prop_at(1).as_int() == 1, "element value unchanged");
}

// ── isFrozen/isSealed：手动 defineProperty 全部冻结（未调 freeze）也应判 true ──
#[test]
fn is_frozen_manual_define_props() {
    let r = eval_many(&[
        "var o = {a: 1}",
        "Object.defineProperty(o, 'a', {writable: false, configurable: false})",
        "Object.preventExtensions(o)",
        "Object.isFrozen(o)",
    ])
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "manually frozen object should be isFrozen");
}

// ── Reflect.set / Reflect.defineProperty 对 frozen 对象返回 false ──
#[test]
fn reflect_ops_on_frozen_return_false() {
    let (_vm, r) = eval_keep_vm(
        &[
            "var f = Object.freeze({a: 1})",
            "[Reflect.set(f, 'a', 2), Reflect.set(f, 'b', 2), Reflect.defineProperty(f, 'x', {value: 1})]",
        ]
        .join("; "),
    )
    .unwrap();
    let arr = unsafe { &*r.as_js_object_ptr() };
    for i in 0..3 {
        assert!(
            arr.get_prop_at(i).is_bool() && !arr.get_prop_at(i).as_bool(),
            "reflect op {} should be false",
            i
        );
    }
}

// ── 严格模式 freeze 后 IC 直写路径失效：循环写仍被拦截 ──
#[test]
fn frozen_ic_path_still_blocks_write_in_strict() {
    let r = eval_many(&[
        "var o = Object.freeze({a: 1})",
        "var threw = false",
        "(function() { 'use strict'; for (var i = 0; i < 3; i++) { try { o.a = i } catch (e) { threw = true } } })()",
        "threw",
    ])
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "IC path should still throw on frozen write");
}

// ── sloppy freeze 后循环写静默失效（值不变、不抛） ──
#[test]
fn frozen_ic_path_silently_fails_in_sloppy() {
    let (_vm, r) = eval_keep_vm(
        &[
            "var o = Object.freeze({a: 1})",
            "var threw = false",
            "for (var i = 0; i < 3; i++) { try { o.a = i } catch (e) { threw = true } }",
            "[threw, o.a]",
        ]
        .join("; "),
    )
    .unwrap();
    let arr = unsafe { &*r.as_js_object_ptr() };
    assert!(
        arr.get_prop_at(0).is_bool() && !arr.get_prop_at(0).as_bool(),
        "sloppy frozen write should silently fail"
    );
    assert!(arr.get_prop_at(1).is_int() && arr.get_prop_at(1).as_int() == 1, "value unchanged");
}

// ── sealed 对象 IC 路径：已有属性写正常 ──
#[test]
fn sealed_ic_path_allows_existing_write() {
    let r = eval_many(&["var o = Object.seal({a: 1})", "for (var i = 0; i < 3; i++) { o.a = i }", "o.a"]).unwrap();
    let ok = (r.is_int() && r.as_int() == 2) || (r.is_double() && r.as_double() == 2.0);
    assert!(ok, "sealed existing prop write should work via IC, got {:?}", r);
}
