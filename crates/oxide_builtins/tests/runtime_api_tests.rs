use oxide_runtime_api::{same_value_zero, to_integer_or_infinity, to_length};
use oxide_types::value::JsValue;

#[test]
fn test_to_integer_or_infinity_nan() {
    assert_eq!(to_integer_or_infinity(JsValue::float(f64::NAN)), 0.0);
}

#[test]
fn test_to_integer_or_infinity_zero() {
    assert_eq!(to_integer_or_infinity(JsValue::float(0.0)), 0.0);
    assert_eq!(to_integer_or_infinity(JsValue::float(-0.0)), 0.0);
}

#[test]
fn test_to_integer_or_infinity_infinity() {
    assert_eq!(to_integer_or_infinity(JsValue::float(f64::INFINITY)), f64::INFINITY);
    assert_eq!(to_integer_or_infinity(JsValue::float(f64::NEG_INFINITY)), f64::NEG_INFINITY);
}

#[test]
fn test_to_integer_or_infinity_truncation() {
    assert_eq!(to_integer_or_infinity(JsValue::float(3.7)), 3.0);
    assert_eq!(to_integer_or_infinity(JsValue::float(-3.7)), -3.0);
}

#[test]
fn test_to_integer_or_infinity_int() {
    assert_eq!(to_integer_or_infinity(JsValue::int(42)), 42.0);
}

#[test]
fn test_to_length_nan() {
    assert_eq!(to_length(JsValue::float(f64::NAN)), 0);
}

#[test]
fn test_to_length_negative() {
    assert_eq!(to_length(JsValue::float(-1.0)), 0);
    assert_eq!(to_length(JsValue::int(-42)), 0);
}

#[test]
fn test_to_length_positive() {
    assert_eq!(to_length(JsValue::float(100.0)), 100);
    assert_eq!(to_length(JsValue::int(999)), 999);
}

#[test]
fn test_to_length_overflow_clamp() {
    assert_eq!(to_length(JsValue::float(1e100)), 9_007_199_254_740_991);
}

#[test]
fn test_same_value_zero_nan() {
    assert!(same_value_zero(JsValue::float(f64::NAN), JsValue::float(f64::NAN)));
}

#[test]
fn test_same_value_zero_positive_and_negative_zero() {
    assert!(same_value_zero(JsValue::float(0.0), JsValue::float(-0.0)));
    assert!(same_value_zero(JsValue::float(-0.0), JsValue::float(0.0)));
}

#[test]
fn test_same_value_zero_equal_numbers() {
    assert!(same_value_zero(JsValue::float(42.0), JsValue::float(42.0)));
    assert!(same_value_zero(JsValue::int(42), JsValue::int(42)));
    assert!(same_value_zero(JsValue::int(42), JsValue::float(42.0)));
}

#[test]
fn test_same_value_zero_different_numbers() {
    assert!(!same_value_zero(JsValue::float(1.0), JsValue::float(2.0)));
}

#[test]
fn test_same_value_zero_null_undefined() {
    assert!(same_value_zero(JsValue::null(), JsValue::null()));
    assert!(same_value_zero(JsValue::undefined(), JsValue::undefined()));
    assert!(!same_value_zero(JsValue::null(), JsValue::undefined()));
}
