use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&module)
}

fn eval_with_vm(source: &str) -> (Vm, Result<JsValue, String>) {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&module);
    (vm, result)
}

fn str_val(vm: &Vm, val: JsValue) -> String {
    vm.kernel_core()
        .string_forge()
        .lookup(val.as_string_index())
        .unwrap_or_default()
}

#[test]
fn eval_nan_returns_nan() {
    let result = eval("NaN").unwrap();
    assert!(result.is_double());
    assert!(result.as_double().is_nan());
}

#[test]
fn eval_undefined_returns_undefined() {
    let result = eval("undefined").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn typeof_nan_is_number() {
    let result = eval("typeof NaN").unwrap();
    assert!(result.is_string());
}

#[test]
fn typeof_undefined_is_undefined() {
    let result = eval("typeof undefined").unwrap();
    assert!(result.is_string());
}

#[test]
fn eval_infinity_returns_infinity() {
    let result = eval("Infinity").unwrap();
    assert!(result.is_double());
    assert!(result.as_double().is_infinite());
    assert!(result.as_double().is_sign_positive());
}

#[test]
fn eval_neg_infinity_via_division() {
    let result = eval("1 / Infinity").unwrap();
    assert!(result.is_double());
    assert_eq!(result.as_double(), 0.0);
}

#[test]
fn typeof_infinity_is_number() {
    let result = eval("typeof Infinity").unwrap();
    assert!(result.is_string());
}

#[test]
fn infinity_ne_nan() {
    let result = eval("Infinity !== NaN").unwrap();
    assert!(result.is_bool());
    assert!(result.as_bool());
}

#[test]
fn infinity_gt_large_number() {
    let result = eval("Infinity > 1e308").unwrap();
    assert!(result.is_bool());
    assert!(result.as_bool());
}

#[test]
fn division_by_zero_gives_infinity() {
    let result = eval("1 / 0").unwrap();
    assert!(result.is_double());
    assert!(result.as_double().is_infinite());
}

#[test]
fn global_this_points_to_global_object() {
    let result = eval("globalThis.Object === Object").unwrap();
    assert!(result.is_bool());
    assert!(result.as_bool());
}

#[test]
fn encode_uri_preserves_reserved_chars() {
    let (vm, result) = eval_with_vm("encodeURI('https://example.com/path?q=hello world')");
    assert_eq!(str_val(&vm, result.unwrap()), "https://example.com/path?q=hello%20world");
}

#[test]
fn encode_uri_component_encodes_reserved_chars() {
    let (vm, result) = eval_with_vm("encodeURIComponent('a=1&b=2')");
    assert_eq!(str_val(&vm, result.unwrap()), "a%3D1%26b%3D2");
}

#[test]
fn decode_uri_preserves_escaped_reserved_chars() {
    let (vm, result) = eval_with_vm("decodeURI('https://example.com/path%3Fq=hello%20world')");
    assert_eq!(str_val(&vm, result.unwrap()), "https://example.com/path%3Fq=hello world");
}

#[test]
fn decode_uri_component_decodes_reserved_chars() {
    let (vm, result) = eval_with_vm("decodeURIComponent('a%3D1%26b%3D2')");
    assert_eq!(str_val(&vm, result.unwrap()), "a=1&b=2");
}

#[test]
fn decode_uri_component_malformed_sequence_throws_uri_error() {
    let result = eval("try { decodeURIComponent('%') } catch (e) { e instanceof URIError }").unwrap();
    assert!(result.is_bool());
    assert!(result.as_bool());
}
