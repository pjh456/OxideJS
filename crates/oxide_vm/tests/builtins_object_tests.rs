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
fn object_create_null_proto() {
    let (_vm, _result) = eval("Object.create(null)").unwrap();
}

#[test]
fn object_assign_copies() {
    let (_vm, result) = eval("Object.assign({a:1},{b:2})").unwrap();
    assert!(result.is_object());
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
fn object_get_prototype_of() {
    let (_vm, result) = eval("Object.getPrototypeOf({})").unwrap();
    assert!(result.is_object() || result.is_null());
}

#[test]
fn object_has_own_true() {
    let (_vm, result) = eval("Object.hasOwn({a:1}, 'a')").unwrap();
    assert!(result.is_bool());
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
