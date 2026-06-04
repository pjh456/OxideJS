use std::fmt;

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

/// 48-bit pointer mask (x86-64 canonical VA)
const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// 32-bit integer payload mask
const INT_MASK: u64 = 0x0000_0000_FFFF_FFFF;

fn make_tag(tag: u64) -> u64 {
    QNAN_PREFIX | (tag << TAG_SHIFT)
}

fn get_tag(bits: u64) -> u64 {
    (bits & TAG_MASK) >> TAG_SHIFT
}

#[repr(transparent)]
#[derive(Copy, Clone, PartialEq)]
pub struct JsValue(u64);

impl JsValue {
    pub fn int(v: i32) -> Self {
        Self(make_tag(TAG_INT) | (v as u32 as u64))
    }

    pub fn float(v: f64) -> Self {
        let bits = v.to_bits();
        if is_nan_bits(bits) {
            Self(QNAN_PREFIX | 1)
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

    pub fn is_double(&self) -> bool {
        !is_nan_bits(self.0)
    }

    pub fn is_int(&self) -> bool {
        get_tag(self.0) == TAG_INT
    }

    pub fn is_bool(&self) -> bool {
        get_tag(self.0) == TAG_BOOL
    }

    pub fn is_null(&self) -> bool {
        self.0 == make_tag(TAG_NULL)
    }

    pub fn is_undefined(&self) -> bool {
        self.0 == make_tag(TAG_UNDEFINED)
    }

    pub fn is_object(&self) -> bool {
        get_tag(self.0) == TAG_OBJECT
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
        debug_assert!(self.is_object(), "JsValue is not an object");
        (self.0 & PTR_MASK) as *const u8
    }
}

fn is_nan_bits(bits: u64) -> bool {
    (bits & EXP_MASK) == EXP_MASK && (bits & MANTISSA_MASK) != 0
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
        } else {
            write!(f, "JsValue(Unknown)")
        }
    }
}
