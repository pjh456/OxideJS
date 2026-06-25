use std::collections::HashSet;
use std::fmt::Write;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::builtins::object::walk_own_keys;
use crate::vm::Vm;
use oxide_runtime_api::NativeResult;

pub fn json_parse(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::builtins::error::create_syntax_error(vm, "JSON.parse requires 1 argument"));
    }
    let val = vm.reg(args[1]);
    if !val.is_string() {
        return NativeResult::Err(crate::builtins::error::create_syntax_error(
            vm,
            "JSON.parse: argument is not a string",
        ));
    }
    let text = {
        // SAFETY: val is a string value.
        unsafe { (*val.as_string_ptr()).data.clone() }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(crate::builtins::error::create_syntax_error(vm, &format!("{}", e))),
    };

    NativeResult::Ok(value_to_jsvalue(vm, &parsed))
}

fn value_to_jsvalue(vm: &mut Vm, val: &serde_json::Value) -> JsValue {
    match val {
        serde_json::Value::Null => JsValue::null(),
        serde_json::Value::Bool(b) => JsValue::bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                JsValue::float(f)
            } else {
                JsValue::float(0.0)
            }
        }
        serde_json::Value::String(s) => vm.new_string(s),
        serde_json::Value::Array(arr) => {
            let array_proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
            let n = arr.len();
            let array_obj = vm.alloc_object(JsObject::new_array(
                EMPTY_SHAPE_ID,
                JsValue::from_js_object(array_proto),
                n,
                vm.epoch().bump(),
            ));
            for (i, v) in arr.iter().enumerate() {
                let jsv = value_to_jsvalue(vm, v);
                unsafe {
                    (*array_obj).set_prop_at(i, jsv);
                }
            }
            JsValue::from_js_object(array_obj)
        }
        serde_json::Value::Object(map) => {
            let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
            let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
            for (key, val) in map {
                let si = vm.kernel_core().perm_interner().intern(key).0;
                let jsv = value_to_jsvalue(vm, val);
                let new_shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), si);
                obj.set_shape_id(new_shape);
                obj.ensure_hash_props().push(jsv);
            }
            let obj_ptr = vm.alloc_object(obj);
            JsValue::from_js_object(obj_ptr)
        }
    }
}

pub fn json_stringify(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let value = vm.reg(args[1]);
    if value.is_undefined() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let mut visited = HashSet::new();
    let mut output = String::new();
    if jsvalue_to_json(vm, value, &mut visited, &mut output).is_err() {
        return NativeResult::Err(crate::builtins::error::create_type_error(vm, "cyclic object value"));
    };
    NativeResult::Ok(vm.new_string(&output))
}

fn jsvalue_to_json(vm: &Vm, val: JsValue, visited: &mut HashSet<*const u8>, out: &mut String) -> Result<(), ()> {
    if val.is_null() {
        out.push_str("null");
    } else if val.is_undefined() {
    } else if val.is_bool() {
        out.push_str(if val.as_bool() { "true" } else { "false" });
    } else if val.is_int() {
        write!(out, "{}", val.as_int()).unwrap();
    } else if val.is_double() {
        let n = val.as_double();
        if !n.is_finite() {
            out.push_str("null");
        } else if n.fract() == 0.0 && n.abs() < 1e21 {
            write!(out, "{}", n as i64).unwrap();
        } else {
            let mut buf = ryu::Buffer::new();
            out.push_str(buf.format(n));
        }
    } else if val.is_string() {
        // SAFETY: val is a string value.
        let s = unsafe { (*val.as_string_ptr()).data.clone() };
        stringify_string(&s, out);
    } else if val.is_object() {
        let obj_ptr = val.as_js_object_ptr();
        if obj_ptr.is_null() {
            out.push_str("null");
            return Ok(());
        }

        if visited.contains(&(obj_ptr as *const u8)) {
            return Err(());
        }
        visited.insert(obj_ptr as *const u8);

        let obj = unsafe { &*obj_ptr };

        if obj.is_array() {
            stringify_array(vm, obj, visited, out)?;
        } else {
            stringify_object(vm, obj, visited, out)?;
        }

        visited.remove(&(obj_ptr as *const u8));
    }
    Ok(())
}

fn stringify_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let mut buf = [0u16; 2];
                let encoded = c.encode_utf16(&mut buf);
                for unit in encoded {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            _ => out.push(c),
        }
    }
    out.push('"');
}

fn stringify_object(vm: &Vm, obj: &JsObject, visited: &mut HashSet<*const u8>, out: &mut String) -> Result<(), ()> {
    out.push('{');
    let keys = walk_own_keys(vm, obj);
    let mut first = true;
    for (si, pos) in keys {
        let val = obj.get_prop_at(pos);
        let is_function = val.is_object() && {
            let ptr = val.as_js_object_ptr();
            if ptr.is_null() {
                false
            } else {
                unsafe { (*ptr).is_function() }
            }
        };
        if val.is_undefined() || is_function {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        let name = vm.kernel_core().perm_interner().lookup(si).unwrap_or_default();
        stringify_string(name, out);
        out.push(':');
        jsvalue_to_json(vm, val, visited, out)?;
    }
    out.push('}');
    Ok(())
}

fn stringify_array(vm: &Vm, obj: &JsObject, visited: &mut HashSet<*const u8>, out: &mut String) -> Result<(), ()> {
    out.push('[');
    let len = obj.prop_vec_len();
    for i in 0..len {
        if i > 0 {
            out.push(',');
        }
        let val = obj.get_prop_at(i);
        let is_function = val.is_object() && {
            let ptr = val.as_js_object_ptr();
            if ptr.is_null() {
                false
            } else {
                unsafe { (*ptr).is_function() }
            }
        };
        if is_function || val.is_undefined() {
            out.push_str("null");
        } else {
            jsvalue_to_json(vm, val, visited, out)?;
        }
    }
    out.push(']');
    Ok(())
}
