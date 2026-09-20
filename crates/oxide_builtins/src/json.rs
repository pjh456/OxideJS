use std::collections::HashSet;
use std::fmt::Write;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{int_key_value, is_int_key, make_int_key};
use oxide_types::value::JsValue;

use crate::object::walk_own_keys;

use oxide_runtime_api::{NativeResult, VmHost};

/// `JSON.parse(text, reviver)`：解析 JSON 文本为 JS 值（经 serde_json）。
/// 提供 reviver 时以后序遍历逐属性调用 reviver 重建值。
pub fn json_parse<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "JSON.parse requires 1 argument"));
    }
    let val = vm.reg(args[1]);
    if !val.is_string() {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "JSON.parse: argument is not a string"));
    }
    let text = {
        // SAFETY: val 已确认是字符串值。
        unsafe { (*val.as_string_ptr()).to_owned_string() }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(crate::error::create_syntax_error(vm, &format!("{}", e))),
    };

    let mut result = value_to_jsvalue(vm, &parsed);

    // reviver 遍历以 holder 包装对象为根：解析得到的根值存于空串键槽，
    // 后序遍历自该槽展开，重建完成后以槽内最终值作为返回值。
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

    // 后序遍历：先递归处理子节点。
    if val.is_object() {
        let obj_ptr = val.as_js_object_ptr();
        if !obj_ptr.is_null() {
            let obj = unsafe { &*obj_ptr };
            if obj.is_array() {
                let len = obj.prop_count() as usize;
                for i in 0..len {
                    let child_si = make_int_key(i as u32);
                    walk_reviver(vm, obj_ptr, child_si, reviver)?;
                }
                // 子节点可能已改写父级，重新读取当前值。
                val = holder.get_prop_at(pos);
            } else {
                let keys = walk_own_keys(vm, obj);
                for (child_si, _child_pos) in keys {
                    walk_reviver(vm, obj_ptr, child_si, reviver)?;
                }
                // 子节点可能已改写父级，重新读取当前值。
                val = holder.get_prop_at(pos);
            }
        }
    }

    // 对当前值调用 reviver，用返回值覆盖属性槽。
    let key_val = crate::object::key_si_to_js_value(vm, key_si);
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
                // 键经字符串规范化：规范数字串（"0"/"5"）映射整数键，与属性访问统一。
                let si = vm.string_key_si(key);
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
        let s = unsafe { (*val.as_string_ptr()).to_owned_string() };
        s.chars().take(10).collect()
    } else {
        let s = oxide_runtime_api::to_string(val);
        s.chars().take(10).collect()
    }
}

/// 键 si 物化为单元序列：整数键 → ASCII 数字串，字符串键 → 码表键经 `decode_key`
/// 还原（含孤立 surrogate 单元）。序列化文本与 toJSON/replacer 键参数均用此真实串。
fn key_si_to_units<H: VmHost>(vm: &H, si: u32) -> Vec<u16> {
    if is_int_key(si) {
        int_key_value(si).to_string().encode_utf16().collect()
    } else {
        vm.kernel_core()
            .perm_interner()
            .lookup(si)
            .map(oxide_kernel::string_forge::decode_key)
            .unwrap_or_default()
    }
}

fn call_to_json<H: VmHost>(vm: &mut H, obj_val: JsValue, key: &[u16]) -> Result<JsValue, JsValue> {
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
                let key_val = vm.new_string_units_owned(key.to_vec());
                match vm.call_function_sync(fn_val, obj_val, &[key_val]) {
                    Ok(v) => Ok(v),
                    Err(msg) => {
                        // 用户抛出的原始值原样传播（toJSON 抛非 Error 值不降级为 TypeError）。
                        let exc = vm
                            .take_uncaught_value()
                            .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                        Err(exc)
                    }
                }
            } else {
                Ok(obj_val)
            }
        }
        _ => Ok(obj_val),
    }
}

/// 序列化族单自身属性值读（SerializeJSONProperty 步 2 的 Get）：数据属性直读
/// 存储槽；访问器属性触发 getter（this = 容器对象），getter 异常传播原始
/// 抛出值。
fn read_json_property_value<H: VmHost>(
    vm: &mut H, obj: &JsObject, obj_val: JsValue, si: u32, pos: u32,
) -> Result<JsValue, JsValue> {
    if obj.is_accessor_meta(pos) {
        match vm.ordinary_get(obj, si, obj_val) {
            Ok(value) => Ok(value),
            Err(msg) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                Err(exc)
            }
        }
    } else {
        Ok(obj.get_prop_at(pos))
    }
}

/// `JSON.stringify(value, replacer, space)`：将 JS 值序列化为 JSON 文本。
/// 支持 replacer 函数/属性白名单、toJSON 钩子、缩进与循环引用检测。
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
                    let len = robj.prop_count() as usize;
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

    let value = match call_to_json(vm, value, &[]) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };

    let value = if let Some(replacer) = replacer_fn {
        let key_val = vm.new_string("");
        match vm.call_function_sync(replacer, holder, &[key_val, value]) {
            Ok(v) => v,
            Err(msg) => {
                let exc = vm
                    .take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                return NativeResult::Err(exc);
            }
        }
    } else {
        value
    };

    let mut visited = HashSet::new();
    let mut output = String::new();
    let indent_level: usize = 0;
    match jsvalue_to_json(
        vm,
        value,
        &mut visited,
        &mut output,
        replacer_fn,
        replacer_whitelist.as_ref(),
        &space,
        indent_level,
    ) {
        Ok(()) => NativeResult::Ok(vm.new_string_owned(output)),
        Err(exc) => NativeResult::Err(exc),
    }
}

/// 值位序列化（SerializeValue 及对象/数组展开）。toJSON 钩子不在本入口应用——
/// 唯一应用点在调用方（顶层 `json_stringify` 与 `stringify_object`/`stringify_array`
/// 的属性循环内，Get 之后、replacer 之前），保证每值位恰一次且与 replacer 顺序
/// 符合规范。`Err` 携带的原始异常值（含环检 TypeError）原样上抛。
#[allow(clippy::too_many_arguments)]
fn jsvalue_to_json<H: VmHost>(
    vm: &mut H, val: JsValue, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&HashSet<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
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
        } else {
            oxide_runtime_api::write_number_into(n, out);
        }
    } else if val.is_string() {
        // SAFETY: val 已确认是字符串值；单元视图按码单元流序列化（见 stringify_string_units）。
        let units = unsafe { (*val.as_string_ptr()).units() };
        stringify_string_units(&units, out);
    } else if val.is_object() {
        let obj_ptr = val.as_js_object_ptr();
        if obj_ptr.is_null() {
            out.push_str("null");
            return Ok(());
        }

        if !visited.insert(obj_ptr as *const JsObject) {
            return Err(crate::error::create_type_error(vm, "Converting circular structure to JSON"));
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

/// JSON 字符串序列化（按码单元流）：代理对原样输出 astral 字符，非配对孤立
/// surrogate 输出 \uXXXX（well-formed JSON 要求，round-trip 经 JSON.parse 复原同值），
/// C0/C1 控制码输出 \uXXXX，其余按 JSON 转义规则。
fn stringify_string_units(units: &[u16], out: &mut String) {
    out.push('"');
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if (0xD800..=0xDBFF).contains(&u) && i + 1 < units.len() && (0xDC00..=0xDFFF).contains(&units[i + 1]) {
            let cp = 0x10000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(units[i + 1]) - 0xDC00);
            if let Some(c) = char::from_u32(cp) {
                out.push(c);
            }
            i += 2;
            continue;
        }
        match u {
            0x22 => out.push_str("\\\""),
            0x5C => out.push_str("\\\\"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x00..=0x1F | 0x7F..=0x9F | 0xD800..=0xDFFF => {
                let _ = write!(out, "\\u{:04x}", u);
            }
            _ => {
                if let Some(c) = char::from_u32(u32::from(u)) {
                    out.push(c);
                }
            }
        }
        i += 1;
    }
    out.push('"');
}

#[allow(clippy::too_many_arguments)]
fn stringify_object<H: VmHost>(
    vm: &mut H, obj: &JsObject, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&HashSet<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
    let has_space = !space.is_empty();
    out.push('{');

    let keys = walk_own_keys(vm, obj);
    // 仅序列化可枚举自身属性（规范 EnumerableOwnPropertyNames）。数组 replacer
    // 白名单判定在 Get 之前（SerializeJSONObject 步 4a），非白名单键不触发 getter。
    let entries: Vec<(Vec<u16>, String, u32, u32)> = keys
        .into_iter()
        .filter(|(_si, pos)| {
            obj.prop_meta_at(*pos)
                .map(|m| m.attributes.enumerable())
                .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable())
        })
        .filter_map(|(si, pos)| {
            let units = key_si_to_units(vm, si);
            // 白名单匹配走 lossy 串形态（与白名单构造侧 to_string 同口径）。
            let name = String::from_utf16_lossy(&units);
            if let Some(whitelist) = replacer_whitelist {
                if !whitelist.contains(&name) {
                    return None;
                }
            }
            Some((units, name, pos, si))
        })
        .collect();

    let obj_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let mut first = true;
    for (units, _name, pos, si) in entries {
        // 规范序 Get → toJSON → replacer（SerializeJSONProperty 步 1、2a、2b）。
        let val = read_json_property_value(vm, obj, obj_val, si, pos)?;
        let val = call_to_json(vm, val, &units)?;

        // replacer 函数回调。
        let val = if let Some(replacer) = replacer_fn {
            let key_val = vm.new_string_units_owned(units.clone());
            match vm.call_function_sync(replacer, obj_val, &[key_val, val]) {
                Ok(v) => {
                    if v.is_undefined() {
                        continue;
                    }
                    v
                }
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    return Err(exc);
                }
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
            stringify_string_units(&units, out);
            out.push(':');
            out.push(' ');
        } else {
            stringify_string_units(&units, out);
            out.push(':');
        }

        jsvalue_to_json(vm, val, visited, out, replacer_fn, replacer_whitelist, space, indent_level + 1)?;
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
) -> Result<(), JsValue> {
    let has_space = !space.is_empty();
    out.push('[');

    let obj_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let len = obj.prop_count() as usize;
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

        let index_str = i.to_string();
        let index_units: Vec<u16> = index_str.encode_utf16().collect();

        // 规范序 Get → toJSON → replacer（数组元素位同款，getter 的 this = 数组自身）。
        let val = read_json_property_value(vm, obj, obj_val, make_int_key(i as u32), i as u32)?;
        let val = call_to_json(vm, val, &index_units)?;

        // replacer 函数回调。
        let val = if let Some(replacer) = replacer_fn {
            let key_val = vm.new_string(&index_str);
            match vm.call_function_sync(replacer, obj_val, &[key_val, val]) {
                Ok(v) => {
                    if v.is_undefined() {
                        out.push_str("null");
                        continue;
                    }
                    v
                }
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    return Err(exc);
                }
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
            jsvalue_to_json(vm, val, visited, out, replacer_fn, replacer_whitelist, space, indent_level + 1)?;
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
