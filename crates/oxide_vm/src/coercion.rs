use crate::vm::Vm;
use oxide_kernel::string_forge::StringForge;
use oxide_types::object::JsObject;
use oxide_types::shape::EMPTY_SHAPE_ID;
use oxide_types::value::JsValue;

pub fn to_primitive(val: JsValue) -> JsValue {
    if val.is_object() {
        panic_to_object();
    }
    val
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
    if val.is_object() {
        panic_to_object();
    }
    f64::NAN
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
        return string_forge
            .lookup(val.as_string_index())
            .unwrap_or_default();
    }
    if val.is_object() {
        panic_to_object();
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
        return true;
    }
    if val.is_object() {
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
        panic_to_object();
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
        let ls = string_forge
            .lookup(lhs.as_string_index())
            .unwrap_or_default();
        let rs = string_forge
            .lookup(rhs.as_string_index())
            .unwrap_or_default();
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

fn panic_to_object() -> ! {
    panic!("ToObject failed: null or undefined cannot be converted to object")
}

pub fn to_object(val: JsValue, vm: &mut Vm) -> Result<JsValue, &'static str> {
    if val.is_object() {
        return Ok(val);
    }
    if val.is_null() || val.is_undefined() {
        return Err("TypeError: Cannot convert null or undefined to object");
    }
    let proto_ptr = &*vm.object_prototype as *const JsObject as *mut JsObject;
    let proto_val = JsValue::from_js_object(proto_ptr);
    let obj = vm
        .epoch
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
    let obj_val = JsValue::from_js_object(obj);
    let obj_ref = unsafe { &mut *obj };
    obj_ref.set_inline_prop(0, val);
    obj_ref.set_prop_count(1);
    Ok(obj_val)
}
