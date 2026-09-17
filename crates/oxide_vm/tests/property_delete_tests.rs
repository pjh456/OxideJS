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

fn eval_many(lines: &[&str]) -> Result<JsValue, String> {
    let source = lines.join("; ");
    eval(&source)
}

fn truthy(source: &str) {
    let result = eval(source).unwrap_or_else(|e| panic!("{source} -> {e}"));
    assert!(result.is_bool() && result.as_bool(), "{source} -> 期望 true，得 {:?}", result);
}

// ── 删除可配置属性成功 ──
#[test]
fn delete_configurable_property_succeeds_and_returns_true() {
    let r = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, configurable: true})",
        "delete obj.x",
    ])
    .unwrap();
    assert!(r.is_bool() && r.as_bool(), "delete configurable should return true, got {:?}", r);
}

// ── 删除不可配置属性：sloppy 模式返回 false（不抛错）──
#[test]
fn delete_non_configurable_returns_false_in_sloppy_mode() {
    let r = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, configurable: false})",
        "delete obj.x",
    ])
    .unwrap();
    assert!(
        r.is_bool() && !r.as_bool(),
        "delete non-configurable in sloppy should return false, got {:?}",
        r
    );
}

// ── 删除不存在的属性返回 true ──
#[test]
fn delete_non_existent_returns_true() {
    let r = eval_many(&["var obj = {}", "delete obj.x"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "delete non-existent should return true, got {:?}", r);
}

// ── 删除默认（无显式 defineProperty）可配置属性成功 ──
#[test]
fn delete_default_configurable_succeeds() {
    let r = eval_many(&["var obj = {}", "obj.x = 1", "delete obj.x"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "delete default configurable should return true, got {:?}", r);
}

// ── 子对象删除原型上不可配置的属性：子对象不拥有它，因此返回 true ──
#[test]
fn delete_non_configurable_proto_property_on_child_returns_true() {
    let r = eval_many(&[
        "var proto = {}",
        "proto.x = 1",
        "Object.defineProperty(proto, 'x', {configurable: false})",
        "var child = Object.create(proto)",
        "delete child.x",
    ])
    .unwrap();
    assert!(
        r.is_bool() && r.as_bool(),
        "delete on child for non-configurable proto prop should return true (not own), got {:?}",
        r
    );
}

// -- Delete configurable proto-level property through child --
#[test]
fn delete_configurable_proto_property_on_child_returns_true() {
    let r = eval_many(&[
        "var proto = {}",
        "Object.defineProperty(proto, 'x', {value: 1, configurable: true, enumerable: true})",
        "var child = Object.create(proto)",
        "delete child.x",
    ])
    .unwrap();
    assert!(
        r.is_bool() && r.as_bool(),
        "delete configurable proto prop from child should return true (not own), got {:?}",
        r
    );
}

// ── 严格模式：不可配置数据属性抛 TypeError 且属性保留 ──
#[test]
fn strict_delete_non_configurable_data_throws_type_error() {
    truthy(
        "\"use strict\"; var o = {}; \
         Object.defineProperty(o, 'x', {value: 1, configurable: false}); \
         var threw = false; try { delete o.x; } catch (e) { threw = e instanceof TypeError; } \
         threw && o.x === 1",
    );
}

// ── 严格模式：不可配置访问器属性同样抛 TypeError 且 getter 保留 ──
#[test]
fn strict_delete_non_configurable_accessor_throws_type_error() {
    truthy(
        "\"use strict\"; var o = {}; \
         Object.defineProperty(o, 'x', {get: function () { return 1; }, configurable: false}); \
         var threw = false; try { delete o.x; } catch (e) { threw = e instanceof TypeError; } \
         threw && o.x === 1",
    );
}

// ── 严格模式：删除全局不可配置内置属性（NaN）抛 TypeError ──
#[test]
fn strict_delete_global_nan_throws_type_error() {
    truthy(
        "\"use strict\"; var g = this; \
         var threw = false; try { delete g.NaN; } catch (e) { threw = e instanceof TypeError; } \
         threw && g.NaN !== undefined",
    );
}

// ── 严格模式：删除 Math.LN2 抛 TypeError ──
#[test]
fn strict_delete_math_ln2_throws_type_error() {
    truthy(
        "\"use strict\"; \
         var threw = false; try { delete Math.LN2; } catch (e) { threw = e instanceof TypeError; } \
         threw && Math.LN2 !== undefined",
    );
}

// ── 严格模式：可配置属性仍可删除 ──
#[test]
fn strict_delete_configurable_property_succeeds() {
    truthy(
        "\"use strict\"; var o = {}; \
         Object.defineProperty(o, 'x', {value: 1, configurable: true}); \
         delete o.x === true && o.x === undefined",
    );
}

// ── 严格模式：删除不存在的属性返回 true（不得过抛） ──
#[test]
fn strict_delete_missing_property_returns_true() {
    truthy("\"use strict\"; var o = {}; delete o.missing === true");
}

// ── 数组 length 虚拟属性：sloppy 删除返回 false 且 length 不变 ──
#[test]
fn sloppy_delete_array_length_returns_false() {
    truthy("var a = [1, 2, 3]; a.x = 10; delete a.length === false && a.length === 3");
}

// ── 数组 length 虚拟属性：严格模式删除抛 TypeError 且 length 不变 ──
#[test]
fn strict_delete_array_length_throws_type_error() {
    truthy(
        "\"use strict\"; var a = [1, 2, 3]; \
         var threw = false; try { delete a.length; } catch (e) { threw = e instanceof TypeError; } \
         threw && a.length === 3",
    );
}

// ── 冻结数组元素不可配置：严格模式删除抛 TypeError 且元素保留 ──
#[test]
fn strict_delete_frozen_array_element_throws_type_error() {
    truthy(
        "\"use strict\"; var a = [1, 2, 3]; Object.freeze(a); \
         var threw = false; try { delete a[0]; } catch (e) { threw = e instanceof TypeError; } \
         threw && a[0] === 1",
    );
}

// ── Reflect.deleteProperty 恒不抛：不可配置与数组 length 均返回 false ──
#[test]
fn reflect_delete_non_configurable_and_array_length_return_false() {
    truthy(
        "var o = {}; Object.defineProperty(o, 'x', {value: 1, configurable: false}); \
         var a = [1, 2, 3]; \
         Reflect.deleteProperty(o, 'x') === false && o.x === 1 \
         && Reflect.deleteProperty(a, 'length') === false && a.length === 3",
    );
}
