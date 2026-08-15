//! ECMAScript 值表示：NaN-boxing 的 64 位 `JsValue`。
//!
//! 普通 double 直接以 IEEE-754 位模式存放；其余类型用静默 NaN 前缀
//! `0xFFF8_0000_0000_0000` 加上 3 位 tag（bits 50-48）区分 int / bool /
//! null / undefined / object / string / symbol。对象与字符串存 48 位指针，
//! 使一个 `JsValue` 可塞进寄存器且类型判断为常数时间。

use std::fmt;

use crate::object::{JsObject, JsString};
use num_bigint::BigInt;

/// 静默 NaN 前缀——bits 63-51 = sign(1) + exponent(0x7FF) + quiet_bit(1)
const QNAN_PREFIX: u64 = 0xFFF8_0000_0000_0000;

/// NaN 指数掩码——bits 62-52
const EXP_MASK: u64 = 0x7FF0_0000_0000_0000;

/// 尾数掩码——bits 51-0
const MANTISSA_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;

/// tag 位于尾数的 bits 50-48
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const TAG_SHIFT: u64 = 48;
const TAG_INT: u64 = 0;
const TAG_BOOL: u64 = 1;
const TAG_NULL: u64 = 2;
const TAG_UNDEFINED: u64 = 3;
const TAG_OBJECT: u64 = 4;
const TAG_STRING: u64 = 5;
const TAG_SYMBOL: u64 = 6;
/// BigInt 指针（见 `bigint`/`as_bigint_ptr`）。tag 7 原为 NaN 规范化
/// 编码所在，现 NaN 规范化改用普通 quiet NaN 位模式（见 [`JsValue::float`]）。
const TAG_BIGINT: u64 = 7;

/// 48-bit pointer mask (x86-64 canonical VA)
pub const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// 32 位整数载荷掩码
const INT_MASK: u64 = 0x0000_0000_FFFF_FFFF;

fn make_tag(tag: u64) -> u64 {
    QNAN_PREFIX | (tag << TAG_SHIFT)
}

fn get_tag(bits: u64) -> u64 {
    (bits & TAG_MASK) >> TAG_SHIFT
}

/// ECMAScript 语言类型分类（`typeof` 与相等/强转的分派粒度）。
///
/// Number 内部两种表示（int/double）分列两个变体；需按"都是 Number"
/// 合并判断时用 [`JsType::is_number`]。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum JsType {
    /// 32 位整数表示（tag 0）。
    Int,
    /// 普通双精度位模式（非 NaN-box，含 ±Infinity 与规范 NaN）。
    Double,
    /// 布尔值。
    Bool,
    /// `null`。
    Null,
    /// `undefined`。
    Undefined,
    /// 对象引用（48 位指针）。
    Object,
    /// 字符串引用（48 位指针）。
    String,
    /// 符号值（payload 为符号表下标）。
    Symbol,
    /// BigInt 指针（48 位指针，tag 7）。
    BigInt,
}

impl JsType {
    /// 是否为 ECMAScript Number（int 或 double 表示）。
    #[inline]
    pub fn is_number(self) -> bool {
        matches!(self, JsType::Int | JsType::Double)
    }
}

/// 统一 ECMAScript 值：一个 NaN-boxed 的 64 位字。
///
/// `repr(transparent)` 包裹单个 `u64`。double 原样编码；非 double 类型
/// 使用 NaN 前缀 + tag + payload。`Copy`，寄存器宽度，是引擎栈与对象
/// 属性向量的基本元素。字符串/对象按指针恒等比较，内容比较交给上层。
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
            return self.as_string_ptr() == other.as_string_ptr();
        }
        if self.is_symbol() && other.is_symbol() {
            return self.as_symbol_index() == other.as_symbol_index();
        }
        if self.is_bigint() && other.is_bigint() {
            // 按值比较：不同分配但数值相等的 BigInt 视为相等。
            // SAFETY: bigint 指针由 JsValue::bigint 构造，指向存活的 i128。
            return unsafe { *self.as_bigint_ptr() == *other.as_bigint_ptr() };
        }
        false
    }
}

impl JsValue {
    #[allow(dead_code)]
    pub(crate) fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// 原始 NaN-boxed 位模式。用于在 `is_object()` 之后做受检的指针提取。
    #[inline(always)]
    pub fn to_bits(self) -> u64 {
        self.0
    }

    /// 单次提取 ECMAScript 语言类型：一次范围判断 + 掩码位移，替代多个
    /// `is_*` 串联判断。
    ///
    /// # 边界与前提
    /// - 非 NaN-box 位模式（含规范 NaN 与 ±Infinity）一律归为 `Double`
    /// - NaN-box 内按 tag 位映射（bits 50-48 三比特全被占用，无遗漏）
    ///
    /// # 注意事项
    /// int 与 double 是两种表示；按"都是 Number"判断用 [`JsType::is_number`]。
    #[inline]
    pub fn js_type(self) -> JsType {
        let prefix = (self.0 >> TAG_SHIFT) as u16;
        if !(0xFFF8..=0xFFFF).contains(&prefix) {
            return JsType::Double;
        }
        match get_tag(self.0) {
            TAG_INT => JsType::Int,
            TAG_BOOL => JsType::Bool,
            TAG_NULL => JsType::Null,
            TAG_UNDEFINED => JsType::Undefined,
            TAG_OBJECT => JsType::Object,
            TAG_STRING => JsType::String,
            TAG_SYMBOL => JsType::Symbol,
            // tag 7 = BigInt（NaN 规范化编码已改用普通 quiet NaN，见 [`JsValue::float`]）。
            TAG_BIGINT => JsType::BigInt,
            // 3 位 tag 全被占用，该分支不可达（仅满足类型系统穷尽性）。
            _ => JsType::Double,
        }
    }

    /// 是否为 ECMAScript Number（int 或 double 表示）。
    #[inline]
    pub fn is_number(self) -> bool {
        matches!(self.js_type(), JsType::Int | JsType::Double)
    }

    /// 构造 32 位整数（tag = int，payload 为 `i32` 位模式）。
    pub fn int(v: i32) -> Self {
        Self(make_tag(TAG_INT) | (v as u32 as u64))
    }

    /// 构造双精度浮点。
    ///
    /// NaN 会被规范化为引擎内唯一的安静 NaN 编码，保证
    /// `float(x).as_double()` 幂等且 `PartialEq` 对 NaN 恒为 false。
    /// 该编码落在 `is_nan_boxed` 范围之外（不占任何 tag），把 tag 7 空间
    /// 留给 [`bigint`](JsValue::bigint)。
    pub fn float(v: f64) -> Self {
        let bits = v.to_bits();
        if is_nan_bits(bits) {
            // 指数全 1、尾数非 0 的 quiet NaN，非 NaN-box 前缀（0xFFF8..=0xFFFF）。
            Self(0x7FF8_0000_0000_0000)
        } else {
            Self(bits)
        }
    }

    /// 构造布尔值。
    pub fn bool(v: bool) -> Self {
        Self(make_tag(TAG_BOOL) | (v as u64))
    }

    /// 构造 `null`。
    pub fn null() -> Self {
        Self(make_tag(TAG_NULL))
    }

    /// 构造 `undefined`。
    pub fn undefined() -> Self {
        Self(make_tag(TAG_UNDEFINED))
    }

    /// 构造对象引用（48 位指针 + object tag）。
    ///
    /// # Panics (debug)
    ///
    /// debug 构建下若指针超出 48 位地址空间会 panic。
    pub fn object(ptr: *const u8) -> Self {
        let addr = ptr as u64;
        debug_assert!(addr <= PTR_MASK, "object pointer must fit in 48 bits");
        Self(make_tag(TAG_OBJECT) | addr)
    }

    /// 构造字符串引用（指向 [`JsString`] 的 48 位指针 + string tag）。
    pub fn string(ptr: *const JsString) -> Self {
        let addr = ptr as u64;
        debug_assert!(addr <= PTR_MASK, "string pointer must fit in 48 bits");
        Self(make_tag(TAG_STRING) | addr)
    }

    /// 永久字符串（内核持有、永不回收）的语义别名。
    /// 编码与 `string` 完全相同；perm 与 session 的区分在所有权（是否属于
    /// GC 根集），而非 NaN-box 位。
    pub fn perm_string(ptr: *const JsString) -> Self {
        Self::string(ptr)
    }

    /// 是否为普通双精度浮点（非 NaN-box 编码）。
    pub fn is_double(&self) -> bool {
        !is_nan_boxed(self.0)
    }

    /// 是否为 32 位整数。
    pub fn is_int(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_INT
    }

    /// 是否为布尔值。
    pub fn is_bool(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_BOOL
    }

    /// 是否为 `null`。
    pub fn is_null(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_NULL
    }

    /// 是否为 `undefined`。
    pub fn is_undefined(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_UNDEFINED
    }

    /// 是否为 `null` 或 `undefined`（`??` 与可选链的判据）。
    pub fn is_nullish(&self) -> bool {
        self.is_null() || self.is_undefined()
    }

    /// 是否为对象引用。
    pub fn is_object(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_OBJECT
    }

    /// 是否为字符串引用。
    pub fn is_string(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_STRING
    }

    /// 解出双精度值。
    ///
    /// debug 构建下断言必须是 double；release 下非法时返回 `NaN`。
    pub fn as_double(&self) -> f64 {
        debug_assert!(self.is_double(), "JsValue is not a double");
        #[cfg(not(debug_assertions))]
        if !self.is_double() {
            return f64::NAN;
        }
        f64::from_bits(self.0)
    }

    /// 解出 32 位整数。
    ///
    /// debug 构建下断言必须是 int；release 下非法时返回 0。
    pub fn as_int(&self) -> i32 {
        debug_assert!(self.is_int(), "JsValue is not an int");
        #[cfg(not(debug_assertions))]
        if !self.is_int() {
            return 0;
        }
        (self.0 & INT_MASK) as i32
    }

    /// 解出布尔值。
    ///
    /// debug 构建下断言必须是 bool；release 下非法时返回 false。
    pub fn as_bool(&self) -> bool {
        debug_assert!(self.is_bool(), "JsValue is not a bool");
        #[cfg(not(debug_assertions))]
        if !self.is_bool() {
            return false;
        }
        (self.0 & 1) != 0
    }

    /// 解出对象底层指针；非对象时返回空指针。
    pub fn as_ptr(&self) -> *const u8 {
        if !self.is_object() {
            return std::ptr::null();
        }
        (self.0 & PTR_MASK) as *const u8
    }

    /// 解出可变对象底层指针；非对象时返回空指针。
    pub fn as_object_ptr(&self) -> *mut u8 {
        if !self.is_object() {
            return std::ptr::null_mut();
        }
        (self.0 & PTR_MASK) as *mut u8
    }

    /// 解出 `JsObject` 指针；非对象时返回空指针。
    pub fn as_js_object_ptr(&self) -> *mut JsObject {
        if !self.is_object() {
            return std::ptr::null_mut();
        }
        (self.0 & PTR_MASK) as *mut JsObject
    }

    /// 从 `JsObject` 指针构造对象值（`object` 的类型化别名）。
    pub fn from_js_object(ptr: *mut JsObject) -> Self {
        let addr = ptr as u64;
        debug_assert!(addr <= PTR_MASK, "object pointer must fit in 48 bits");
        Self(make_tag(TAG_OBJECT) | addr)
    }

    /// 解出字符串指针；调用方须先保证 [`is_string`](JsValue::is_string)。
    pub fn as_string_ptr(&self) -> *const JsString {
        debug_assert!(self.is_string(), "JsValue is not a string");
        (self.0 & PTR_MASK) as *const JsString
    }

    /// 解出可变字符串指针；调用方须先保证 [`is_string`](JsValue::is_string)。
    pub fn as_string_ptr_mut(&self) -> *mut JsString {
        debug_assert!(self.is_string(), "JsValue is not a string");
        (self.0 & PTR_MASK) as *mut JsString
    }

    /// 构造 symbol 值（payload 为符号表下标）。
    pub fn symbol(index: u32) -> Self {
        Self(make_tag(TAG_SYMBOL) | (index as u64))
    }

    /// 是否为 symbol。
    pub fn is_symbol(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_SYMBOL
    }

    /// 解出 symbol 下标；调用方须先保证 [`is_symbol`](JsValue::is_symbol)。
    pub fn as_symbol_index(&self) -> u32 {
        debug_assert!(self.is_symbol());
        (self.0 & INT_MASK) as u32
    }

    /// 构造 BigInt 值（payload 为堆分配 `i128` 的 48 位指针）。
    ///
    /// 与 JsString 同机制：i128 本体存于 VM 管理的堆 box，这里携带指针。
    /// tag 7 原为 NaN 规范化编码，NaN 已改用普通 quiet NaN 位模式，故 tag 7
    /// 空出给 BigInt。
    pub fn bigint(ptr: *const BigInt) -> Self {
        let addr = ptr as u64;
        debug_assert!(addr <= PTR_MASK, "bigint pointer must fit in 48 bits");
        Self(make_tag(TAG_BIGINT) | addr)
    }

    /// 是否为 BigInt。
    pub fn is_bigint(&self) -> bool {
        is_nan_boxed(self.0) && get_tag(self.0) == TAG_BIGINT
    }

    /// 解出 BigInt 底层指针；调用方须先保证 [`is_bigint`](JsValue::is_bigint)。
    pub fn as_bigint_ptr(&self) -> *const BigInt {
        debug_assert!(self.is_bigint());
        (self.0 & PTR_MASK) as *const BigInt
    }
}

fn is_nan_bits(bits: u64) -> bool {
    (bits & EXP_MASK) == EXP_MASK && (bits & MANTISSA_MASK) != 0
}

fn is_nan_boxed(bits: u64) -> bool {
    (0xFFF8..=0xFFFF).contains(&((bits >> 48) as u16))
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
        } else if self.is_symbol() {
            write!(f, "Symbol(idx={})", self.as_symbol_index())
        } else if self.is_bigint() {
            // SAFETY: bigint 指针指向存活的 i128。
            write!(f, "BigInt({})", unsafe { &*self.as_bigint_ptr() })
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
            write!(f, "JsValue(String({:p}))", self.as_string_ptr())
        } else if self.is_symbol() {
            write!(f, "JsValue(Symbol(idx={}))", self.as_symbol_index())
        } else if self.is_bigint() {
            // SAFETY: bigint 指针指向存活的 i128。
            write!(f, "JsValue(BigInt({}))", unsafe { &*self.as_bigint_ptr() })
        } else {
            write!(f, "JsValue(Unknown)")
        }
    }
}

#[cfg(test)]
mod tests {
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
}
