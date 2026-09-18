use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module))?;
    Ok((vm, result))
}

#[test]
fn object_keys_empty() {
    let (_vm, result) = eval("Object.keys({})").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 0);
}

#[test]
fn object_keys_has_own_property() {
    let (_vm, result) = eval("Object.keys({a:1,b:2})").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
}

#[test]
fn object_member_native_call_uses_regular_call_path() {
    let (_vm, result) = eval("Object.keys({a:1}).length").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn object_member_call_allows_user_overwrite() {
    let (_vm, result) = eval("Object.keys = function() { return 9; }; Object.keys()").unwrap();
    assert_eq!(result.as_int(), 9);
}

#[test]
fn missing_object_member_call_is_not_call_native_target_error() {
    let err = match eval("Object.noSuchMethod()") {
        Ok(_) => panic!("missing member call should fail"),
        Err(err) => err,
    };
    assert!(err.contains("CALL target is not callable"), "unexpected error: {err}");
    assert!(!err.contains("CALL_NATIVE target"), "unexpected CALL_NATIVE path: {err}");
}

#[test]
fn object_create_null_proto() {
    let (_vm, _result) = eval("Object.create(null)").unwrap();
}

#[test]
fn object_assign_copies() {
    let (_vm, result) = eval("Object.assign({a:1},{b:2})").unwrap();
    assert!(result.is_object());
}

#[test]
fn object_assign_boxes_primitive_target() {
    let (_vm, result) = eval("var result = Object.assign('a'); result.valueOf()").unwrap();
    assert!(result.is_string());
    let rendered = _vm.lookup_str(result).unwrap_or_default();
    assert_eq!(rendered, "a");
}

#[test]
fn object_is_uses_same_value_semantics() {
    let (_vm, result) = eval("Object.is(NaN, NaN)").unwrap();
    assert_eq!(result, JsValue::bool(true));

    let (_vm, result) = eval("Object.is(1, 1)").unwrap();
    assert_eq!(result, JsValue::bool(true));

    let (_vm, result) = eval("Object.is(1, '1')").unwrap();
    assert_eq!(result, JsValue::bool(false));
}

#[test]
fn object_get_own_property_descriptor_value() {
    let (_vm, result) = eval("Object.getOwnPropertyDescriptor({a:1},'a')").unwrap();
    assert!(result.is_object());
}

#[test]
fn object_get_own_property_descriptor_missing() {
    let (_vm, result) = eval("Object.getOwnPropertyDescriptor({},'x')").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn object_define_property_sets_value() {
    let (_vm, result) = eval("Object.defineProperty({},'x',{value:42})").unwrap();
    assert!(result.is_object());
}

#[test]
fn object_define_property_data_defaults_non_writable() {
    let (_vm, result) =
        eval("var o={}; Object.defineProperty(o,'x',{value:1}); Object.getOwnPropertyDescriptor(o,'x').writable")
            .unwrap();
    assert_eq!(result, JsValue::bool(false));
}

#[test]
fn object_define_property_accessor_descriptor_gets() {
    let (_vm, result) = eval(
        "var o={}; Object.defineProperty(o,'x',{get:function(){return 7}, enumerable:true, configurable:true}); o.x",
    )
    .unwrap();
    assert_eq!(result, JsValue::int(7));
}

#[test]
fn object_define_property_rejects_mixed_descriptor() {
    let err = match eval("var o={}; Object.defineProperty(o,'x',{value:1,get:function(){return 2}})") {
        Ok(_) => panic!("expected mixed descriptor to fail"),
        Err(err) => err,
    };
    assert!(err.contains("TypeError"));
}

#[test]
fn object_define_property_non_writable_assignment_throws_in_strict() {
    let err = match eval("var o={}; Object.defineProperty(o,'x',{value:1}); (function(){'use strict'; o.x=2})()") {
        Ok(_) => panic!("expected assignment to fail"),
        Err(err) => err,
    };
    assert!(err.contains("TypeError"));
}

#[test]
fn object_get_own_property_descriptor_accessor_shape() {
    let (_vm, result) =
        eval("var o={}; Object.defineProperty(o,'x',{get:function(){return 1}, configurable:true}); Object.getOwnPropertyDescriptor(o,'x').value").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn object_get_prototype_of() {
    let (_vm, result) = eval("Object.getPrototypeOf({})").unwrap();
    assert!(result.is_object() || result.is_null());
}

#[test]
fn object_has_own_true() {
    let (_vm, result) = eval("Object.hasOwn({a:1}, 'a')").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn object_has_own_false_for_inherited_property() {
    let (_vm, result) = eval("Object.hasOwn({}, 'toString')").unwrap();
    assert_eq!(result, JsValue::bool(false));
}

#[test]
fn object_proto_has_own_property_call_works() {
    let (_vm, result) = eval("Object.prototype.hasOwnProperty.call(Object, 'assign')").unwrap();
    assert_eq!(result, JsValue::bool(true));
}

#[test]
fn object_proto_property_is_enumerable_call_works() {
    let (_vm, result) = eval("Object.prototype.propertyIsEnumerable.call(Object, 'assign')").unwrap();
    // 内置方法按 ES 规范不可枚举。
    assert_eq!(result, JsValue::bool(false));
}

#[test]
fn object_entries_returns_array() {
    let (_vm, result) = eval("Object.entries({a:1,b:2})").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
}

#[test]
fn object_values_returns_array() {
    let (_vm, result) = eval("Object.values({a:1,b:2})").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
}

// -- Object.fromEntries --

#[test]
fn from_entries_numeric_key_normalized_to_int_key() {
    // 数字键 entry 走 ToPropertyKey 规范化：fromEntries 产出的 "5" 键与 o[5] 同一键。
    let (_vm, result) = eval("Object.fromEntries([[5,'a']])[5]").unwrap();
    assert!(result.is_string() && unsafe { &*result.as_string_ptr() }.as_str() == "a");
}

#[test]
fn from_entries_string_index_key_reads_via_numeric_access() {
    // 字符串 "5" 键是规范数组下标，读 o[5] 命中同一整数键。
    let (_vm, result) = eval("Object.fromEntries([['5','a']])[5]").unwrap();
    assert!(result.is_string() && unsafe { &*result.as_string_ptr() }.as_str() == "a");
}

#[test]
fn from_entries_numeric_key_write_keeps_single_key() {
    // 再写 o[5] 不产生第二个 "5" 键（键唯一）。
    let (_vm, result) =
        eval("var o = Object.fromEntries([[5,'a']]); o[5]='x'; Object.getOwnPropertyNames(o).join(',')").unwrap();
    assert!(result.is_string() && unsafe { &*result.as_string_ptr() }.as_str() == "5");
}

#[test]
fn from_entries_multiple_pairs_roundtrip() {
    let (_vm, result) = eval("var o = Object.fromEntries([[1,'a'],[2,'b']]); o[1] + o[2]").unwrap();
    assert!(result.is_string() && unsafe { &*result.as_string_ptr() }.as_str() == "ab");
}

// -- Object.keys/values/entries 数组元素区 --

#[test]
fn keys_array_includes_integer_elements() {
    let (_vm, result) = eval("Object.keys([1,2]).join(',')").unwrap();
    let s = unsafe { &*result.as_string_ptr() }.to_owned_string();
    assert_eq!(s, "0,1");
}

#[test]
fn keys_array_skips_holes() {
    let (_vm, result) = eval("Object.keys([1,,3]).join(',')").unwrap();
    let s = unsafe { &*result.as_string_ptr() }.to_owned_string();
    assert_eq!(s, "0,2");
}

#[test]
fn entries_array_reads_element_values() {
    let (_vm, result) = eval("var e = Object.entries([1,2]); e[1][1]").unwrap();
    assert!(
        result.is_int() && result.as_int() == 2,
        "second entry value should be 2, got {:?}",
        result
    );
}

#[test]
fn keys_array_mixed_named_prop_order() {
    // 整数下标升序在前，命名属性按插入序随后（无重复键）。
    let (_vm, result) = eval("var a = [10]; a.foo = 1; Object.keys(a).join(',')").unwrap();
    let s = unsafe { &*result.as_string_ptr() }.to_owned_string();
    assert_eq!(s, "0,foo");
}

#[test]
fn keys_array_non_enumerable_element_filtered() {
    // defineProperty 把元素改为不可枚举后 keys 应省略（values/entries 同理）。
    let (_vm, result) =
        eval("var a = [1, 2]; Object.defineProperty(a, '1', {enumerable: false}); Object.keys(a).join(',')").unwrap();
    let s = unsafe { &*result.as_string_ptr() }.to_owned_string();
    assert_eq!(s, "0");
}

// -- Object.assign --

#[test]
fn assign_filters_non_enumerable_source() {
    let (_vm, result) = eval(
        "var src = Object.defineProperty({}, 'x', {value: 1, enumerable: false}); JSON.stringify(Object.assign({}, src))",
    )
    .unwrap();
    let s = unsafe { &*result.as_string_ptr() }.to_owned_string();
    assert_eq!(s, "{}");
}

#[test]
fn assign_triggers_source_getter() {
    let (_vm, result) = eval("var n = 0; var t = Object.assign({}, {get b(){ n++; return 2 }}); [t.b, n]").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(
        obj.get_prop_at(0).is_int() && obj.get_prop_at(0).as_int() == 2,
        "getter value should be copied"
    );
    assert!(
        obj.get_prop_at(1).is_int() && obj.get_prop_at(1).as_int() == 1,
        "getter should run exactly once"
    );
}

#[test]
fn assign_from_array_source() {
    let (_vm, result) = eval("var t = Object.assign({}, [1, 2]); [t['0'], t['1']]").unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.get_prop_at(0).is_int() && obj.get_prop_at(0).as_int() == 1);
    assert!(obj.get_prop_at(1).is_int() && obj.get_prop_at(1).as_int() == 2);
}

#[test]
fn assign_target_setter_receives_target() {
    // 规范 Set(to, key, value, true)：target 同名 setter 的 this 是 target。
    let (_vm, result) = eval(
        "var recv = null; var t = {}; Object.defineProperty(t, 'x', {set: function(v){ recv = this; }, configurable: true}); Object.assign(t, {x: 1}); recv === t",
    )
    .unwrap();
    assert!(result.is_bool() && result.as_bool(), "target setter should receive target as this");
}

#[test]
fn assign_to_frozen_target_throws() {
    let err = match eval("var t = Object.freeze({a: 1}); Object.assign(t, {b: 2});") {
        Ok(_) => panic!("assign to frozen target should fail"),
        Err(err) => err,
    };
    assert!(err.contains("extensible"), "unexpected error: {err}");
}

// -- Symbol 键错位回归（shape 槽位计数含 symbol 键） --

// Object.setPrototypeOf：对象目标设置原型并返回原对象；proto 非对象/null 抛 TypeError。
#[test]
fn object_set_prototype_of_sets_prototype_and_validates() {
    let (_vm, result) =
        eval("var o = {}; (Object.setPrototypeOf(o, null) === o) && (Object.getPrototypeOf(o) === null)").unwrap();
    assert!(result.is_bool() && result.as_bool());

    let err = match eval("Object.setPrototypeOf({}, 1)") {
        Ok(_) => panic!("non-object prototype should fail"),
        Err(err) => err,
    };
    assert!(err.contains("TypeError"), "unexpected error: {err}");
}

#[test]
fn entries_values_with_interleaved_symbol_key() {
    let (_vm, result) = eval(
        "var s = Symbol(); var o = {a: 1, [s]: 2, b: 3}; [JSON.stringify(Object.entries(o)), JSON.stringify(Object.values(o))]",
    )
    .unwrap();
    let obj = unsafe { &*result.as_js_object_ptr() };
    let s0 = unsafe { &*obj.get_prop_at(0).as_string_ptr() }.to_owned_string();
    let s1 = unsafe { &*obj.get_prop_at(1).as_string_ptr() }.to_owned_string();
    assert_eq!(s0, "[[\"a\",1],[\"b\",3]]");
    assert_eq!(s1, "[1,3]");
}
