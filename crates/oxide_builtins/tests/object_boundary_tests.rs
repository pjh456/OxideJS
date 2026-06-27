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
fn object_keys_no_args_type_error() {
    assert!(eval("Object.keys()").is_err(), "Object.keys() should throw TypeError");
}

#[test]
fn object_keys_non_object_type_error() {
    assert!(eval("Object.keys(42)").is_err(), "Object.keys(42) should throw TypeError");
}

#[test]
fn object_create_null_proto() {
    let (_vm, result) = eval("var o = Object.create(null); o").unwrap();
    assert!(result.is_object());
    let obj_ptr = result.as_js_object_ptr();
    assert!(!obj_ptr.is_null());
    let obj = unsafe { &*obj_ptr };
    let proto = obj.proto();
    assert!(proto.is_null(), "Object.create(null) should set prototype to null");
}

#[test]
fn object_create_number_proto_type_error() {
    assert!(eval("Object.create(42)").is_err(), "Object.create(42) should throw TypeError");
}

#[test]
fn object_create_no_args_type_error() {
    assert!(eval("Object.create()").is_err(), "Object.create() should throw TypeError");
}

#[test]
fn object_get_own_property_names_no_args_type_error() {
    assert!(
        eval("Object.getOwnPropertyNames()").is_err(),
        "Object.getOwnPropertyNames() should throw TypeError"
    );
}

#[test]
fn object_get_own_property_names_non_object_type_error() {
    assert!(
        eval("Object.getOwnPropertyNames(42)").is_err(),
        "Object.getOwnPropertyNames(42) should throw TypeError"
    );
}

#[test]
fn object_keys_valid_object() {
    let (_vm, result) = eval("var o = {a:1, b:2}; Object.keys(o).join(',')").unwrap();
    assert!(result.is_string(), "Object.keys should return an array");
}

#[test]
fn test_define_property_data_and_accessor_conflict() {
    assert!(
        eval("Object.defineProperty({}, 'x', {value: 1, get: function() {}})").is_err(),
        "data+accessor conflict should throw TypeError"
    );
}

#[test]
fn test_define_property_non_object_target() {
    assert!(
        eval("Object.defineProperty(null, 'x', {value: 1})").is_err(),
        "non-object target should throw TypeError"
    );
}

#[test]
fn test_define_property_missing_descriptor() {
    assert!(
        eval("Object.defineProperty({}, 'x')").is_err(),
        "missing descriptor should throw TypeError"
    );
}

#[test]
fn test_assign_skips_null_source() {
    let (_vm, result) = eval("var t = {a:1}; Object.assign(t, null); t.a").unwrap();
    assert_eq!(result.as_int(), 1, "Object.assign should skip null sources");
}

#[test]
fn test_assign_merges_two_sources() {
    let (_vm, result) = eval("var t = {a:1}; Object.assign(t, {b:2}); t.b").unwrap();
    assert!(result.is_int() || result.is_double(), "Object.assign should copy from source");
}

#[test]
fn test_integrity_freeze_returns_object() {
    let (_vm, result) = eval("var o = {x:1}; Object.freeze(o) === o").unwrap();
    assert!(result.is_bool() && result.as_bool(), "freeze should return the object");
}

#[test]
fn test_integrity_freeze_sets_non_extensible() {
    let (_vm, result) = eval("var o = {x:1}; Object.freeze(o); Object.isExtensible(o)").unwrap();
    assert!(result.is_bool() && !result.as_bool(), "freeze should set non-extensible");
}

#[test]
fn test_integrity_seal_non_extensible() {
    let (_vm, result) = eval("var o = {x:1}; Object.seal(o); Object.isExtensible(o)").unwrap();
    assert!(result.is_bool() && !result.as_bool(), "seal should set non-extensible");
}

#[test]
fn test_integrity_prevent_extensions() {
    let (_vm, result) = eval("var o = {x:1}; Object.preventExtensions(o); Object.isExtensible(o)").unwrap();
    assert!(result.is_bool() && !result.as_bool(), "preventExtensions should set non-extensible");
}

#[test]
fn test_is_frozen_non_object_returns_true() {
    let (_vm, result) = eval("Object.isFrozen(42)").unwrap();
    assert!(result.is_bool() && result.as_bool(), "non-object should be frozen per ES spec");
}

#[test]
fn test_is_sealed_non_object_returns_true() {
    let (_vm, result) = eval("Object.isSealed(42)").unwrap();
    assert!(result.is_bool() && result.as_bool(), "non-object should be sealed per ES spec");
}

#[test]
fn test_is_extensible_non_object_returns_false() {
    let (_vm, result) = eval("Object.isExtensible(42)").unwrap();
    assert!(result.is_bool() && !result.as_bool(), "non-object should not be extensible per ES spec");
}

#[test]
fn test_is_extensible_no_args_type_error() {
    assert!(eval("Object.isExtensible()").is_err());
}

#[test]
fn test_get_prototype_of_null_throws_type_error() {
    assert!(eval("Object.getPrototypeOf(null)").is_err());
}

#[test]
fn test_get_prototype_of_undefined_throws_type_error() {
    assert!(eval("Object.getPrototypeOf(undefined)").is_err());
}

#[test]
fn test_get_prototype_of_plain_object() {
    let (_vm, result) = eval("var o = {}; Object.getPrototypeOf(o)").unwrap();
    assert!(result.is_object() || result.is_null());
}

#[test]
fn test_has_own_own_property() {
    let (_vm, result) = eval("var o = {a:1}; Object.hasOwn(o, 'a')").unwrap();
    assert!(result.is_bool() && result.as_bool());
}

#[test]
fn test_has_own_inherited_property() {
    let (_vm, result) = eval("Object.hasOwn({}, 'toString')").unwrap();
    assert!(result.is_bool() && !result.as_bool());
}

#[test]
fn test_define_properties_no_args_type_error() {
    assert!(eval("Object.defineProperties()").is_err());
}

#[test]
fn test_define_properties_applies_multiple_props() {
    let (_vm, result) = eval("var o = {}; Object.defineProperties(o, {a: {value: 1}, b: {value: 2}}); o.a").unwrap();
    assert!(result.is_int() || result.is_double());
}

#[test]
fn test_from_entries_no_args_type_error() {
    assert!(eval("Object.fromEntries()").is_err());
}

#[test]
fn test_from_entries_empty_array() {
    let (_vm, result) = eval("Object.fromEntries([])").unwrap();
    assert!(result.is_object(), "fromEntries([]) should return an empty object");
}

#[test]
fn test_object_is_nan() {
    let (_vm, result) = eval("Object.is(NaN, NaN)").unwrap();
    assert!(result.is_bool() && result.as_bool());
}

#[test]
fn test_object_is_signed_zero() {
    let (_vm, result) = eval("Object.is(0, -0)").unwrap();
    assert!(result.is_bool() && !result.as_bool());
}

#[test]
fn test_entries_returns_key_value_pairs() {
    let (_vm, result) = eval("var o = {a:1, b:2}; Object.entries(o).length").unwrap();
    assert!(result.is_int() || result.is_double());
}

#[test]
fn test_values_returns_array() {
    let (_vm, result) = eval("var o = {a:1, b:2}; Object.values(o).length").unwrap();
    assert!(result.is_int() || result.is_double());
}

#[test]
fn test_get_own_property_descriptor_no_args_type_error() {
    assert!(eval("Object.getOwnPropertyDescriptor()").is_err());
}

#[test]
fn test_has_own_property_non_object_this_type_error() {
    assert!(eval("Object.prototype.hasOwnProperty.call(42, 'x')").is_err());
}

#[test]
fn test_property_is_enumerable_own_property() {
    let (_vm, result) = eval("var o = {x: 1}; o.propertyIsEnumerable('x')").unwrap();
    assert!(result.is_bool() && result.as_bool());
}
