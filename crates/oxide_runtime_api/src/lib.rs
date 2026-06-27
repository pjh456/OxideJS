//! `oxide_runtime_api` — the abstract interface between builtins and the VM.
//!
//! Builtins are written generically against the [`VmHost`] trait
//! (`fn xxx<H: VmHost>(host: &mut H, args: &[u8]) -> NativeResult`); `Vm`
//! implements `VmHost`. This breaks what would otherwise be a circular
//! dependency between the builtins crate and `oxide_vm`:
//!
//! `oxide_types ← oxide_kernel ← oxide_runtime_api ← oxide_builtins ← oxide_vm`
//!
//! The trait is GENERIC-friendly, not object-safe: monomorphizing `H = Vm`
//! inlines every `host.*()` call, so there is zero runtime overhead versus
//! builtins living inside `oxide_vm`.

use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::shape::EMPTY_SHAPE_ID;
use oxide_types::value::JsValue;

/// Return value of every builtin native function.
pub enum NativeResult {
    Ok(JsValue),
    Err(JsValue),
    TailCall { callee: JsValue, this: JsValue, args: Vec<JsValue> },
}

impl NativeResult {
    pub fn ok(val: JsValue) -> Self {
        Self::Ok(val)
    }

    pub fn err(val: JsValue) -> Self {
        Self::Err(val)
    }

    pub fn unwrap(self) -> JsValue {
        match self {
            Self::Ok(val) => val,
            Self::Err(_) => panic!("called `NativeResult::unwrap()` on an `Err` value"),
            Self::TailCall { .. } => panic!("called `NativeResult::unwrap()` on a `TailCall` value"),
        }
    }

    pub fn map_err<E, F>(self, op: F) -> Result<JsValue, E>
    where
        F: FnOnce(JsValue) -> E,
    {
        match self {
            Self::Ok(val) => Ok(val),
            Self::Err(err) => Err(op(err)),
            Self::TailCall { .. } => panic!("TailCall cannot be converted to Result"),
        }
    }
}

/// The set of `Vm` capabilities that builtins depend on.
///
/// Signatures are byte-for-byte copies of the corresponding inherent methods on
/// `Vm`; `impl VmHost for Vm` delegates to them. The trait is intentionally
/// flat and not object-safe — builtins always take `&mut impl VmHost`.
pub trait VmHost {
    // Register access
    fn reg(&self, idx: u8) -> JsValue;
    fn set_reg(&mut self, idx: u8, val: JsValue);

    // Object allocation / string creation
    fn alloc_object(&mut self, obj: JsObject) -> *mut JsObject;
    fn new_string(&mut self, s: &str) -> JsValue;

    // Kernel accessors
    fn kernel_core(&self) -> &Arc<KernelCore>;
    fn session(&self) -> &KernelSession;
    fn epoch(&self) -> &Epoch;

    // Property resolution
    fn property_key_si(&mut self, val: JsValue) -> u32;
    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue>;
    fn get_own_property_slot(&self, obj: &JsObject, prop_name_si: u32) -> Option<u32>;

    // Property access
    fn ordinary_get(&mut self, obj: &JsObject, prop_name_si: u32, receiver: JsValue) -> Result<JsValue, String>;
    fn ordinary_set(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, receiver: JsValue,
    ) -> Result<(), String>;

    // Property definition
    fn define_data_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue, attributes: PropAttributes,
    ) -> Result<(), String>;
    fn define_accessor_property(
        &mut self, obj: &mut JsObject, prop_name_si: u32, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) -> Result<(), String>;
    fn set_or_create_prop_value(&mut self, obj: &mut JsObject, prop_name_si: u32, val: JsValue);

    // Lookup / coercion
    fn lookup_str(&self, val: JsValue) -> Option<String>;
    fn coerce_primitive_bounded(&mut self, value: JsValue, prefer_string: bool) -> Result<JsValue, String>;
    fn coerce_number_bounded(&mut self, value: JsValue) -> Result<f64, String>;

    // Call infrastructure
    fn call_function_sync(&mut self, callee: JsValue, receiver: JsValue, args: &[JsValue]) -> Result<JsValue, String>;

    // Error handling
    fn checked_object_ptr(&mut self, val: JsValue, error_msg: &str) -> Result<Option<*mut JsObject>, String>;
    fn raise_type_error(&mut self, msg: &str) -> Result<(), String>;
    fn error_message_text(&self, kind: &str, msg: &str) -> String;
    fn call_stack_function_names(&self) -> Vec<String>;
    fn promote_if_needed_for_write_ptr(&mut self, target_ptr: *mut JsObject, value: JsValue) -> JsValue;
    fn step_rng(&mut self);
    fn math_rng_value(&self) -> f64;
    fn sub_module_function_name(&self, sub_idx: u16) -> String;
    fn symbol_intern(&mut self, desc: String) -> u32;
    fn symbol_description(&self, idx: u32) -> Option<&str>;
    fn symbol_lookup_global(&self, key: &str) -> Option<u32>;
    fn symbol_register_global(&mut self, key: String, idx: u32);
    fn symbol_key_for_id(&self, idx: u32) -> Option<String>;
}

pub fn format_error_message(kind: &str, msg: &str) -> String {
    if msg.is_empty() || msg == kind || msg.starts_with(&format!("{kind}:")) {
        msg.to_string()
    } else {
        format!("{kind}: {msg}")
    }
}

/// Borrow a string value's content.
///
/// # Safety
/// `val` must be a string `JsValue` whose `JsString` pointer is alive.
#[inline]
pub unsafe fn string_data(val: JsValue) -> &'static str {
    (*val.as_string_ptr()).as_str()
}

#[inline]
pub fn string_value_eq(a: JsValue, b: JsValue) -> bool {
    if a.as_string_ptr() == b.as_string_ptr() {
        return true;
    }
    let sa = unsafe { &*a.as_string_ptr() };
    let sb = unsafe { &*b.as_string_ptr() };
    sa.hash == sb.hash && sa.data == sb.data
}

pub fn to_number(val: JsValue) -> f64 {
    if val.is_int() {
        return val.as_int() as f64;
    }
    if val.is_double() {
        return val.as_double();
    }
    if val.is_bool() {
        return if val.as_bool() { 1.0 } else { 0.0 };
    }
    if val.is_null() {
        return 0.0;
    }
    if val.is_undefined() {
        return f64::NAN;
    }
    if val.is_string() {
        let s = unsafe { string_data(val) };
        return s.parse::<f64>().unwrap_or(f64::NAN);
    }
    if val.is_object() {
        return f64::NAN;
    }
    f64::NAN
}

pub fn to_uint32(val: JsValue) -> u32 {
    let n = to_number(val);
    if n == 0.0 || !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

pub fn to_int32(val: JsValue) -> i32 {
    let int = to_uint32(val);
    if int > i32::MAX as u32 {
        (int as i64 - 4_294_967_296i64) as i32
    } else {
        int as i32
    }
}

pub fn to_string(val: JsValue) -> String {
    if val.is_int() {
        return val.as_int().to_string();
    }
    if val.is_double() {
        let d = val.as_double();
        if d.is_nan() {
            return "NaN".to_string();
        }
        if d.is_infinite() {
            return if d.is_sign_positive() {
                "Infinity".to_string()
            } else {
                "-Infinity".to_string()
            };
        }
        if d.is_finite() && d.fract() == 0.0 {
            return (d as i64).to_string();
        }
        let mut buf = ryu::Buffer::new();
        return buf.format(d).to_string();
    }
    if val.is_bool() {
        return val.as_bool().to_string();
    }
    if val.is_null() {
        return "null".to_string();
    }
    if val.is_undefined() {
        return "undefined".to_string();
    }
    if val.is_string() {
        return unsafe { string_data(val) }.to_string();
    }
    if val.is_object() {
        return "[object]".to_string();
    }
    String::new()
}

pub fn to_boolean(val: JsValue) -> bool {
    if val.is_undefined() || val.is_null() {
        return false;
    }
    if val.is_bool() {
        return val.as_bool();
    }
    if val.is_int() {
        return val.as_int() != 0;
    }
    if val.is_double() {
        let d = val.as_double();
        return !(d == 0.0 || d == -0.0 || d.is_nan());
    }
    if val.is_string() {
        return !unsafe { (*val.as_string_ptr()).is_empty() };
    }
    if val.is_object() {
        return true;
    }
    if val.is_symbol() {
        return true;
    }
    false
}

/// Whether two values share the same ECMAScript language type. Number treats
/// int- and double-tagged values as one type (both are Number).
fn same_type(a: JsValue, b: JsValue) -> bool {
    if a.is_string() && b.is_string() {
        return true;
    }
    if (a.is_int() || a.is_double()) && (b.is_int() || b.is_double()) {
        return true;
    }
    if a.is_bool() && b.is_bool() {
        return true;
    }
    if a.is_null() && b.is_null() {
        return true;
    }
    if a.is_undefined() && b.is_undefined() {
        return true;
    }
    if a.is_object() && b.is_object() {
        return true;
    }
    if a.is_symbol() && b.is_symbol() {
        return true;
    }
    false
}

/// IsLooselyEqual(x, y) — ECMA-262 §7.2.15 (`==`).
///
/// BigInt steps are omitted (the engine prunes BigInt). Object operands are
/// coerced via ToPrimitive, which may invoke user `valueOf` / `toString` /
/// `@@toPrimitive`; hence the `VmHost` parameter and the `Result` (a thrown
/// TypeError from those callbacks propagates as `Err`). Note: `Object == Symbol`
/// does NOT trigger ToPrimitive (spec steps 11/12 cover only Number/String) and
/// falls through to `false`.
pub fn abstract_eq<H: VmHost>(lhs: JsValue, rhs: JsValue, host: &mut H) -> Result<bool, String> {
    // Step 1: same type -> strict equality.
    if same_type(lhs, rhs) {
        return Ok(strict_equality(lhs, rhs));
    }
    // Steps 2-3: null <-> undefined.
    if (lhs.is_null() && rhs.is_undefined()) || (lhs.is_undefined() && rhs.is_null()) {
        return Ok(true);
    }
    // Steps 5-6: Number <-> String.
    if (lhs.is_int() || lhs.is_double()) && rhs.is_string() {
        return Ok(strict_double_eq(to_number(lhs), to_number(rhs)));
    }
    if lhs.is_string() && (rhs.is_int() || rhs.is_double()) {
        return Ok(strict_double_eq(to_number(lhs), to_number(rhs)));
    }
    // Step 9: x is Boolean -> compare ToNumber(x).
    if lhs.is_bool() {
        return abstract_eq(JsValue::float(to_number(lhs)), rhs, host);
    }
    // Step 10: y is Boolean -> compare ToNumber(y).
    if rhs.is_bool() {
        return abstract_eq(lhs, JsValue::float(to_number(rhs)), host);
    }
    // Step 11: x is Number/String, y is Object -> ToPrimitive(y).
    if (lhs.is_int() || lhs.is_double() || lhs.is_string()) && rhs.is_object() {
        let prim = to_primitive(rhs, ToPrimitiveHint::Default, host)?;
        return abstract_eq(lhs, prim, host);
    }
    // Step 12: x is Object, y is Number/String -> ToPrimitive(x).
    if lhs.is_object() && (rhs.is_int() || rhs.is_double() || rhs.is_string()) {
        let prim = to_primitive(lhs, ToPrimitiveHint::Default, host)?;
        return abstract_eq(prim, rhs, host);
    }
    // Step 14: otherwise not equal.
    Ok(false)
}

pub fn strict_eq(lhs: JsValue, rhs: JsValue) -> bool {
    if lhs.is_int() && rhs.is_int() {
        return lhs.as_int() == rhs.as_int();
    }
    if lhs.is_double() && rhs.is_double() {
        return strict_double_eq(lhs.as_double(), rhs.as_double());
    }
    if lhs.is_bool() && rhs.is_bool() {
        return lhs.as_bool() == rhs.as_bool();
    }
    if lhs.is_null() && rhs.is_null() {
        return true;
    }
    if lhs.is_undefined() && rhs.is_undefined() {
        return true;
    }
    if lhs.is_string() && rhs.is_string() {
        return string_value_eq(lhs, rhs);
    }
    if lhs.is_object() && rhs.is_object() {
        return lhs.as_ptr() == rhs.as_ptr();
    }
    if lhs.is_symbol() && rhs.is_symbol() {
        return lhs.as_symbol_index() == rhs.as_symbol_index();
    }
    false
}

fn strict_double_eq(a: f64, b: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return false;
    }
    a == b
}

pub fn relational_compare(lhs: JsValue, rhs: JsValue) -> Option<bool> {
    if lhs.is_string() && rhs.is_string() {
        let ls = unsafe { string_data(lhs) };
        let rs = unsafe { string_data(rhs) };
        return Some(ls < rs);
    }
    let l = to_number(lhs);
    let r = to_number(rhs);
    if l.is_nan() || r.is_nan() {
        return None;
    }
    l.partial_cmp(&r).map(|o| o.is_lt())
}

pub fn string_concat(lhs: &str, rhs: &str) -> String {
    let mut s = String::with_capacity(lhs.len() + rhs.len());
    s.push_str(lhs);
    s.push_str(rhs);
    s
}

fn to_f64(val: JsValue) -> f64 {
    if val.is_int() {
        val.as_int() as f64
    } else if val.is_double() {
        val.as_double()
    } else {
        f64::NAN
    }
}

pub fn same_value(lhs: JsValue, rhs: JsValue) -> bool {
    if lhs.is_double() && rhs.is_double() {
        let a = lhs.as_double();
        let b = rhs.as_double();
        if a.is_nan() && b.is_nan() {
            return true;
        }
        if a == 0.0 && b == 0.0 {
            let a_neg = a.is_sign_negative();
            let b_neg = b.is_sign_negative();
            return a_neg == b_neg;
        }
        return a == b;
    }
    if lhs.is_int() && rhs.is_int() {
        return lhs.as_int() == rhs.as_int();
    }
    if (lhs.is_int() || lhs.is_double()) && (rhs.is_int() || rhs.is_double()) {
        let a = to_f64(lhs);
        let b = to_f64(rhs);
        if a.is_nan() && b.is_nan() {
            return true;
        }
        if a == 0.0 && b == 0.0 {
            return a.is_sign_negative() == b.is_sign_negative();
        }
        return a == b;
    }
    if lhs.is_bool() && rhs.is_bool() {
        return lhs.as_bool() == rhs.as_bool();
    }
    if lhs.is_null() && rhs.is_null() {
        return true;
    }
    if lhs.is_undefined() && rhs.is_undefined() {
        return true;
    }
    if lhs.is_string() && rhs.is_string() {
        return string_value_eq(lhs, rhs);
    }
    if lhs.is_object() && rhs.is_object() {
        return lhs.as_ptr() == rhs.as_ptr();
    }
    if lhs.is_symbol() && rhs.is_symbol() {
        return lhs.as_symbol_index() == rhs.as_symbol_index();
    }
    false
}

/// ToIntegerOrInfinity(argument) — ECMA-262 §7.1.4.
pub fn to_integer_or_infinity(val: JsValue) -> f64 {
    let n = to_number(val);
    if n.is_nan() || n == 0.0 {
        0.0
    } else if n.is_infinite() {
        n
    } else {
        n.trunc()
    }
}

/// ToLength(argument) — ECMA-262 §7.1.20.
pub fn to_length(val: JsValue) -> u64 {
    let n = to_number(val);
    let len = if n.is_nan() || n <= 0.0 { 0.0 } else { n.min(9_007_199_254_740_991.0) };
    len.trunc() as u64
}

/// SameValueZero(x, y) — ECMA-262 §7.2.11.
pub fn same_value_zero(lhs: JsValue, rhs: JsValue) -> bool {
    if (lhs.is_double() || lhs.is_int()) && (rhs.is_double() || rhs.is_int()) {
        let a = to_f64(lhs);
        let b = to_f64(rhs);
        if a.is_nan() && b.is_nan() {
            return true;
        }
        return a == b;
    }
    same_value(lhs, rhs)
}

pub fn strict_equality(lhs: JsValue, rhs: JsValue) -> bool {
    if lhs.is_double() && rhs.is_double() {
        return strict_double_eq(lhs.as_double(), rhs.as_double());
    }
    same_value(lhs, rhs)
}

pub fn to_object<H: VmHost>(val: JsValue, host: &mut H) -> Result<JsValue, String> {
    if val.is_object() {
        return Ok(val);
    }
    if val.is_null() || val.is_undefined() {
        return Err(host.error_message_text("TypeError", "Cannot convert null or undefined to object"));
    }
    let world = host.session().builtin_world();
    let (proto_ptr, type_tag) = if val.is_string() {
        (P::as_ptr(&world.string_proto) as *mut JsObject, JsObject::OBJ_TYPE_STRING_OBJ)
    } else if val.is_int() || val.is_double() {
        (P::as_ptr(&world.number_proto) as *mut JsObject, JsObject::OBJ_TYPE_NUMBER_OBJ)
    } else if val.is_bool() {
        (P::as_ptr(&world.boolean_proto) as *mut JsObject, JsObject::OBJ_TYPE_BOOLEAN_OBJ)
    } else {
        (P::as_ptr(&world.object_proto) as *mut JsObject, JsObject::OBJ_TYPE_PLAIN)
    };
    let proto_val = JsValue::from_js_object(proto_ptr);
    let obj = host.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    let obj_val = JsValue::from_js_object(obj);
    let obj_ref = unsafe { &mut *obj };
    obj_ref.type_tag = type_tag;
    obj_ref.ensure_hash_props().push(val);
    obj_ref.set_prop_count(1);
    Ok(obj_val)
}

/// Hint for the abstract ToPrimitive operation (ECMA-262 §7.1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToPrimitiveHint {
    Default,
    String,
    Number,
}

impl ToPrimitiveHint {
    /// OrdinaryToPrimitive method-name ordering hint: `Default`/`Number` -> "number", `String` -> "string".
    pub fn as_str(self) -> &'static str {
        match self {
            ToPrimitiveHint::String => "string",
            ToPrimitiveHint::Default | ToPrimitiveHint::Number => "number",
        }
    }
}

/// ToPrimitive(input, hint) per ECMA-262 §7.1.1.
///
/// Primitives pass through unchanged. Objects are coerced via the VM's
/// `coerce_primitive_bounded`, which consults `obj[Symbol.toPrimitive]` first and
/// otherwise runs OrdinaryToPrimitive (valueOf/toString in hint order). Keeping the
/// object path in one place avoids duplicating OrdinaryToPrimitive here.
pub fn to_primitive<H: VmHost>(val: JsValue, hint: ToPrimitiveHint, host: &mut H) -> Result<JsValue, String> {
    if !val.is_object() {
        return Ok(val);
    }
    host.coerce_primitive_bounded(val, hint == ToPrimitiveHint::String)
}

/// ToNumber(input) with full object coercion: objects go through ToPrimitive
/// (number hint); Symbol values throw a TypeError per spec.
pub fn to_number_full<H: VmHost>(val: JsValue, host: &mut H) -> Result<f64, String> {
    let primitive = to_primitive(val, ToPrimitiveHint::Number, host)?;
    if primitive.is_symbol() {
        return Err(host.error_message_text("TypeError", "Cannot convert a Symbol value to a number"));
    }
    Ok(to_number(primitive))
}

/// ToString(input) with full object coercion: objects go through ToPrimitive (string hint).
pub fn to_string_full<H: VmHost>(val: JsValue, host: &mut H) -> Result<String, String> {
    let primitive = to_primitive(val, ToPrimitiveHint::String, host)?;
    Ok(to_string(primitive))
}
