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
    // 后序遍历自该槽展开。返回值 = 根级 reviver 返回值原样（wrapper 的
    // `''` 槽由父级循环施加，根级无父级，槽恒不被触碰）。
    if args.len() > 2 {
        let reviver_val = vm.reg(args[2]);
        if reviver_val.is_object() {
            let rptr = reviver_val.as_js_object_ptr();
            if !rptr.is_null() && unsafe { (*rptr).is_function() } {
                let empty_si = vm.kernel_core().perm_interner().intern("").0;
                let holder = create_wrapper(vm, result);
                let holder_ptr = holder.as_js_object_ptr();
                result = match walk_reviver(vm, holder_ptr, empty_si, reviver_val) {
                    Ok(new_val) => new_val,
                    Err(e) => return NativeResult::Err(e),
                };
            }
        }
    }

    NativeResult::Ok(result)
}

/// InternalizeJSONProperty 后序遍历：Get 读当前值 → 递归子节点并把每子返回值
/// 施加于容器（undefined → [[Delete]]，否则 CreateDataProperty，两失败路径
/// 静默）→ 调用 reviver。返回值 = 本级 reviver 返回值原样，由父级循环施加
/// （根级由 json_parse 直接作 parse 结果）。
fn walk_reviver<H: VmHost>(
    vm: &mut H, holder_ptr: *mut JsObject, key_si: u32, reviver: JsValue,
) -> Result<JsValue, JsValue> {
    let holder_val = JsValue::from_js_object(holder_ptr);

    // 读臂：完整 Get（原型链 + 自身访问器触发），getter 抛出传播原值。
    let val = match vm.ordinary_get(unsafe { &*holder_ptr }, key_si, holder_val) {
        Ok(v) => v,
        Err(msg) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
            return Err(exc);
        }
    };

    // 后序遍历：先递归子节点，每子返回值施加于容器。
    if val.is_object() {
        let obj_ptr = val.as_js_object_ptr();
        if !obj_ptr.is_null() {
            if unsafe { (*obj_ptr).is_array() } {
                let len = unsafe { (*obj_ptr).prop_count() } as usize;
                for i in 0..len {
                    let child_si = make_int_key(i as u32);
                    let new_val = walk_reviver(vm, obj_ptr, child_si, reviver)?;
                    apply_child_result(vm, obj_ptr, child_si, new_val);
                }
            } else {
                let keys = {
                    let obj = unsafe { &*obj_ptr };
                    walk_own_keys(vm, obj)
                };
                for (child_si, _child_pos) in keys {
                    let new_val = walk_reviver(vm, obj_ptr, child_si, reviver)?;
                    apply_child_result(vm, obj_ptr, child_si, new_val);
                }
            }
        }
    }

    // 对当前值调用 reviver，返回值上抛由父级施加。
    let key_val = crate::object::key_si_to_js_value(vm, key_si);
    match vm.call_function_sync(reviver, holder_val, &[key_val, val]) {
        Ok(new_val) => Ok(new_val),
        Err(msg) => {
            // reviver 内抛出的原始值原样传播（不折叠为 TypeError 文本）。
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
            Err(exc)
        }
    }
}

/// 子节点递归返回值施加于容器：`undefined` → [[Delete]]（非可配置保留），
/// 否则 CreateDataProperty（非可配置静默保旧值，新键建自身）。两失败路径
/// 均静默不抛（规范明注）。
fn apply_child_result<H: VmHost>(vm: &mut H, child_ptr: *mut JsObject, child_si: u32, new_val: JsValue) {
    if new_val.is_undefined() {
        let _ = crate::object::delete_own_property(vm, unsafe { &mut *child_ptr }, child_si);
    } else {
        let _ = vm.define_data_property(
            unsafe { &mut *child_ptr },
            child_si,
            new_val,
            PropAttributes::new(true, true, true),
        );
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

/// space 参数文本化（Stringify 步 5）：数字经 ToIntegerOrInfinity 钳 10，
/// 字符串取前 10 字符；装箱 String 走完整 ToString（步 4c 同款语义，
/// 抛出值原样上抛），其余形态维持占位行为。
fn process_space<H: VmHost>(vm: &mut H, val: JsValue) -> Result<String, JsValue> {
    if val.is_int() || val.is_double() {
        let n = oxide_runtime_api::to_integer_or_infinity(val);
        if n.is_nan() || n.is_infinite() || n <= 0.0 {
            return Ok(String::new());
        }
        let clamped = (n as usize).min(10);
        return Ok(" ".repeat(clamped));
    }
    if val.is_string() {
        let s = unsafe { (*val.as_string_ptr()).to_owned_string() };
        return Ok(s.chars().take(10).collect());
    }
    if val.is_object() {
        let ptr = val.as_js_object_ptr();
        if !ptr.is_null() && unsafe { (*ptr).is_string_obj() } {
            let s = match oxide_runtime_api::to_string_value_full(val, vm) {
                Ok(s) => s,
                Err(msg) => {
                    return Err(vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg)));
                }
            };
            let s = unsafe { (*s.as_string_ptr()).to_owned_string() };
            return Ok(s.chars().take(10).collect());
        }
    }
    let s = oxide_runtime_api::to_string(val);
    Ok(s.chars().take(10).collect())
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

/// 白名单元素 → 键名提取（Stringify 步 4）：String/Number 原语直取；装箱
/// String/Number 经 ToPrimitive string hint（toString 优先、valueOf 兜底、
/// 两法皆不可调用落盒值）；其余形态（装箱 Boolean、普通对象、Symbol 等）
/// 返回 None 跳过。方法读取与调用异常传播原始抛出值。
fn whitelist_element_name<H: VmHost>(vm: &mut H, elem: JsValue) -> Result<Option<String>, JsValue> {
    if elem.is_undefined() {
        return Ok(None);
    }
    if elem.is_string() || elem.is_int() || elem.is_double() {
        return Ok(Some(oxide_runtime_api::to_string(elem)));
    }
    if !elem.is_object() {
        return Ok(None);
    }
    let ptr = elem.as_js_object_ptr();
    if ptr.is_null() {
        return Ok(None);
    }
    let obj = unsafe { &*ptr };
    let boxed = obj.boxed_value();
    if !boxed.is_string() && !boxed.is_int() && !boxed.is_double() {
        return Ok(None);
    }

    // ToPrimitive string hint：toString 优先，valueOf 兜底。
    let to_string_si = vm.kernel_core().perm_interner().intern("toString").0;
    let value_of_si = vm.kernel_core().perm_interner().intern("valueOf").0;
    for m_si in [to_string_si, value_of_si] {
        let m = vm.ordinary_get(obj, m_si, elem).map_err(|msg| {
            vm.take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg))
        })?;
        if !m.is_object() {
            continue;
        }
        let fptr = m.as_js_object_ptr();
        if fptr.is_null() || !unsafe { (*fptr).is_function() } {
            continue;
        }
        let r = vm.call_function_sync(m, elem, &[]).map_err(|msg| {
            vm.take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg))
        })?;
        return Ok(Some(oxide_runtime_api::to_string(r)));
    }

    // 装箱原型面理论不可达兜底：盒值直取。
    Ok(Some(oxide_runtime_api::to_string(boxed)))
}

fn call_to_json<H: VmHost>(vm: &mut H, obj_val: JsValue, key: &[u16]) -> Result<JsValue, JsValue> {
    // SerializeJSONProperty 步 2：对象与 BigInt 原语均查 toJSON（Get 对原语
    // 自动装箱），BigInt 的查表对象取 %BigIntPrototype%。
    if !obj_val.is_object() && !obj_val.is_bigint() {
        return Ok(obj_val);
    }
    let obj_ptr = if obj_val.is_object() {
        let p = obj_val.as_js_object_ptr();
        if p.is_null() {
            return Ok(obj_val);
        }
        p
    } else {
        vm.session().builtin_world().bigint_proto.as_ptr() as *mut JsObject
    };
    let tojson_si = vm.kernel_core().perm_interner().intern("toJSON").0;
    // Get 语义查表：访问器形 toJSON 触发 getter（receiver = 原值，BigInt 原语
    // 以自身作 this），getter 异常传播原始抛出值。
    let fn_val = match vm.ordinary_get(unsafe { &*obj_ptr }, tojson_si, obj_val) {
        Ok(v) => v,
        Err(msg) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
            return Err(exc);
        }
    };
    if fn_val.is_object() {
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
    } else {
        Ok(obj_val)
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
    let mut replacer_whitelist: Option<Vec<String>> = None;
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
                    let mut whitelist: Vec<String> = Vec::new();
                    for i in 0..len {
                        let elem = robj.get_prop_at(i);
                        match whitelist_element_name(vm, elem) {
                            Ok(Some(name)) => {
                                // 有序去重：重复项不入列表。
                                if !whitelist.contains(&name) {
                                    whitelist.push(name);
                                }
                            }
                            Ok(None) => {}
                            Err(exc) => return NativeResult::Err(exc),
                        }
                    }
                    replacer_whitelist = Some(whitelist);
                }
            }
        }
    }

    let space = if args.len() > 3 {
        match process_space(vm, vm.reg(args[3])) {
            Ok(s) => s,
            Err(exc) => return NativeResult::Err(exc),
        }
    } else {
        String::new()
    };

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
    replacer_whitelist: Option<&Vec<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
    if val.is_null() {
        out.push_str("null");
    } else if val.is_undefined() {
    } else if val.is_bool() {
        out.push_str(if val.as_bool() { "true" } else { "false" });
    } else if val.is_int() {
        let _ = write!(out, "{}", val.as_int());
    } else if val.is_double() {
        let n = val.as_double();
        if !n.is_finite() {
            out.push_str("null");
        } else {
            oxide_runtime_api::write_number_into(n, out);
        }
    } else if val.is_bigint() {
        // SerializeJSONProperty 步 10：BigInt 无 JSON 文本形态，抛 TypeError。
        return Err(crate::error::create_type_error(vm, "Do not know how to serialize a BigInt"));
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
        // 步 4d：装箱 BigInt 无条件解包 [[BigIntData]]，后续步 10 抛 TypeError。
        if obj.boxed_value().is_bigint() {
            return Err(crate::error::create_type_error(vm, "Do not know how to serialize a BigInt"));
        }
        // 步 4c：装箱 String 走完整 ToString（[[StringData]] 臂）：尊重
        // toString/Symbol.toPrimitive 覆盖、不直读载荷，抛出值原样传播。
        if obj.is_string_obj() {
            let s = match oxide_runtime_api::to_string_value_full(val, vm) {
                Ok(s) => s,
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    visited.remove(&(obj_ptr as *const JsObject));
                    return Err(exc);
                }
            };
            let units = vm.string_units(s);
            stringify_string_units(&units, out);
        } else if obj.is_typed_array_obj() {
            stringify_typed_array(vm, obj, visited, out, replacer_fn, replacer_whitelist, space, indent_level)?;
        } else if obj.is_array() {
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
    replacer_whitelist: Option<&Vec<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
    let has_space = !space.is_empty();
    out.push('{');

    // 键集：白名单给定时 K = P（列表序，不限自身可枚举，值读走 Get）；
    // 无白名单走自身可枚举序（EnumerableOwnPropertyNames）。
    let entries: Vec<(Vec<u16>, u32, Option<u32>)> = if let Some(whitelist) = replacer_whitelist {
        whitelist
            .iter()
            .map(|name| {
                let si = vm.string_key_si(name);
                (key_si_to_units(vm, si), si, None)
            })
            .collect()
    } else {
        let keys = walk_own_keys(vm, obj);
        keys.into_iter()
            .filter(|(_si, pos)| {
                obj.prop_meta_at(*pos)
                    .map(|m| m.attributes.enumerable())
                    .unwrap_or(PropAttributes::DEFAULT_DATA.enumerable())
            })
            .map(|(si, pos)| (key_si_to_units(vm, si), si, Some(pos)))
            .collect()
    };

    let obj_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let mut first = true;
    for (units, si, read_pos) in entries {
        // 规范序 Get → toJSON → replacer（SerializeJSONProperty 步 1、2a、2b）。
        // K = P 时值读经 Get（原型链 + 访问器），无白名单自身槽直读。
        let val = match read_pos {
            Some(pos) => read_json_property_value(vm, obj, obj_val, si, pos)?,
            None => vm.ordinary_get(obj, si, obj_val).map_err(|msg| {
                vm.take_uncaught_value()
                    .unwrap_or_else(|| crate::error::create_type_error(vm, &msg))
            })?,
        };
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
    replacer_whitelist: Option<&Vec<String>>, space: &str, indent_level: usize,
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

/// TypedArray 序列化：整数索引是可枚举自身属性（元素存于原生载荷，不在形状链），
/// 逐元素按对象臂规范序 Get → toJSON → replacer 处理，输出形态与普通对象一致。
#[allow(clippy::too_many_arguments)]
fn stringify_typed_array<H: VmHost>(
    vm: &mut H, obj: &JsObject, visited: &mut HashSet<*const JsObject>, out: &mut String, replacer_fn: Option<JsValue>,
    replacer_whitelist: Option<&Vec<String>>, space: &str, indent_level: usize,
) -> Result<(), JsValue> {
    let has_space = !space.is_empty();
    let this_val = JsValue::from_js_object(obj as *const JsObject as *mut JsObject);
    let view = crate::typed_array::get_typed_array_data(vm, this_val)?;
    // live 口径：buffer 收缩/detach 后按当前 live 长度枚举（detached 空对象形态）。
    let len = crate::typed_array::ta_live_length(view);
    out.push('{');

    // 键集：白名单给定时 K = P（列表序，整数索引名读元素、越界名跳过、
    // 非索引名经 Get 读命名键如 length）；无白名单走升序元素序。
    let entries: Vec<(Vec<u16>, JsValue)> = if let Some(whitelist) = replacer_whitelist {
        let mut list: Vec<(Vec<u16>, JsValue)> = Vec::new();
        for name in whitelist {
            let si = vm.string_key_si(name);
            let val = if is_int_key(si) {
                let idx = int_key_value(si);
                if idx as usize >= len {
                    continue;
                }
                crate::typed_array::typed_array_element_get(vm, obj, idx)
            } else {
                vm.ordinary_get(obj, si, this_val)
            }
            .map_err(|msg| crate::error::create_type_error(vm, &msg))?;
            list.push((key_si_to_units(vm, si), val));
        }
        list
    } else {
        let mut list: Vec<(Vec<u16>, JsValue)> = Vec::with_capacity(len);
        for i in 0..len {
            let val = crate::typed_array::typed_array_element_get(vm, obj, i as u32)
                .map_err(|msg| crate::error::create_type_error(vm, &msg))?;
            list.push((i.to_string().encode_utf16().collect(), val));
        }
        list
    };

    let mut first = true;
    for (index_units, val) in entries {
        let val = call_to_json(vm, val, &index_units)?;

        // replacer 函数回调。
        let val = if let Some(replacer) = replacer_fn {
            let key_val = vm.new_string_units_owned(index_units.clone());
            match vm.call_function_sync(replacer, this_val, &[key_val, val]) {
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
            stringify_string_units(&index_units, out);
            out.push(':');
            out.push(' ');
        } else {
            stringify_string_units(&index_units, out);
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
