use std::fmt;

use crate::object::JsObject;

/// Quiet NaN prefix — bits 63-51 = sign(1) + exponent(0x7FF) + quiet_bit(1)
const QNAN_PREFIX: u64 = 0xFFF8_0000_0000_0000;

/// NaN exponent mask — bits 62-52
const EXP_MASK: u64 = 0x7FF0_0000_0000_0000;

/// Mantissa mask — bits 51-0
const MANTISSA_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;

/// Tag sits in bits 50-48 of mantissa
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const TAG_SHIFT: u64 = 48;
const TAG_INT: u64 = 0;
const TAG_BOOL: u64 = 1;
const TAG_NULL: u64 = 2;
const TAG_UNDEFINED: u64 = 3;
const TAG_OBJECT: u64 = 4;
const TAG_STRING: u64 = 5;

/// 48-bit pointer mask (x86-64 canonical VA)
const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// 32-bit integer payload mask
const INT_MASK: u64 = 0x0000_0000_FFFF_FFFF;

/// String payload: [hash_prefix:16bit][table_index:32bit]
const STRING_HASH_SHIFT: u64 = 32;
const STRING_HASH_MASK: u64 = 0xFFFF_0000_0000;
const STRING_INDEX_MASK: u64 = 0x0000_FFFF_FFFF;

fn make_tag(tag: u64) -> u64 {
    QNAN_PREFIX | (tag << TAG_SHIFT)
}

fn get_tag(bits: u64) -> u64 {
    (bits & TAG_MASK) >> TAG_SHIFT
}

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct JsValue(u64);

impl PartialEq for JsValue {
    fn eq(&self, other: &Self) -> bool {
        if self.is_int() && other.is_int() {
            return self.as_int() == other.as_int();
        }
        if self.is_double() && other.is_double() {
            let a = self.as_double();
            let b = other.as_double();
            if a.is_nan() || b.is_nan() {
                return false;
            }
            return a == b;
        }
        if self.is_bool() && other.is_bool() {
            return self.as_bool() == other.as_bool();
        }
        if self.is_null() && other.is_null() {
            return true;
        }
        if self.is_undefined() && other.is_undefined() {
            return true;
        }
        if self.is_object() && other.is_object() {
            return self.as_ptr() == other.as_ptr();
        }
        if self.is_string() && other.is_string() {
            let ha = self.as_string_hash();
            let hb = other.as_string_hash();
            if ha != hb {
                return false;
            }
            return self.as_string_index() == other.as_string_index();
        }
        false
    }
}

impl JsValue {
    #[allow(dead_code)]
    pub(crate) fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub fn int(v: i32) -> Self {
        Self(make_tag(TAG_INT) | (v as u32 as u64))
    }

    pub fn float(v: f64) -> Self {
        let bits = v.to_bits();
        if is_nan_bits(bits) {
            Self(QNAN_PREFIX | (7u64 << TAG_SHIFT) | 1)
        } else {
            Self(bits)
        }
    }

    pub fn bool(v: bool) -> Self {
        Self(make_tag(TAG_BOOL) | (v as u64))
    }

    pub fn null() -> Self {
        Self(make_tag(TAG_NULL))
    }

    pub fn undefined() -> Self {
        Self(make_tag(TAG_UNDEFINED))
    }

    pub fn object(ptr: *const u8) -> Self {
        let addr = ptr as u64;
        debug_assert!(addr <= PTR_MASK, "object pointer must fit in 48 bits");
        Self(make_tag(TAG_OBJECT) | addr)
    }

    pub fn string(index: u32, hash: u16) -> Self {
        let payload = ((hash as u64) << STRING_HASH_SHIFT) | (index as u64);
        debug_assert!(
            payload & !(STRING_HASH_MASK | STRING_INDEX_MASK) == 0,
            "string payload overflow"
        );
        Self(make_tag(TAG_STRING) | payload)
    }

    pub fn is_double(&self) -> bool {
        !is_nan_boxed(self.0)
    }

    pub fn is_int(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_INT
    }

    pub fn is_bool(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_BOOL
    }

    pub fn is_null(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_NULL
    }

    pub fn is_undefined(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_UNDEFINED
    }

    pub fn is_object(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_OBJECT
    }

    pub fn is_string(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_STRING
    }

    pub fn as_double(&self) -> f64 {
        debug_assert!(self.is_double(), "JsValue is not a double");
        f64::from_bits(self.0)
    }

    pub fn as_int(&self) -> i32 {
        debug_assert!(self.is_int(), "JsValue is not an int");
        (self.0 & INT_MASK) as i32
    }

    pub fn as_bool(&self) -> bool {
        debug_assert!(self.is_bool(), "JsValue is not a bool");
        (self.0 & 1) != 0
    }

    pub fn as_ptr(&self) -> *const u8 {
        if !self.is_object() {
            return std::ptr::null();
        }
        (self.0 & PTR_MASK) as *const u8
    }

    pub fn as_object_ptr(&self) -> *mut u8 {
        if !self.is_object() {
            return std::ptr::null_mut();
        }
        (self.0 & PTR_MASK) as *mut u8
    }

    pub fn as_js_object_ptr(&self) -> *mut JsObject {
        if !self.is_object() {
            return std::ptr::null_mut();
        }
        (self.0 & PTR_MASK) as *mut JsObject
    }

    pub fn from_js_object(ptr: *mut JsObject) -> Self {
        let addr = ptr as u64;
        debug_assert!(addr <= PTR_MASK, "object pointer must fit in 48 bits");
        Self(make_tag(TAG_OBJECT) | addr)
    }

    pub fn as_string_index(&self) -> u32 {
        debug_assert!(self.is_string(), "JsValue is not a string");
        (self.0 & STRING_INDEX_MASK) as u32
    }

    pub fn as_string_hash(&self) -> u16 {
        debug_assert!(self.is_string(), "JsValue is not a string");
        ((self.0 & STRING_HASH_MASK) >> STRING_HASH_SHIFT) as u16
    }
}

fn is_nan_bits(bits: u64) -> bool {
    (bits & EXP_MASK) == EXP_MASK && (bits & MANTISSA_MASK) != 0
}

fn is_nan_boxed(bits: u64) -> bool {
    (0xFFF8..=0xFFFD).contains(&((bits >> 48) as u16))
}

impl fmt::Display for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_int() {
            write!(f, "{}", self.as_int())
        } else if self.is_double() {
            let d = self.as_double();
            if d.is_nan() {
                write!(f, "NaN")
            } else if d.is_infinite() {
                if d.is_sign_positive() {
                    write!(f, "Infinity")
                } else {
                    write!(f, "-Infinity")
                }
            } else {
                write!(f, "{d}")
            }
        } else if self.is_bool() {
            write!(f, "{}", self.as_bool())
        } else if self.is_null() {
            write!(f, "null")
        } else if self.is_undefined() {
            write!(f, "undefined")
        } else if self.is_object() {
            write!(f, "{{object}}")
        } else if self.is_string() {
            write!(f, "{{string}}")
        } else {
            write!(f, "{{unknown}}")
        }
    }
}

impl fmt::Debug for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_int() {
            write!(f, "JsValue(Int({}))", self.as_int())
        } else if self.is_double() {
            let d = self.as_double();
            if d.is_nan() {
                write!(f, "JsValue(Double(NaN))")
            } else {
                write!(f, "JsValue(Double({d}))")
            }
        } else if self.is_bool() {
            write!(f, "JsValue(Bool({}))", self.as_bool())
        } else if self.is_null() {
            write!(f, "JsValue(Null)")
        } else if self.is_undefined() {
            write!(f, "JsValue(Undefined)")
        } else if self.is_object() {
            write!(f, "JsValue(Object({:p}))", self.as_ptr())
        } else if self.is_string() {
            write!(
                f,
                "JsValue(String(idx={}, hash={:#06x}))",
                self.as_string_index(),
                self.as_string_hash()
            )
        } else {
            write!(f, "JsValue(Unknown)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::JsValue;
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
                ];
                let count = matched.iter().filter(|&&x| x).count();
                assert_eq!(count, 1, "bits={bits:#018x} matched {count} types");
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
}
