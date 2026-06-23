use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_types::value::JsValue;
use oxide_vm::coercion;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

#[test]
fn eval_coercion_null_equals_null() {
    assert_eq!(eval("null == null"), "true");
}

#[test]
fn eval_coercion_bool_equals_int() {
    assert_eq!(eval("false == 0"), "true");
}

#[test]
fn eval_not_falsy() {
    assert_eq!(eval("!0"), "true");
}

#[test]
fn test_same_value_signed_zero() {
    assert!(!coercion::same_value(JsValue::float(0.0), JsValue::float(-0.0)));
}

#[test]
fn test_strict_equality_signed_zero() {
    assert!(coercion::strict_equality(JsValue::float(0.0), JsValue::float(-0.0)));
}

#[test]
fn test_same_value_nan() {
    assert!(coercion::same_value(JsValue::float(f64::NAN), JsValue::float(f64::NAN)));
}

#[test]
fn test_same_value_type_mismatch() {
    assert!(!coercion::same_value(JsValue::int(1), JsValue::bool(true)));
}

#[test]
fn test_same_value_int_float_equal() {
    assert!(coercion::same_value(JsValue::int(1), JsValue::float(1.0)));
    assert!(!coercion::same_value(JsValue::int(1), JsValue::float(2.0)));
}

#[test]
fn test_strict_equality_nan() {
    assert!(!coercion::strict_equality(JsValue::float(f64::NAN), JsValue::float(f64::NAN)));
}

#[test]
fn test_strict_equality_type_mismatch() {
    assert!(!coercion::strict_equality(JsValue::int(1), JsValue::bool(true)));
}

#[test]
fn test_strict_equality_int_float_equal() {
    assert!(coercion::strict_equality(JsValue::int(1), JsValue::float(1.0)));
    assert!(!coercion::strict_equality(JsValue::int(1), JsValue::float(2.0)));
}

#[test]
fn test_strict_equality_null_undefined() {
    assert!(!coercion::strict_equality(JsValue::null(), JsValue::undefined()));
}

#[test]
fn test_to_int32_and_to_uint32() {
    let vm = Vm::new();
    let sf = vm.kernel_core().string_forge().as_ref();
    let string_three = {
        let (idx, hash) = vm.kernel_core().string_forge().intern("3");
        JsValue::string(idx, hash)
    };
    let string_bad = {
        let (idx, hash) = vm.kernel_core().string_forge().intern("x");
        JsValue::string(idx, hash)
    };

    assert_eq!(coercion::to_int32(string_three, sf), 3);
    assert_eq!(coercion::to_int32(string_bad, sf), 0);
    assert_eq!(coercion::to_int32(JsValue::undefined(), sf), 0);
    assert_eq!(coercion::to_int32(JsValue::float(1.9), sf), 1);
    assert_eq!(coercion::to_int32(JsValue::float(-1.9), sf), -1);
    assert_eq!(coercion::to_int32(JsValue::float(f64::NAN), sf), 0);
    assert_eq!(coercion::to_int32(JsValue::float(f64::INFINITY), sf), 0);
    assert_eq!(coercion::to_uint32(JsValue::int(-1), sf), 4_294_967_295);
}

#[test]
fn primitive_property_write_throws_clean_type_error() {
    for source in ["'abc'.x = 1", "(5).foo = 1", "true.foo = 1"] {
        let err = eval(source);
        assert!(err.contains("TypeError"), "expected TypeError for {source}, got {err}");
        assert!(
            !err.contains("IC_SET_PROP on non-object")
                && !err.contains("SET_PROP on non-object")
                && !err.contains("SET_PROP_DYNAMIC on non-object"),
            "unexpected internal opcode message for {source}: {err}"
        );
    }
}
