use super::{JsType, JsValue};
use num_bigint::BigInt;
use proptest::prelude::*;
use proptest::test_runner::TestRunner;

#[test]
fn int_roundtrip_prop() {
    let mut runner = TestRunner::default();
    runner
        .run(&(i32::MIN..=i32::MAX), |v| {
            let val = JsValue::int(v);
            assert!(val.is_int());
            assert_eq!(val.as_int(), v);
            Ok(())
        })
        .unwrap();
}

#[test]
fn float_roundtrip_prop() {
    let mut runner = TestRunner::default();
    runner
        .run(&any::<f64>(), |v| {
            let val = JsValue::float(v);
            assert!(val.is_double());
            if !v.is_nan() {
                assert_eq!(val.as_double(), v);
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn bool_roundtrip_prop() {
    let mut runner = TestRunner::default();
    runner
        .run(&any::<bool>(), |v| {
            let val = JsValue::bool(v);
            assert!(val.is_bool());
            assert_eq!(val.as_bool(), v);
            Ok(())
        })
        .unwrap();
}

#[test]
fn random_u64_type_safety() {
    let mut runner = TestRunner::default();
    runner
        .run(&any::<u64>(), |bits| {
            let val = JsValue::from_bits(bits);
            let matched = [
                val.is_double(),
                val.is_int(),
                val.is_bool(),
                val.is_null(),
                val.is_undefined(),
                val.is_object(),
                val.is_string(),
                val.is_symbol(),
                val.is_bigint(),
            ];
            let count = matched.iter().filter(|&&x| x).count();
            assert_eq!(count, 1, "bits={bits:#018x} matched {count} types");
            Ok(())
        })
        .unwrap();
}

#[test]
fn js_type_matches_tag_checks() {
    let mut runner = TestRunner::default();
    runner
        .run(&any::<u64>(), |bits| {
            let val = JsValue::from_bits(bits);
            let t = val.js_type();
            // 随机位模式上 js_type 与 9 个 is_* 检查 1:1 对拍。
            let expected = if val.is_double() {
                JsType::Double
            } else if val.is_int() {
                JsType::Int
            } else if val.is_bool() {
                JsType::Bool
            } else if val.is_null() {
                JsType::Null
            } else if val.is_undefined() {
                JsType::Undefined
            } else if val.is_object() {
                JsType::Object
            } else if val.is_string() {
                JsType::String
            } else if val.is_symbol() {
                JsType::Symbol
            } else {
                JsType::BigInt
            };
            assert_eq!(t, expected, "bits={bits:#018x} js_type={t:?}");
            assert_eq!(t.is_number(), val.is_int() || val.is_double(), "bits={bits:#018x}");
            Ok(())
        })
        .unwrap();
}

#[test]
fn null_identity() {
    assert_eq!(JsValue::null(), JsValue::null());
}

#[test]
fn undefined_identity() {
    assert_eq!(JsValue::undefined(), JsValue::undefined());
}

#[test]
fn nullish_matches_only_null_and_undefined() {
    assert!(JsValue::null().is_nullish());
    assert!(JsValue::undefined().is_nullish());
    assert!(!JsValue::int(0).is_nullish());
    assert!(!JsValue::bool(false).is_nullish());
    assert!(!JsValue::float(f64::NAN).is_nullish());
}

#[test]
fn canonicalization_idempotent() {
    let mut runner = TestRunner::default();
    runner
        .run(&any::<f64>(), |v| {
            let a = JsValue::float(v);
            let b = JsValue::float(a.as_double());
            assert_eq!(a, b);
            Ok(())
        })
        .unwrap();
}

#[test]
fn string_ptr_roundtrip() {
    use crate::object::JsString;
    let s = Box::new(JsString::new("test".to_string()));
    let ptr: *const JsString = &*s;
    let val = JsValue::string(ptr);
    assert!(val.is_string());
    assert_eq!(val.as_string_ptr(), ptr);
    assert_eq!(unsafe { (*val.as_string_ptr()).as_str() }, "test");
}

#[test]
fn string_pointer_equality() {
    use crate::object::JsString;
    let a = Box::new(JsString::new("x".to_string()));
    let b = Box::new(JsString::new("x".to_string()));
    let va = JsValue::string(&*a);
    let vb = JsValue::string(&*b);
    // 内容相同但分配不同 → NOT ==（指针同一性）。
    // 语义内容相等在 coercion 层处理，不在 PartialEq。
    assert_ne!(va, vb);
    assert_eq!(va, JsValue::string(&*a));
    assert_eq!(unsafe { (*va.as_string_ptr()).as_str() }, unsafe { (*vb.as_string_ptr()).as_str() });
}

#[test]
fn bigint_pointer_roundtrip() {
    let boxed = Box::new(BigInt::from(123));
    let ptr: *const BigInt = &*boxed;
    let val = JsValue::bigint(ptr);
    assert!(val.is_bigint());
    assert_eq!(val.as_bigint_ptr(), ptr);
    assert_eq!(unsafe { &*val.as_bigint_ptr() }, &BigInt::from(123));
    assert!(!val.is_double());
    assert!(!val.is_int());
}

#[test]
fn bigint_value_equality_by_value() {
    let a = JsValue::bigint(Box::into_raw(Box::new(BigInt::from(7))));
    let b = JsValue::bigint(Box::into_raw(Box::new(BigInt::from(7))));
    let c = JsValue::bigint(Box::into_raw(Box::new(BigInt::from(8))));
    assert_eq!(a, b);
    assert_ne!(a, c);
    unsafe {
        drop(Box::from_raw(a.as_bigint_ptr() as *mut BigInt));
        drop(Box::from_raw(b.as_bigint_ptr() as *mut BigInt));
        drop(Box::from_raw(c.as_bigint_ptr() as *mut BigInt));
    }
}
