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
