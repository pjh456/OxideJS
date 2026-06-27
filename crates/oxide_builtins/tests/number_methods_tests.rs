use oxide_runtime_api::to_number;
use oxide_types::value::JsValue;

#[test]
fn is_nan_nan_returns_true() {
    assert!(to_number(JsValue::float(f64::NAN)).is_nan());
}

#[test]
fn is_nan_number_123_returns_false() {
    assert!(!to_number(JsValue::int(123)).is_nan());
}

#[test]
fn is_nan_undefined_returns_true() {
    assert!(to_number(JsValue::undefined()).is_nan());
}

#[test]
fn is_finite_number_42_returns_true() {
    assert!(to_number(JsValue::int(42)).is_finite());
}

#[test]
fn is_finite_infinity_returns_false() {
    assert!(!to_number(JsValue::float(f64::INFINITY)).is_finite());
}

#[test]
fn is_integer_non_number_float_nan() {
    let val = JsValue::undefined();
    assert!(!val.is_int() && !val.is_double());
}

#[test]
fn is_integer_int_42() {
    let val = JsValue::int(42);
    assert!(val.is_int());
    let n = to_number(val);
    assert!(n.trunc() == n && n.is_finite());
}

#[test]
fn is_integer_double_fractional() {
    let val = JsValue::float(42.5);
    assert!(val.is_double());
    let n = to_number(val);
    assert!(!(n.trunc() == n && n.is_finite()));
}

#[test]
fn is_safe_integer_max_value() {
    let val = JsValue::float(9007199254740991f64);
    let n = to_number(val);
    assert!(n.trunc() == n && n.is_finite());
    assert!(n >= -9007199254740991i64 as f64 && n <= 9007199254740991i64 as f64);
}

#[test]
fn is_safe_integer_overflow() {
    let val = JsValue::float(9007199254740992f64);
    let n = to_number(val);
    assert!(n.trunc() == n && n.is_finite());
    assert!(!(n >= -9007199254740991i64 as f64 && n <= 9007199254740991i64 as f64));
}
