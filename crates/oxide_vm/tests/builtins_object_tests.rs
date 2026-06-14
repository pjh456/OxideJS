use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&module)?;
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
    let rendered = _vm
        .kernel_core()
        .string_forge()
        .lookup(result.as_string_index())
        .unwrap_or_default();
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
fn object_define_property_non_writable_assignment_throws() {
    let err = match eval("var o={}; Object.defineProperty(o,'x',{value:1}); o.x=2") {
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
    assert_eq!(result, JsValue::bool(true));
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
