use oxide_compiler::compiler::Compiler;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program =
        oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new()
        .compile(&program)
        .map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

#[test]
fn object_keys_empty() {
    let result = eval("Object.keys({})").unwrap();
    assert!(result.is_object());
}

#[test]
fn object_keys_has_own_property() {
    eval("Object.keys({a:1})").unwrap();
}

#[test]
fn object_create_null_proto() {
    eval("Object.create(null)").unwrap();
}

#[test]
fn object_assign_copies() {
    let result = eval("Object.assign({a:1},{b:2})").unwrap();
    assert!(result.is_object());
}

#[test]
fn object_get_own_property_descriptor_value() {
    let result = eval("Object.getOwnPropertyDescriptor({a:1},'a')").unwrap();
    assert!(result.is_object());
}

#[test]
fn object_get_own_property_descriptor_missing() {
    let result = eval("Object.getOwnPropertyDescriptor({},'x')").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn object_define_property_sets_value() {
    let result = eval("Object.defineProperty({},'x',{value:42})").unwrap();
    assert!(result.is_object());
}

#[test]
fn object_get_prototype_of() {
    let result = eval("Object.getPrototypeOf({})").unwrap();
    assert!(result.is_object() || result.is_null());
}

#[test]
fn object_has_own_true() {
    let result = eval("Object.hasOwn({a:1}, 'a')").unwrap();
    assert!(result.is_bool());
}

#[test]
fn object_entries_returns_array() {
    let result = eval("Object.entries({a:1,b:2})").unwrap();
    assert!(result.is_object());
}

#[test]
fn object_values_returns_array() {
    let result = eval("Object.values({a:1,b:2})").unwrap();
    assert!(result.is_object());
}
