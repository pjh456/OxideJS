use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
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

// -- Delete configurable property succeeds --
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

// -- Delete non-configurable property throws --
#[test]
fn delete_non_configurable_throws_type_error() {
    let result = eval_many(&[
        "var obj = {}",
        "Object.defineProperty(obj, 'x', {value: 1, configurable: false})",
        "delete obj.x",
    ]);
    assert_err_contains(result, "cannot delete non-configurable property");
}

// -- Delete non-existent property returns true --
#[test]
fn delete_non_existent_returns_true() {
    let r = eval_many(&["var obj = {}", "delete obj.x"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "delete non-existent should return true, got {:?}", r);
}

// -- Delete default (no explicit defineProperty) configurable succeeds --
#[test]
fn delete_default_configurable_succeeds() {
    let r = eval_many(&["var obj = {}", "obj.x = 1", "delete obj.x"]).unwrap();
    assert!(r.is_bool() && r.as_bool(), "delete default configurable should return true, got {:?}", r);
}

// -- Delete non-configurable proto property on child: child doesn't own it, so returns true --
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
