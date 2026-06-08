use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

fn string_value(vm: &Vm, val: JsValue) -> String {
    vm.kernel().string_forge().lookup(val.as_string_index()).unwrap_or_default()
}

// -- JSON.parse --

#[test]
fn json_parse_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('{\"a\":1,\"b\":2}')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
}

#[test]
fn json_parse_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('[1,2,3]')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_array());
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(obj.get_prop_at(2).as_double(), 3.0);
}

#[test]
fn json_parse_string() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('\"hello\"')").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "hello");
}

#[test]
fn json_parse_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('42')").unwrap();
    assert!(result.is_double());
}

#[test]
fn json_parse_float() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('3.14')").unwrap();
    assert!(result.is_double());
}

#[test]
fn json_parse_true() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('true')").unwrap();
    assert!(result.is_bool());
    assert!(result.as_bool());
}

#[test]
fn json_parse_false() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('false')").unwrap();
    assert!(result.is_bool());
    assert!(!result.as_bool());
}

#[test]
fn json_parse_null() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('null')").unwrap();
    assert!(result.is_null());
}

#[test]
fn json_parse_nested() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('{\"a\":1,\"b\":[1,2],\"c\":{\"d\":\"hello\"}}')").unwrap();
    assert!(result.is_object());
}

#[test]
fn json_parse_invalid_throws() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "JSON.parse('not json')");
    assert!(err.is_err());
}

// -- JSON.stringify --

#[test]
fn json_stringify_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify({a:1})").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "{\"a\":1}");
}

#[test]
fn json_stringify_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify([1,2,3])").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "[1,2,3]");
}

#[test]
fn json_stringify_string() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify('hello')").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "\"hello\"");
}

#[test]
fn json_stringify_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(42)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "42");
}

#[test]
fn json_stringify_float() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(3.14)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert!(s.starts_with("3.14"));
}

#[test]
fn json_stringify_true() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(true)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "true");
}

#[test]
fn json_stringify_false() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(false)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "false");
}

#[test]
fn json_stringify_null() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(null)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "null");
}

#[test]
fn json_stringify_undefined() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(undefined)").unwrap();
    assert!(result.is_undefined());
}

// -- Roundtrip --

#[test]
fn json_parse_stringify_roundtrip() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse(JSON.stringify({a:1,b:2}))").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_vec_len(), 2);
}

#[test]
fn json_roundtrip_array() {
    let mut vm = Vm::new();
    let _ = eval(&mut vm, "var arr = JSON.parse(JSON.stringify([1,2,3])); arr").unwrap();
}

// -- Edge cases --

#[test]
fn json_stringify_empty_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify({})").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "{}");
}

#[test]
fn json_stringify_empty_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify([])").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "[]");
}

#[test]
fn json_stringify_escape_quotes() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify('he\"llo')").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "\"he\\\"llo\"");
}

#[test]
fn json_stringify_escape_backslash() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify('a\\\\b')").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "\"a\\\\b\"");
}

#[test]
fn json_stringify_nested_objects() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify({a: {b: 1}})").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "{\"a\":{\"b\":1}}");
}

#[test]
fn json_parse_empty_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('{}')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_vec_len(), 0);
}

#[test]
fn json_parse_empty_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('[]')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_array());
    assert_eq!(obj.prop_vec_len(), 0);
}

#[test]
fn json_parse_int_is_double() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('123')").unwrap();
    assert!((result.as_double() - 123.0).abs() < 0.0001);
}

// -- Cycle detection --

#[test]
fn json_stringify_cycle_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "var a = {}; a.self = a; JSON.stringify(a)");
    assert!(err.is_err());
}
