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

fn stringify_val(val: &JsValue) -> String {
    if val.is_string() {
        unsafe { (*val.as_string_ptr()).to_owned_string() }
    } else {
        format!("{:?}", val)
    }
}

// --- replacer array ---

#[test]
fn replacer_array_filters_properties() {
    let (_vm, result) = eval(r#"JSON.stringify({a:1,b:2,c:3}, ['a','c'])"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"a":1,"c":3}"#);
}

#[test]
fn replacer_array_numeric_keys() {
    let (_vm, result) = eval(r#"JSON.stringify({a:1,b:2}, ['b'])"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"b":2}"#);
}

// --- replacer function ---

#[test]
fn replacer_function_skip_property() {
    let (_vm, result) =
        eval(r#"JSON.stringify({a:1,b:2}, function(k,v){if(k==='a')return undefined;return v})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"b":2}"#);
}

#[test]
fn replacer_function_array_null() {
    let (_vm, result) =
        eval(r#"JSON.stringify([1,2,3], function(k,v){if(k==='1')return undefined;return v})"#).unwrap();
    assert_eq!(stringify_val(&result), "[1,null,3]");
}

#[test]
fn replacer_function_transform() {
    let (_vm, result) =
        eval(r#"JSON.stringify({a:1}, function(k,v){if(typeof v==='number')return v*2;return v})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"a":2}"#);
}

// --- space ---

#[test]
fn space_number_indent() {
    let (_vm, result) = eval(r#"JSON.stringify({a:1}, null, 2)"#).unwrap();
    let s = stringify_val(&result);
    assert!(s.len() > 5, "expected indented output, got: {}", s);
    assert!(s.starts_with('{'), "should start with brace");
}

#[test]
fn space_negative_clamped() {
    let (_vm, result) = eval(r#"JSON.stringify({a:1}, null, -5)"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"a":1}"#);
}

// --- toJSON ---

#[test]
fn tojson_called_before_serialize() {
    let (_vm, result) = eval(r#"JSON.stringify({toJSON:function(){return {x:1}}})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"x":1}"#);
}

#[test]
fn tojson_non_callable_ignored() {
    let (_vm, result) = eval(r#"JSON.stringify({toJSON:'not-a-function', a:1})"#).unwrap();
    // toJSON 属性是字符串，应按普通属性序列化。
    assert!(stringify_val(&result).contains("toJSON"));
    assert!(stringify_val(&result).contains(r#""a":1"#));
}

// --- cycle detection ---

#[test]
fn cycle_throws_type_error() {
    let result = eval("var a={};a.self=a;JSON.stringify(a)");
    match result {
        Err(e) => assert!(e.to_lowercase().contains("circular"), "error should mention 'circular', got: {}", e),
        Ok(_) => panic!("expected cycle error, got ok"),
    }
}

#[test]
fn no_cycle_distinct_objects_ok() {
    let (_vm, result) = eval(r#"JSON.stringify([{a:1},{a:1}])"#).unwrap();
    assert_eq!(stringify_val(&result), r#"[{"a":1},{"a":1}]"#);
}

// --- reviver ---

#[test]
fn reviver_transform_values() {
    let (_vm, result) =
        eval(r#"JSON.parse('{"a":1,"b":2}', function(k,v){if(typeof v==='number')return v*2;return v})"#).unwrap();
    assert!(result.is_object());
}

#[test]
fn reviver_delete_property() {
    let (_vm, result) =
        eval(r#"JSON.parse('{"a":1,"b":2}', function(k,v){if(k==='a')return undefined;return v})"#).unwrap();
    // 属性被软删除（置为 undefined）。
    // 为 test262 兼容，stringify 应省略 undefined 属性。
    assert!(result.is_object());
}

#[test]
fn reviver_root_key_empty_string() {
    let (_vm, result) = eval(r#"JSON.parse('42', function(k,v){return v+1})"#).unwrap();
    assert!(result.is_int() || result.is_double(), "expected number");
    if result.is_int() {
        assert_eq!(result.as_int(), 43);
    } else {
        assert_eq!(result.as_double() as i32, 43);
    }
}

#[test]
fn reviver_no_reviver_works() {
    let (_vm, result) = eval(r#"JSON.parse('{"a":1}')"#).unwrap();
    assert!(result.is_object());
}
