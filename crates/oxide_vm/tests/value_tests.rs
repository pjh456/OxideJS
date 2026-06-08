use oxide_vm::JsValue;

#[test]
fn int_roundtrip() {
    for v in [0, 1, -1, 42, i32::MIN, i32::MAX] {
        let val = JsValue::int(v);
        assert!(val.is_int(), "int({v}) should be int");
        assert_eq!(val.as_int(), v, "int({v}) roundtrip failed");
    }
}

#[test]
fn float_roundtrip() {
    for v in [0.0, -0.0, 1.0, -1.0, 42.5, f64::MIN, f64::MAX] {
        let val = JsValue::float(v);
        assert!(val.is_double(), "float({v}) should be double");
        assert_eq!(val.as_double(), v, "float({v}) roundtrip failed");
    }
}

#[test]
fn float_infinity() {
    let val = JsValue::float(f64::INFINITY);
    assert!(val.is_double());
    assert!(val.as_double().is_infinite());
    assert!(val.as_double().is_sign_positive());

    let val = JsValue::float(f64::NEG_INFINITY);
    assert!(val.is_double());
    assert!(val.as_double().is_infinite());
    assert!(val.as_double().is_sign_negative());
}

#[test]
fn float_nan_canonicalization() {
    let val = JsValue::float(f64::NAN);
    assert!(val.is_double(), "NaN should be classified as double");
    let d = val.as_double();
    assert!(d.is_nan(), "NaN value should still be NaN");
}

#[test]
fn bool_roundtrip() {
    let t = JsValue::bool(true);
    assert!(t.is_bool());
    assert!(t.as_bool());

    let f = JsValue::bool(false);
    assert!(f.is_bool());
    assert!(!f.as_bool());
}

#[test]
fn null_type_check() {
    let val = JsValue::null();
    assert!(val.is_null());
    assert!(!val.is_int());
    assert!(!val.is_double());
    assert!(!val.is_bool());
    assert!(!val.is_undefined());
    assert!(!val.is_object());
}

#[test]
fn undefined_type_check() {
    let val = JsValue::undefined();
    assert!(val.is_undefined());
    assert!(!val.is_int());
    assert!(!val.is_double());
    assert!(!val.is_bool());
    assert!(!val.is_null());
    assert!(!val.is_object());
}

#[test]
fn null_equals_null() {
    assert_eq!(JsValue::null(), JsValue::null());
}

#[test]
fn null_not_equals_undefined() {
    assert_ne!(JsValue::null(), JsValue::undefined());
}

#[test]
fn object_pointer_roundtrip() {
    let x = 42u8;
    let val = JsValue::object(&x as *const u8);
    assert!(val.is_object());
    let ptr = val.as_ptr();
    assert_eq!(ptr, &x as *const u8);
}

#[test]
fn as_ptr_non_object_returns_null() {
    assert!(JsValue::int(0).as_ptr().is_null());
    assert!(JsValue::float(0.0).as_ptr().is_null());
    assert!(JsValue::bool(true).as_ptr().is_null());
    assert!(JsValue::null().as_ptr().is_null());
    assert!(JsValue::undefined().as_ptr().is_null());
    let x = 42u8;
    assert!(!JsValue::object(&x as *const u8).as_ptr().is_null());
}

#[test]
fn as_object_ptr_non_object_returns_null() {
    assert!(JsValue::int(0).as_object_ptr().is_null());
    assert!(JsValue::float(0.0).as_object_ptr().is_null());
    assert!(JsValue::bool(true).as_object_ptr().is_null());
    assert!(JsValue::null().as_object_ptr().is_null());
    assert!(JsValue::undefined().as_object_ptr().is_null());
    let x = 42u8;
    assert!(!JsValue::object(&x as *const u8).as_object_ptr().is_null());
}

#[test]
fn as_js_object_ptr_non_object_returns_null() {
    assert!(JsValue::int(0).as_js_object_ptr().is_null());
    assert!(JsValue::float(0.0).as_js_object_ptr().is_null());
    assert!(JsValue::bool(true).as_js_object_ptr().is_null());
    assert!(JsValue::null().as_js_object_ptr().is_null());
    assert!(JsValue::undefined().as_js_object_ptr().is_null());
    let x = 42u8;
    assert!(!JsValue::object(&x as *const u8).as_js_object_ptr().is_null());
}

#[test]
fn display_int() {
    assert_eq!(format!("{}", JsValue::int(42)), "42");
    assert_eq!(format!("{}", JsValue::int(-1)), "-1");
}

#[test]
fn display_double() {
    let val = JsValue::float(42.5);
    assert!(format!("{}", val).contains("42.5"));
}

#[test]
fn display_bool() {
    assert_eq!(format!("{}", JsValue::bool(true)), "true");
    assert_eq!(format!("{}", JsValue::bool(false)), "false");
}

#[test]
fn display_null() {
    assert_eq!(format!("{}", JsValue::null()), "null");
}

#[test]
fn display_undefined() {
    assert_eq!(format!("{}", JsValue::undefined()), "undefined");
}

#[test]
fn display_nan() {
    assert_eq!(format!("{}", JsValue::float(f64::NAN)), "NaN");
}

#[test]
fn display_infinity() {
    assert_eq!(format!("{}", JsValue::float(f64::INFINITY)), "Infinity");
}

#[test]
fn debug_int() {
    assert_eq!(format!("{:?}", JsValue::int(42)), "JsValue(Int(42))");
}

#[test]
fn debug_double() {
    let val = JsValue::float(42.5);
    assert!(format!("{:?}", val).starts_with("JsValue(Double("));
}

#[test]
fn debug_bool() {
    assert_eq!(format!("{:?}", JsValue::bool(true)), "JsValue(Bool(true))");
}

#[test]
fn debug_null() {
    assert_eq!(format!("{:?}", JsValue::null()), "JsValue(Null)");
}

#[test]
fn debug_undefined() {
    assert_eq!(format!("{:?}", JsValue::undefined()), "JsValue(Undefined)");
}

#[test]
fn debug_object() {
    let x = 0u8;
    let val = JsValue::object(&x as *const u8);
    assert!(format!("{:?}", val).starts_with("JsValue(Object("));
}

#[test]
fn copy_semantics() {
    let a = JsValue::int(42);
    let b = a;
    assert_eq!(a, b);
    assert_eq!(a.as_int(), 42);
    assert_eq!(b.as_int(), 42);
}

#[test]
fn type_exclusivity_int() {
    let val = JsValue::int(1);
    assert!(val.is_int());
    assert!(!val.is_double());
    assert!(!val.is_bool());
    assert!(!val.is_null());
    assert!(!val.is_undefined());
    assert!(!val.is_object());
}

#[test]
fn type_exclusivity_double() {
    let val = JsValue::float(0.0);
    assert!(val.is_double());
    assert!(!val.is_int());
    assert!(!val.is_bool());
    assert!(!val.is_null());
    assert!(!val.is_undefined());
    assert!(!val.is_object());
}

#[test]
fn type_exclusivity_object() {
    let x = 0u8;
    let val = JsValue::object(&x as *const u8);
    assert!(val.is_object());
    assert!(!val.is_int());
    assert!(!val.is_double());
    assert!(!val.is_bool());
    assert!(!val.is_null());
    assert!(!val.is_undefined());
}
