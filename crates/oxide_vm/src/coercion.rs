use crate::vm::Vm;
use oxide_types::object::JsObject;
use oxide_types::shape::EMPTY_SHAPE_ID;
use oxide_types::value::JsValue;

/// Borrow a string value's content. Caller must hold a valid string `JsValue`.
///
/// # Safety
/// `val` must be a string `JsValue` whose `JsString` pointer is alive (session or
/// permanent string) — the same contract as object-pointer derefs in the VM.
#[inline]
unsafe fn string_data(val: JsValue) -> &'static str {
    (*val.as_string_ptr()).as_str()
}

/// Content equality of two string values: pointer identity → hash+len reject →
/// byte compare. Replaces the old interner-index compare.
#[inline]
fn string_value_eq(a: JsValue, b: JsValue) -> bool {
    if a.as_string_ptr() == b.as_string_ptr() {
        return true;
    }
    // SAFETY: both are string JsValues; their JsString pointers are valid.
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
        // SAFETY: val is a string value.
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
        // SAFETY: val is a string value.
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
        // SAFETY: val is a string value.
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

pub fn abstract_eq(lhs: JsValue, rhs: JsValue) -> bool {
    if lhs.is_null() && rhs.is_undefined() {
        return true;
    }
    if lhs.is_undefined() && rhs.is_null() {
        return true;
    }
    if lhs.is_string() && rhs.is_string() {
        return string_value_eq(lhs, rhs);
    }
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
    if (lhs.is_int() || lhs.is_double()) && (rhs.is_int() || rhs.is_double()) {
        return strict_double_eq(to_number(lhs), to_number(rhs));
    }
    if (lhs.is_int() || lhs.is_double()) && rhs.is_bool() {
        return to_number(lhs) == to_number(rhs);
    }
    if lhs.is_bool() && (rhs.is_int() || rhs.is_double()) {
        return to_number(lhs) == to_number(rhs);
    }
    if lhs.is_bool() || rhs.is_bool() {
        return to_number(lhs) == to_number(rhs);
    }
    if lhs.is_null() || rhs.is_null() {
        return false;
    }
    if lhs.is_object() || rhs.is_object() {
        return lhs.is_object() && rhs.is_object() && lhs.as_ptr() == rhs.as_ptr();
    }
    false
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
        // SAFETY: both are string values.
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

pub fn strict_equality(lhs: JsValue, rhs: JsValue) -> bool {
    if lhs.is_double() && rhs.is_double() {
        return strict_double_eq(lhs.as_double(), rhs.as_double());
    }
    same_value(lhs, rhs)
}

pub fn to_object(val: JsValue, vm: &mut Vm) -> Result<JsValue, String> {
    if val.is_object() {
        return Ok(val);
    }
    if val.is_null() || val.is_undefined() {
        return Err(vm.error_message_text("TypeError", "Cannot convert null or undefined to object"));
    }
    let (proto_ptr, type_tag) = if val.is_string() {
        (
            vm.session().builtin_world().string_proto.as_ptr() as *mut JsObject,
            JsObject::OBJ_TYPE_STRING_OBJ,
        )
    } else if val.is_int() || val.is_double() {
        (
            vm.session().builtin_world().number_proto.as_ptr() as *mut JsObject,
            JsObject::OBJ_TYPE_NUMBER_OBJ,
        )
    } else if val.is_bool() {
        (
            vm.session().builtin_world().boolean_proto.as_ptr() as *mut JsObject,
            JsObject::OBJ_TYPE_BOOLEAN_OBJ,
        )
    } else {
        (&*vm.object_prototype as *const JsObject as *mut JsObject, JsObject::OBJ_TYPE_PLAIN)
    };
    let proto_val = JsValue::from_js_object(proto_ptr);
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    let obj_val = JsValue::from_js_object(obj);
    let obj_ref = unsafe { &mut *obj };
    obj_ref.type_tag = type_tag;
    obj_ref.ensure_hash_props().push(val);
    obj_ref.set_prop_count(1);
    Ok(obj_val)
}
