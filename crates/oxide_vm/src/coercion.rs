use crate::vm::Vm;
use oxide_kernel::string_forge::StringForge;
use oxide_types::object::JsObject;
use oxide_types::shape::EMPTY_SHAPE_ID;
use oxide_types::value::JsValue;

pub fn to_primitive(val: JsValue) -> JsValue {
    val
}

pub fn to_number(val: JsValue, string_forge: &StringForge) -> f64 {
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
        let s = string_forge.lookup(val.as_string_index()).unwrap_or_default();
        return s.parse::<f64>().unwrap_or(f64::NAN);
    }
    if val.is_object() {
        return f64::NAN;
    }
    f64::NAN
}

pub fn to_uint32(val: JsValue, string_forge: &StringForge) -> u32 {
    let n = to_number(val, string_forge);
    if n == 0.0 || !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

pub fn to_int32(val: JsValue, string_forge: &StringForge) -> i32 {
    let int = to_uint32(val, string_forge);
    if int > i32::MAX as u32 {
        (int as i64 - 4_294_967_296i64) as i32
    } else {
        int as i32
    }
}

pub fn to_string(string_forge: &StringForge, val: JsValue) -> String {
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
        return string_forge.lookup(val.as_string_index()).unwrap_or_default();
    }
    if val.is_object() {
        return "[object]".to_string();
    }
    String::new()
}

pub fn to_boolean(val: JsValue, string_forge: &StringForge) -> bool {
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
        let s = string_forge.lookup(val.as_string_index()).unwrap_or_default();
        return !s.is_empty();
    }
    if val.is_object() {
        return true;
    }
    if val.is_symbol() {
        return true;
    }
    false
}

pub fn abstract_eq(lhs: JsValue, rhs: JsValue, string_forge: &StringForge) -> bool {
    if lhs.is_null() && rhs.is_undefined() {
        return true;
    }
    if lhs.is_undefined() && rhs.is_null() {
        return true;
    }
    if lhs.is_string() && rhs.is_string() {
        return lhs == rhs;
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
        return strict_double_eq(to_number(lhs, string_forge), to_number(rhs, string_forge));
    }
    if (lhs.is_int() || lhs.is_double()) && rhs.is_bool() {
        return to_number(lhs, string_forge) == to_number(rhs, string_forge);
    }
    if lhs.is_bool() && (rhs.is_int() || rhs.is_double()) {
        return to_number(lhs, string_forge) == to_number(rhs, string_forge);
    }
    if lhs.is_bool() || rhs.is_bool() {
        return to_number(lhs, string_forge) == to_number(rhs, string_forge);
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
        return lhs == rhs;
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

pub fn relational_compare(string_forge: &StringForge, lhs: JsValue, rhs: JsValue) -> Option<bool> {
    if lhs.is_string() && rhs.is_string() {
        let ls = string_forge.lookup(lhs.as_string_index()).unwrap_or_default();
        let rs = string_forge.lookup(rhs.as_string_index()).unwrap_or_default();
        return Some(ls < rs);
    }
    let l = to_number(lhs, string_forge);
    let r = to_number(rhs, string_forge);
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
        return lhs == rhs;
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

pub fn to_object(val: JsValue, vm: &mut Vm) -> Result<JsValue, &'static str> {
    if val.is_object() {
        return Ok(val);
    }
    if val.is_null() || val.is_undefined() {
        return Err("TypeError: Cannot convert null or undefined to object");
    }
    let proto_ptr = if val.is_string() {
        vm.kernel().builtin_world().string_proto.as_ptr() as *mut JsObject
    } else if val.is_int() || val.is_double() {
        vm.kernel().builtin_world().number_proto.as_ptr() as *mut JsObject
    } else if val.is_bool() {
        vm.kernel().builtin_world().boolean_proto.as_ptr() as *mut JsObject
    } else {
        &*vm.object_prototype as *const JsObject as *mut JsObject
    };
    let proto_val = JsValue::from_js_object(proto_ptr);
    let obj = vm.epoch.alloc(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    let obj_val = JsValue::from_js_object(obj);
    let obj_ref = unsafe { &mut *obj };
    obj_ref.ensure_hash_props().push(val);
    obj_ref.set_prop_count(1);
    Ok(obj_val)
}
