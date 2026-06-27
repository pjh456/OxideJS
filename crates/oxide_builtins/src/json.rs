use std::collections::HashSet;
use std::fmt::Write;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::object::walk_own_keys;

use oxide_runtime_api::{NativeResult, VmHost};

pub fn json_parse<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "JSON.parse requires 1 argument"));
    }
    let val = vm.reg(args[1]);
    if !val.is_string() {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "JSON.parse: argument is not a string"));
    }
    let text = {
        // SAFETY: val is a string value.
        unsafe { (*val.as_string_ptr()).data.clone() }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(crate::error::create_syntax_error(vm, &format!("{}", e))),
    };

    let mut result = value_to_jsvalue(vm, &parsed);

    // Apply reviver if provided (D-05)
    if args.len() > 2 {
        let reviver_val = vm.reg(args[2]);
        if reviver_val.is_object() {
            let rptr = reviver_val.as_js_object_ptr();
            if !rptr.is_null() && unsafe { (*rptr).is_function() } {
                let empty_si = vm.kernel_core().perm_interner().intern("").0;
                let holder = create_wrapper(vm, result);
                let holder_ptr = holder.as_js_object_ptr();
                match walk_reviver(vm, holder_ptr, empty_si, reviver_val) {
                    Ok(()) => {
                        let holder_obj = unsafe { &*holder_ptr };
                        let final_val = holder_obj.get_prop_at(0u32);
                        result = if final_val.is_undefined() { JsValue::undefined() } else { final_val };
                    }
                    Err(e) => return NativeResult::Err(e),
                }
            }
        }
    }

    NativeResult::Ok(result)
}

fn walk_reviver<H: VmHost>(
    vm: &mut H, holder_ptr: *mut JsObject, key_si: u32, reviver: JsValue,
) -> Result<(), JsValue> {
    let holder = unsafe { &*holder_ptr };
    let slot = vm.get_own_property_slot(holder, key_si);
    let pos = match slot {
        Some(p) => p,
        None => return Ok(()),
    };
    let mut val = holder.get_prop_at(pos);

    // Post-order: recurse on children first
    if val.is_object() {
        let obj_ptr = val.as_js_object_ptr();
        if !obj_ptr.is_null() {
            let obj = unsafe { &*obj_ptr };
            if obj.is_array() {
                let len = obj.prop_vec_len();
                for i in 0..len {
                    let index_str = i.to_string();
                    let child_si = vm.kernel_core().perm_interner().intern(&index_str).0;
                    walk_reviver(vm, obj_ptr, child_si, reviver)?;
                }
                // Re-read value after children modified it
                val = holder.get_prop_at(pos);
            } else {
                let keys = walk_own_keys(vm, obj);
                for (child_si, _child_pos) in keys {
                    walk_reviver(vm, obj_ptr, child_si, reviver)?;
                }
                // Re-read value after children modified it
                val = holder.get_prop_at(pos);
            }
        }
    }

    // Call reviver on current value
    let kc = vm.kernel_core().clone();
    let key_str = kc.perm_interner().lookup(key_si).unwrap_or("");
    let key_val = vm.new_string(key_str);
    let holder_val = JsValue::from_js_object(holder_ptr);
    match vm.call_function_sync(reviver, holder_val, &[key_val, val]) {
        Ok(new_val) => {
            unsafe {
                (*holder_ptr).set_prop_at(pos, new_val);
            }
            Ok(())
        }
        Err(msg) => Err(crate::error::create_type_error(vm, &msg)),
    }
}

fn value_to_jsvalue<H: VmHost>(vm: &mut H, val: &serde_json::Value) -> JsValue {
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

fn create_wrapper<H: VmHost>(vm: &mut H, value: JsValue) -> JsValue {
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let empty_si = vm.kernel_core().perm_interner().intern("").0;
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
    let new_shape = vm.kernel_core().shape_forge().make_shape(obj.shape_id(), empty_si);
    obj.set_shape_id(new_shape);
    obj.ensure_hash_props().push(value);
    let obj_ptr = vm.alloc_object(obj);
    JsValue::from_js_object(obj_ptr)
}

fn process_space(val: JsValue) -> String {
    if val.is_int() || val.is_double() {
        let n = oxide_runtime_api::to_integer_or_infinity(val);
        if n.is_nan() || n.is_infinite() || n <= 0.0 {
            return String::new();
        }
        let clamped = (n as usize).min(10);
        " ".repeat(clamped)
    } else if val.is_string() {
        let s = unsafe { (*val.as_string_ptr()).data.clone() };
        s.chars().take(10).collect()
    } else {
        let s = oxide_runtime_api::to_string(val);
        s.chars().take(10).collect()
    }
}

fn call_to_json<H: VmHost>(vm: &mut H, obj_val: JsValue, key: &str) -> Result<JsValue, JsValue> {
    if !obj_val.is_object() {
        return Ok(obj_val);
    }
    let obj_ptr = obj_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Ok(obj_val);
    }
    let tojson_si = vm.kernel_core().perm_interner().intern("toJSON").0;
    let resolved = vm.resolve_property(unsafe { &*obj_ptr }, tojson_si);
    match resolved {
        Some(fn_val) if fn_val.is_object() => {
            let fn_ptr = fn_val.as_js_object_ptr();
            if !fn_ptr.is_null() && unsafe { (*fn_ptr).is_function() } {
                let key_val = vm.new_string(key);
                match vm.call_function_sync(fn_val, obj_val, &[key_val]) {
                    Ok(v) => Ok(v),
                    Err(msg) => Err(crate::error::create_type_error(vm, &msg)),
                }
            } else {
                Ok(obj_val)
            }
        }
        _ => Ok(obj_val),
    }
}

pub fn json_stringify<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let value = vm.reg(args[1]);
    if value.is_undefined() {
        return NativeResult::Ok(JsValue::undefined());
    }

    let mut replacer_fn: Option<JsValue> = None;
    let mut replacer_whitelist: Option<HashSet<String>> = None;
    if args.len() > 2 {
        let replacer_val = vm.reg(args[2]);
        if replacer_val.is_object() {
            let rptr = replacer_val.as_js_object_ptr();
            if !rptr.is_null() {
                let robj = unsafe { &*rptr };
                if robj.is_function() {
                    replacer_fn = Some(replacer_val);
                } else if robj.is_array() {
                    let len = robj.prop_vec_len();
                    let mut whitelist = HashSet::new();
                    for i in 0..len {
                        let elem = robj.get_prop_at(i);
                        if !elem.is_undefined() {
                            whitelist.insert(oxide_runtime_api::to_string(elem));
                        }
                    }
                    replacer_whitelist = Some(whitelist);
                }
            }
        }
    }

    let space = if args.len() > 3 { process_space(vm.reg(args[3])) } else { String::new() };

    let holder = create_wrapper(vm, value);

    let value = match call_to_json(vm, value, "") {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };

    let value = if let Some(replacer) = replacer_fn {
        let key_val = vm.new_string("");
        match vm.call_function_sync(replacer, holder, &[key_val, value]) {
            Ok(v) => v,
            Err(msg) => return NativeResult::Err(crate::error::create_type_error(vm, &msg)),
        }
    } else {
        value
    };

    let mut visited = HashSet::new();
    let mut output = String::new();
    let indent_level: usize = 0;
    let key = "";
    if jsvalue_to_json(
        vm,
        value,
        &mut visited,
        &mut output,
        replacer_fn,
        replacer_whitelist.as_ref(),
        &space,
        indent_level,
        key,
    )
    .is_err()
    {
        return NativeResult::Err(crate::error::create_type_error(vm, "Converting circular structure to JSON"));
    };
    NativeResult::Ok(vm.new_string(&output))
}

#[allow(clippy::too_many_arguments)]
fn jsvalue_to_json<H: VmHost>(
    vm: &mut H, val: JsValue, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&HashSet<String>>, space: &str, indent_level: usize, key: &str,
) -> Result<(), ()> {
    let val = call_to_json(vm, val, key).map_err(|_| ())?;
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
        let s = unsafe { (*val.as_string_ptr()).data.clone() };
        stringify_string(&s, out);
    } else if val.is_object() {
        let obj_ptr = val.as_js_object_ptr();
        if obj_ptr.is_null() {
            out.push_str("null");
            return Ok(());
        }

        if !visited.insert(obj_ptr as *const JsObject) {
            return Err(());
        }

        let obj = unsafe { &*obj_ptr };
        if obj.is_array() {
            stringify_array(vm, obj, visited, out, replacer_fn, replacer_whitelist, space, indent_level)?;
        } else {
            stringify_object(vm, obj, visited, out, replacer_fn, replacer_whitelist, space, indent_level)?;
        }

        visited.remove(&(obj_ptr as *const JsObject));
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

#[allow(clippy::too_many_arguments)]
fn stringify_object<H: VmHost>(
    vm: &mut H, obj: &JsObject, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&HashSet<String>>, space: &str, indent_level: usize,
) -> Result<(), ()> {
    let has_space = !space.is_empty();
    out.push('{');

    let keys = walk_own_keys(vm, obj);
    // Pre-compute (name, position) pairs with whitelist filtering
    let kc = vm.kernel_core().clone();
    let perm_interner = kc.perm_interner();
    let entries: Vec<(String, u32)> = keys
        .into_iter()
        .filter_map(|(si, pos)| {
            let name = perm_interner.lookup(si).unwrap_or("").to_string();
            if let Some(whitelist) = replacer_whitelist {
                if !whitelist.contains(&name) {
                    return None;
                }
            }
            Some((name, pos))
        })
        .collect();

    let mut first = true;
    for (name, pos) in entries {
        let val = obj.get_prop_at(pos);

        // Replacer function callback (toJSON handled by jsvalue_to_json)
        let val = if let Some(replacer) = replacer_fn {
            let key_val = vm.new_string(&name);
            let holder = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
            match vm.call_function_sync(replacer, holder, &[key_val, val]) {
                Ok(v) => {
                    if v.is_undefined() {
                        continue;
                    }
                    v
                }
                Err(_) => return Err(()),
            }
        } else {
            val
        };

        let is_function = val.is_object() && {
            let ptr = val.as_js_object_ptr();
            !ptr.is_null() && unsafe { (*ptr).is_function() }
        };
        if val.is_undefined() || is_function {
            continue;
        }

        if !first && !has_space {
            out.push(',');
        } else if !first {
            out.push_str(",\n");
        }
        first = false;

        if has_space {
            for _ in 0..indent_level + 1 {
                out.push_str(space);
            }
            stringify_string(&name, out);
            out.push(':');
            out.push(' ');
        } else {
            stringify_string(&name, out);
            out.push(':');
        }

        jsvalue_to_json(vm, val, visited, out, replacer_fn, replacer_whitelist, space, indent_level + 1, &name)?;
    }

    if has_space && !first {
        out.push('\n');
        for _ in 0..indent_level {
            out.push_str(space);
        }
    }
    out.push('}');
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stringify_array<H: VmHost>(
    vm: &mut H, obj: &JsObject, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&HashSet<String>>, space: &str, indent_level: usize,
) -> Result<(), ()> {
    let has_space = !space.is_empty();
    out.push('[');

    let len = obj.prop_vec_len();
    for i in 0..len {
        if i > 0 && !has_space {
            out.push(',');
        } else if i > 0 {
            out.push_str(",\n");
        }

        if has_space {
            out.push('\n');
            for _ in 0..indent_level + 1 {
                out.push_str(space);
            }
        }

        let val = obj.get_prop_at(i);

        let index_str = i.to_string();

        // Replacer function callback (toJSON handled by jsvalue_to_json)
        let val = if let Some(replacer) = replacer_fn {
            let key_val = vm.new_string(&index_str);
            let holder = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
            match vm.call_function_sync(replacer, holder, &[key_val, val]) {
                Ok(v) => {
                    if v.is_undefined() {
                        out.push_str("null");
                        continue;
                    }
                    v
                }
                Err(_) => return Err(()),
            }
        } else {
            val
        };

        let is_function = val.is_object() && {
            let ptr = val.as_js_object_ptr();
            !ptr.is_null() && unsafe { (*ptr).is_function() }
        };
        if is_function || val.is_undefined() {
            out.push_str("null");
        } else {
            jsvalue_to_json(
                vm,
                val,
                visited,
                out,
                replacer_fn,
                replacer_whitelist,
                space,
                indent_level + 1,
                &index_str,
            )?;
        }
    }

    if has_space && len > 0 {
        out.push('\n');
        for _ in 0..indent_level {
            out.push_str(space);
        }
    }
    out.push(']');
    Ok(())
}
