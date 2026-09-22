use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;
use crate::regexp::build_groups_object;

use super::common::{try_string, MatchText};
use super::{
    as_units, byte_to_unit, find_units, make_string_array_values, make_units_array, map_well_formed_segments,
    split_limit_to_uint32, this_text, this_units, unit_to_byte,
};

// ── 正则替换（单元口径） ────────────────────────────────────────────────

/// 调用函数 replacer 并把返回值转为单元序列；调用抛出的异常原样恢复。
fn call_replacer<H: VmHost>(vm: &mut H, replacer: JsValue, cb_args: &[JsValue]) -> Result<Vec<u16>, JsValue> {
    match vm.call_function_sync(replacer, JsValue::undefined(), cb_args) {
        Ok(result) => match oxide_runtime_api::to_units_full(result, vm) {
            Ok(units) => Ok(units),
            Err(_) => {
                // 转换失败（Symbol 等）或回调异常均须原样传播。
                if let Some(exc) = vm.take_uncaught_value() {
                    return Err(exc);
                }
                Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"))
            }
        },
        Err(err) => Err(vm
            .take_uncaught_value()
            .unwrap_or_else(|| crate::error::create_type_error(vm, &format!("replace replacer: {}", err)))),
    }
}

/// 判断值是否为 RegExp 对象：非对象、空指针、原型非对象均返回 false，
/// 原型指针与 `%RegExpPrototype%` 恒等比较。
fn is_regexp_obj<H: VmHost>(val: JsValue, vm: &H) -> bool {
    if !val.is_object() {
        return false;
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return false;
    }
    let obj = unsafe { &*ptr };
    let proto = obj.proto();
    if !proto.is_object() {
        return false;
    }
    let proto_ptr = proto.as_js_object_ptr();
    if proto_ptr.is_null() {
        return false;
    }
    let rp = vm.session().builtin_world().regexp_proto.as_ptr() as *mut JsObject;
    std::ptr::eq(proto_ptr, rp)
}

/// IsRegExp 口径判定：非对象 false；对象一律 Get(@@match) 后 ToBoolean
/// （真 RegExp 亦读，getter 副作用可观测；getter 抛错恢复原异常上抛）。
fn is_regexp_live<H: VmHost>(vm: &mut H, val: JsValue) -> Result<bool, JsValue> {
    if !val.is_object() {
        return Ok(false);
    }
    let match_key = oxide_types::private_key::make_well_known_symbol_key(1);
    let ptr = val.as_js_object_ptr();
    // SAFETY: val 已校验为非空对象值。
    let obj = unsafe { &*ptr };
    let b = match vm.ordinary_get(obj, match_key, val) {
        Ok(v) => v,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::error::create_type_error(vm, "cannot read @@match"));
        }
    };
    Ok(oxide_runtime_api::to_boolean(b))
}

/// GetMethod(searchValue, @@replace)：undefined/null 返回 None，不可调用抛
/// TypeError，属性读抛错恢复原异常值。
fn rx_get_replace_method<H: VmHost>(
    vm: &mut H, rx: *mut JsObject, this_val: JsValue,
) -> Result<Option<JsValue>, JsValue> {
    let replace_key = oxide_types::private_key::make_well_known_symbol_key(2);
    // SAFETY: this_val 为对象值，rx 与其同址。
    let func = match vm.ordinary_get(unsafe { &*rx }, replace_key, this_val) {
        Ok(v) => v,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::error::create_type_error(vm, "cannot read @@replace"));
        }
    };
    if func.is_undefined() || func.is_null() {
        return Ok(None);
    }
    if !crate::iterator::is_callable(func) {
        return Err(crate::error::create_type_error(vm, "Symbol.replace is not callable"));
    }
    Ok(Some(func))
}

/// 字符串臂的替换形态：功能替换器（回调值）或非功能替换串单元序列，
/// 恰好其一在场。
enum ArmReplacer<'a> {
    Fn(JsValue),
    Text(&'a [u16]),
}

/// 字符串臂：子串定位 + 逐命中替换。功能替换器经
/// Call(replaceValue, undefined, «matched, position, string»)+ToString；非功能
/// 走 GetSubstitution（捕获表空、namedCaptures undefined）。空模式在每个码元
/// 边界命中（replaceAll 含串尾，replace 仅串首）。
fn string_arm_replace<H: VmHost>(
    vm: &mut H, text: &[u16], search: &[u16], replacer: &ArmReplacer<'_>, all: bool, s_val: JsValue,
) -> NativeResult {
    let search_length = search.len();
    let mut positions: Vec<usize> = Vec::new();
    if search_length == 0 {
        positions.push(0);
        if all {
            for i in 1..=text.len() {
                positions.push(i);
            }
        }
    } else {
        let mut start = 0;
        while let Some(p) = find_units(text, search, start) {
            positions.push(p);
            if !all {
                break;
            }
            start = p + search_length;
        }
    }
    let mut out = Vec::new();
    let mut last_end = 0;
    for p in positions {
        out.extend_from_slice(&text[last_end..p]);
        let matched = &text[p..p + search_length];
        let repl = match replacer {
            ArmReplacer::Fn(f) => {
                let cb_args = [vm.new_string_units(matched), JsValue::int(p as i32), s_val];
                try_string!(call_replacer(vm, *f, &cb_args))
            }
            ArmReplacer::Text(r) => match crate::regexp::get_substitution_units(vm, matched, text, p, &[], None, r) {
                Ok(u) => u,
                Err(e) => return NativeResult::Err(e),
            },
        };
        out.extend_from_slice(&repl);
        last_end = p + search_length;
    }
    out.extend_from_slice(&text[last_end..]);
    NativeResult::Ok(vm.new_string_units_owned(out))
}

// ── 拆分 / 正则匹配 ─────────────────────────────────────────────────────

/// GetMethod(sep, @@split)：undefined/null 返回 None（落字符串臂），不可调用
/// 抛 TypeError，属性读抛错恢复原异常值。
fn rx_get_split_method<H: VmHost>(vm: &mut H, sep: JsValue) -> Result<Option<JsValue>, JsValue> {
    let split_key = oxide_types::private_key::make_well_known_symbol_key(4);
    let ptr = sep.as_js_object_ptr();
    // SAFETY: 调用方已校验 sep 为非空对象值。
    let func = match vm.ordinary_get(unsafe { &*ptr }, split_key, sep) {
        Ok(v) => v,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::error::create_type_error(vm, "cannot read @@split"));
        }
    };
    if func.is_undefined() || func.is_null() {
        return Ok(None);
    }
    if !crate::iterator::is_callable(func) {
        return Err(crate::error::create_type_error(vm, "Symbol.split is not callable"));
    }
    Ok(Some(func))
}

/// `String.prototype.split(separator, limit)`：按分隔符拆分为字符串数组；
/// 分隔符可为 RegExp（含捕获组）或字符串。空分隔按单元逐个产出（规格口径，
/// 孤立 surrogate 为 1 单元元素）。
///
/// # 步骤
/// 1. receiver 前置校验（RequireObjectCoercible，纯 is_* 读取，先于一切分叉）。
/// 2. separator 为对象时：GetMethod(separator, @@split)，可调用则
///    Call(splitter, separator, «thisValue, limit»)——传原始 this/limit
///    寄存器不预转换，结果原值返回（RegExp 分隔经此委托 Symbol.split）。
/// 3. string = ToString(this)（转换异常传播）。
/// 4. lim：limit 缺省或 undefined 为 2^32-1，否则 ToUint32（mod 2^32 回绕）。
/// 5. separatorString = ToString(separator)（转换异常传播；先于 lim = 0 判定）。
/// 6. lim = 0 → 空数组（先于 sep-undefined 判定）。
/// 7. separator 为 undefined → [string]。
/// 8. 空分隔逐码元产出（clamp(lim, 0, 串长)）；非空分隔搜索循环，尾段恒推。
///
/// # 边界与前提
/// - 规范序位：GetMethod 派发先于 ToString(this)，ToUint32(limit) 先于
///   ToString(separator)，ToString(separator) 先于 lim = 0 判定，
///   lim = 0 判定先于 sep-undefined 判定。
/// - 各转换（this/limit/separator）抛出的原生异常原样传播。
pub fn string_split<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.split called with {} args", args.len());
    let this_val = vm.reg(args[0]);

    // receiver 前置校验：null/undefined 抛 TypeError（RequireObjectCoercible，
    // 纯 is_* 读取无用户代码，先于一切分叉）；symbol 的 ToString 必败，提前抛。
    if this_val.is_null() || this_val.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "String.prototype method called on null or undefined",
        ));
    }
    if this_val.is_symbol() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
    }

    let sep_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let limit_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };

    // 对象分隔符经 GetMethod 派发：调用定义则 Call 后原值返回，须先于
    // ToString(this) 短路（实参为原始寄存器，不预转换）。
    if sep_val.is_object() {
        let splitter = try_string!(rx_get_split_method(vm, sep_val));
        if let Some(splitter) = splitter {
            return match vm.call_function_sync(splitter, sep_val, &[this_val, limit_val]) {
                Ok(r) => NativeResult::Ok(r),
                Err(_) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    NativeResult::Err(crate::error::create_type_error(vm, "split matcher call failed"))
                }
            };
        }
    }

    // string = ToString(this)。
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();

    // lim = ToUint32(limit)：undefined 为 2^32-1，对象经完整 ToNumber（异常传播）。
    let lim = try_string!(split_limit_to_uint32(vm, limit_val));

    // separatorString = ToString(separator)（先于 lim = 0 判定：sep 转换
    // 抛错时 lim = 0 的早退不生效）。
    let sep_units: Vec<u16> = try_string!(as_units(vm, sep_val)).into_owned();

    if lim == 0 {
        return NativeResult::Ok(make_units_array(vm, Vec::new()));
    }
    // separator 为 undefined 时返回单元素 [string]。
    if sep_val.is_undefined() {
        return NativeResult::Ok(make_units_array(vm, vec![s.to_vec()]));
    }

    if sep_units.is_empty() {
        // 每单元一个元素（ASCII 走单字符缓存零分配，其余 1 单元串创建）。
        let mut values = Vec::with_capacity(s.len().min(lim));
        for &u in s.iter().take(lim) {
            values.push(match vm.single_unit(u) {
                Some(v) => v,
                None => vm.new_string_units(&[u]),
            });
        }
        return NativeResult::Ok(make_string_array_values(vm, values));
    }

    // 非空字符串分隔：手动查找循环 + limit。
    let mut parts: Vec<Vec<u16>> = Vec::new();
    let mut start = 0;
    loop {
        if parts.len() >= lim {
            break;
        }
        match find_units(&s, &sep_units, start) {
            Some(p) => {
                parts.push(s[start..p].to_vec());
                start = p + sep_units.len();
            }
            None => {
                parts.push(s[start..].to_vec());
                break;
            }
        }
    }
    NativeResult::Ok(make_units_array(vm, parts))
}

/// `replace`/`replaceAll` 共用的分叉实现：
///
/// # 步骤
/// 1. receiver 前置校验（RequireObjectCoercible，纯 is_* 读取）。
/// 2. searchValue 非 null/undefined 时：IsRegExp（真 RegExp 原型恒等；其余对象
///    Get @@match 后 ToBoolean，getter 抛错传播）；replaceAll 命中正则时 live
///    Get "flags" + RequireObjectCoercible + ToString 须含 "g"。
/// 3. GetMethod(searchValue, @@replace) 定义（可调用）则
///    Call(matcher, searchValue, «this, replaceValue»)，结果原值返回（不
///    ToString）；真 RegExp 默认经此委托 regexp_symbol_replace（参数槽位与
///    直接派发一致）。
/// 4. 字符串臂：string = ToString(this)、searchString = ToString(searchValue)
///    （异常传播），子串定位后逐命中替换（string_arm_replace）。
///
/// # 边界与前提
/// - 缺省 searchValue/replaceValue 均按 ToString(undefined)="undefined" 处理
///   （`s.replace()` 全缺省等价于把 "undefined" 替换为 "undefined"，结果与
///   原串一致；`s.replace("b")` 得到 "aundefinedc" 而非 "ac"）。
/// - searchValue 为 null/undefined 时跳过整段第 2 步（无 IsRegExp/GetMethod）。
/// - searchValue 的 @@replace 为 undefined/null（含真 RegExp 自置 undefined）
///   回退字符串臂，searchString 取 ToString(searchValue)。
fn string_replace_impl<H: VmHost>(vm: &mut H, args: &[u8], all: bool) -> NativeResult {
    let this_val = vm.reg(args[0]);

    // receiver 前置校验：null/undefined/symbol 抛 TypeError（规范第 1 步
    // RequireObjectCoercible，纯 is_* 读取无用户代码，先于一切分叉）。
    if this_val.is_null() || this_val.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "String.prototype method called on null or undefined",
        ));
    }
    if this_val.is_symbol() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
    }

    let pattern_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let replacement_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };

    // 第 2 步：searchValue 非 null/undefined 的对象走 IsRegExp + flags +
    // GetMethod；matcher 定义则 Call 后原值返回。
    if !pattern_val.is_null() && !pattern_val.is_undefined() && pattern_val.is_object() {
        let is_regexp = match is_regexp_live(vm, pattern_val) {
            Ok(b) => b,
            Err(e) => return NativeResult::Err(e),
        };
        if all && is_regexp {
            // 规范序：Get "flags"（单读，getter 异常传播）→
            // RequireObjectCoercible（undefined/null 抛 TypeError）→
            // ToString 须含 "g"（转换异常传播）。
            let re_ptr = pattern_val.as_js_object_ptr();
            // SAFETY: pattern_val 已校验为非空对象值。
            let flags_val = match vm.ordinary_get(
                unsafe { &*re_ptr },
                vm.kernel_core().perm_interner().intern("flags").0,
                pattern_val,
            ) {
                Ok(v) => v,
                Err(_) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(vm, "cannot read flags"));
                }
            };
            if flags_val.is_undefined() || flags_val.is_null() {
                return NativeResult::Err(crate::error::create_type_error(
                    vm,
                    "String.prototype.replaceAll called on a regex without the global flag",
                ));
            }
            let flags = match oxide_runtime_api::to_string_value_full(flags_val, vm) {
                Ok(v) => v,
                Err(_) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
                }
            };
            let flags_str = String::from_utf16_lossy(&vm.string_units(flags));
            if !flags_str.contains('g') {
                return NativeResult::Err(crate::error::create_type_error(
                    vm,
                    "String.prototype.replaceAll called on a regex without the global flag",
                ));
            }
        }
        let re_ptr = pattern_val.as_js_object_ptr();
        let matcher = match rx_get_replace_method(vm, re_ptr, pattern_val) {
            Ok(m) => m,
            Err(e) => return NativeResult::Err(e),
        };
        if let Some(matcher) = matcher {
            // Call(matcher, searchValue, « this, replaceValue »)。
            return match vm.call_function_sync(matcher, pattern_val, &[this_val, replacement_val]) {
                Ok(r) => NativeResult::Ok(r),
                Err(_) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    NativeResult::Err(crate::error::create_type_error(vm, "replace matcher call failed"))
                }
            };
        }
    }

    // 字符串臂：string/searchString 完整 ToString（对象 toString/valueOf 抛错
    // 原样传播）；replacer 功能判定 IsCallable，非功能替换串先 ToString
    // （Symbol 抛 TypeError，对象转换抛错传播）。
    let (s_units, s_val) = if this_val.is_string() {
        // SAFETY: this_val 为字符串值，借用即时消费。
        let sp = unsafe { &*this_val.as_string_ptr() };
        (sp.units().into_owned(), this_val)
    } else {
        let v = match oxide_runtime_api::to_string_value_full(this_val, vm) {
            Ok(v) => v,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        };
        // SAFETY: v 为字符串值，借用即时消费。
        let sp = unsafe { &*v.as_string_ptr() };
        (sp.units().into_owned(), v)
    };
    let search_units = if pattern_val.is_string() {
        // SAFETY: pattern_val 为字符串值，借用即时消费。
        let sp = unsafe { &*pattern_val.as_string_ptr() };
        sp.units().into_owned()
    } else if pattern_val.is_undefined() {
        "undefined".encode_utf16().collect()
    } else {
        match oxide_runtime_api::to_units_full(pattern_val, vm) {
            Ok(u) => u,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        }
    };
    let functional = crate::iterator::is_callable(replacement_val);
    let replacement_units: Vec<u16> = if functional {
        Vec::new()
    } else {
        match crate::regexp::replacement_units(vm, replacement_val) {
            Ok(u) => u,
            Err(e) => return NativeResult::Err(e),
        }
    };
    let replacer = if functional {
        ArmReplacer::Fn(replacement_val)
    } else {
        ArmReplacer::Text(&replacement_units)
    };
    string_arm_replace(vm, &s_units, &search_units, &replacer, all, s_val)
}

/// `String.prototype.replace(pattern, replacement)`：替换首个匹配；
/// 支持 RegExp（global 全替换）、字符串以及函数替换器。
pub fn string_replace<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.replace called with {} args", args.len());
    string_replace_impl(vm, args, false)
}

/// `String.prototype.match(pattern)`：按 RegExp 匹配；global 返回全部匹配数组，
/// 否则返回首个匹配及捕获组，无匹配返回 null。
pub fn string_match_fn<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.match called with {} args", args.len());
    // 正则判定与参数字符串转换先行（&mut 路径），后借 this 匹配扫描。
    let pattern_val = if args.len() >= 2 { Some(vm.reg(args[1])) } else { None };
    let is_re = pattern_val.map(|v| is_regexp_obj(v, vm)).unwrap_or(false);
    // global 标志前置读：flags 串共享借用与下方 text 转换的 &mut 借用不可重叠。
    let is_global = if is_re {
        let re_val = match pattern_val {
            Some(v) => v,
            None => return NativeResult::Err(crate::error::create_type_error(vm, "expected regexp")),
        };
        let re_ptr = re_val.as_js_object_ptr();
        // SAFETY: is_re 已保证 pattern_val 为非空对象且 proto 恒等 RegExp.prototype。
        let re = unsafe { &*re_ptr };
        re.native_fn().is_some() && crate::regexp::regexp_has_flag(vm, re, 'g')
    } else {
        false
    };
    let text = try_string!(this_text(vm, args));
    // 缺参按 undefined 模式（规范：构造空模式正则，串头命中）。
    let pattern_val = match pattern_val {
        Some(v) => v,
        None => JsValue::undefined(),
    };
    if is_re {
        let re_ptr = pattern_val.as_js_object_ptr();
        if is_global {
            let re = unsafe { &*re_ptr };
            let fn_ptr = match re.native_fn() {
                Some(p) => p,
                None => return NativeResult::Ok(JsValue::null()),
            };
            // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
            let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
            let mut matches: Vec<Vec<u16>> = Vec::new();
            text.for_each_match(regex, |m| {
                let range = m.range();
                matches.push(text.slice(range.start, range.end).into_owned());
            });
            if matches.is_empty() {
                return NativeResult::Ok(JsValue::null());
            }
            return NativeResult::Ok(make_units_array(vm, matches));
        }
        // 非 global：GetMethod(rx, @@match) + Invoke，结果原值返回
        // （默认 @@match 非 global 臂交付 exec 结果本体，index/input/groups/
        // indices 与 exec 同源；影子 @@match 可替换）。
        let s_units = text.units().into_owned();
        let s_val = vm.new_string_units_owned(s_units);
        let matcher = match rx_get_match_method(vm, re_ptr, pattern_val) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        };
        return match vm.call_function_sync(matcher, pattern_val, &[s_val]) {
            Ok(r) => NativeResult::Ok(r),
            Err(e) => NativeResult::Err(crate::iterator::engine_error(vm, &e)),
        };
    }
    // 非正则模式：Construct %RegExp%（undefined → 空模式、其余经 ToString），
    // 再 GetMethod(rx, @@match) + Invoke；转换抛错（含 Symbol）原样传播。
    let s_units = text.units().into_owned();
    let s_val = vm.new_string_units_owned(s_units);
    let regexp_ctor = vm.session().builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
    let rx_val = match vm.construct_ctor(JsValue::from_js_object(regexp_ctor), &[pattern_val]) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if !rx_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "match pattern constructor must return an object",
        ));
    }
    let matcher = match rx_get_match_method(vm, rx_val.as_js_object_ptr(), rx_val) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    match vm.call_function_sync(matcher, rx_val, &[s_val]) {
        Ok(r) => NativeResult::Ok(r),
        Err(e) => NativeResult::Err(crate::iterator::engine_error(vm, &e)),
    }
}

/// GetMethod(rx, @@match)：可调用返回之，null/undefined/不可调用抛 TypeError；
/// 属性读抛错恢复原异常值。
fn rx_get_match_method<H: VmHost>(vm: &mut H, rx: *mut JsObject, this_val: JsValue) -> Result<JsValue, JsValue> {
    let match_key = oxide_types::private_key::make_well_known_symbol_key(1);
    let matcher = match vm.ordinary_get(unsafe { &*rx }, match_key, this_val) {
        Ok(v) => v,
        Err(e) => return Err(crate::iterator::engine_error(vm, &e)),
    };
    if !crate::iterator::is_callable(matcher) {
        return Err(crate::error::create_type_error(vm, "RegExp @@match is not callable"));
    }
    Ok(matcher)
}

/// `String.prototype.search(pattern)`：返回首个匹配位置（码元），无匹配返回 -1。
pub fn string_search<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.search called with {} args", args.len());
    // 参数读取守卫先行：无参调用缺省 searchString 为 undefined（args 仅含 this 槽）。
    let pattern_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };

    // 正则判定与参数转换先行（&mut 路径），后借 this 扫描。
    let is_re = args.len() >= 2 && is_regexp_obj(pattern_val, vm);
    let pattern: Vec<u16> = if args.len() >= 2 && !is_re {
        try_string!(as_units(vm, pattern_val)).into_owned()
    } else {
        Vec::new()
    };
    let text = try_string!(this_text(vm, args));
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    if is_re {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(JsValue::int(-1)),
        };
        // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
        if let Some(m) = text.find_from_units(regex, 0) {
            return NativeResult::Ok(JsValue::int(text.unit_pos(m.range().start) as i32));
        }
        return NativeResult::Ok(JsValue::int(-1));
    }
    // 空模式命中串头（规格口径）。
    if pattern.is_empty() {
        return NativeResult::Ok(JsValue::int(0));
    }
    let s = text.units();
    if let Some(pos) = find_units(&s, &pattern, 0) {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `String.prototype.isWellFormed()`：字符串无孤立 surrogate（每个码元要么是
/// 合法字符，要么与相邻码元成代理对）时返回 true。
pub fn string_is_well_formed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.isWellFormed called with {} args", args.len());
    let s = try_string!(this_units(vm, args));
    let mut i = 0;
    while i < s.len() {
        let u = s[i];
        if (0xDC00..=0xDFFF).contains(&u) {
            return NativeResult::Ok(JsValue::bool(false));
        }
        if (0xD800..=0xDBFF).contains(&u) {
            if !(i + 1 < s.len() && (0xDC00..=0xDFFF).contains(&s[i + 1])) {
                return NativeResult::Ok(JsValue::bool(false));
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

/// `String.prototype.toWellFormed()`：把孤立 surrogate 替换为 U+FFFD 返回新字符串。
pub fn string_to_well_formed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.toWellFormed called with {} args", args.len());
    let s = try_string!(this_units(vm, args));
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let u = s[i];
        if (0xD800..=0xDBFF).contains(&u) {
            if i + 1 < s.len() && (0xDC00..=0xDFFF).contains(&s[i + 1]) {
                out.push(u);
                out.push(s[i + 1]);
                i += 2;
                continue;
            }
            out.push(0xFFFD);
        } else if (0xDC00..=0xDFFF).contains(&u) {
            out.push(0xFFFD);
        } else {
            out.push(u);
        }
        i += 1;
    }
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// `String.prototype.normalize(form)`：按 NFC/NFD/NFKC/NFKD 规范化为 Unicode
/// 规范形式（良形段映射，孤立 surrogate 单元原样保留）。
pub fn string_normalize<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.normalize called with {} args", args.len());
    use unicode_normalization::UnicodeNormalization;
    let form = if args.len() > 1 {
        try_string!(as_units(vm, vm.reg(args[1]))).into_owned()
    } else {
        "NFC".encode_utf16().collect()
    };
    // 规范化形式名按良形文本比对（NFC 等名无孤立 surrogate）。
    let form_name = String::from_utf16_lossy(&form);
    let s = try_string!(this_units(vm, args));
    let out = map_well_formed_segments(&s, |seg| match form_name.as_str() {
        "NFD" => seg.nfd().collect(),
        "NFKC" => seg.nfkc().collect(),
        "NFKD" => seg.nfkd().collect(),
        _ => seg.nfc().collect(),
    });
    NativeResult::Ok(vm.new_string_units_owned(out))
}

pub(crate) const MALL_INPUT: &str = "__mal_input__";
pub(crate) const MALL_INDEX: &str = "__mal_index__";
pub(crate) const MALL_RE: &str = "__mal_re__";

/// `String.prototype.matchAll(pattern)`：返回带 `next` 的迭代器，逐步产出全部匹配
/// （要求 RegExp 带 global 标志；普通字符串会被转义成等效正则）。包装器 input
/// 属性存原始字符串值（单元保真），index 属性为码元游标。
///
/// # 步骤
/// 1. receiver 前置校验（RequireObjectCoercible，纯 is_* 读取）。
/// 2. pattern 为 null/undefined：跳过对象分支，直接构造路径。
/// 3. pattern 为对象：真 RegExp 先 flags 单读 g 判定；GetMethod 全链单读，
///    undefined/null 落构造路径，非 callable 非空值抛 TypeError，可调用则
///    Call(matcher, pattern, « thisValue ») 原值返回。
/// 4. 非对象 pattern：跳过对象分支，按字面文本编译载体正则。
///
/// # 边界与前提
/// - ToString(thisValue) 延迟到构造路径实际消费（matcher 直调臂不触发）。
/// - 构造路径（RegExpCreate(pattern, "g") + Invoke）复用
///   `match_all_construct_invoke`，无 callable @@matchAll 时抛 TypeError。
pub fn string_match_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.matchAll called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    // receiver 前置校验：null/undefined/symbol 抛 TypeError（规范第 1-2 步
    // RequireObjectCoercible，纯 is_* 读取无用户代码，先于一切分叉）。
    if this_val.is_null() || this_val.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "String.prototype method called on null or undefined",
        ));
    }
    if this_val.is_symbol() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
    }
    if args.len() < 2 {
        builtins_error!("String.prototype.matchAll: invalid receiver");
        return NativeResult::Err(JsValue::undefined());
    }
    let pattern_val = vm.reg(args[1]);

    // 对象分支条件（IsObject）为假：null/undefined pattern 跳过 flags/GetMethod
    // 全部步骤，直接落构造路径。
    if pattern_val.is_null() || pattern_val.is_undefined() {
        return match_all_construct_invoke(vm, this_val, pattern_val);
    }

    if pattern_val.is_object() {
        // 真 RegExp：flags 单读原始值（getter 副作用/抛错只观察一次）→
        // RequireObjectCoercible → ToString 含 "g" 判定；g 判定先于 GetMethod。
        if is_regexp_obj(pattern_val, vm) {
            let re_ptr = pattern_val.as_js_object_ptr();
            // SAFETY: pattern_val 已校验为非空对象值。
            let re_obj = unsafe { &*re_ptr };
            let flags_si = vm.kernel_core().perm_interner().intern("flags").0;
            let flags_val = match vm.ordinary_get(re_obj, flags_si, pattern_val) {
                Ok(v) => v,
                Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
            };
            if flags_val.is_undefined() || flags_val.is_null() {
                return NativeResult::Err(crate::error::create_type_error(
                    vm,
                    "String.prototype.matchAll: regex must have global flag",
                ));
            }
            let flags_sv = match oxide_runtime_api::to_string_value_full(flags_val, vm) {
                Ok(v) => v,
                Err(_) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
                }
            };
            if !String::from_utf16_lossy(&vm.string_units(flags_sv)).contains('g') {
                return NativeResult::Err(crate::error::create_type_error(
                    vm,
                    "String.prototype.matchAll: regex must have global flag",
                ));
            }
        }
        // GetMethod(pattern, @@matchAll) 全链单次 Get：own 命中 undefined/null
        // 即停（Get 语义），不回退原型链。
        let match_all_key = oxide_types::private_key::make_well_known_symbol_key(7);
        let matcher = match vm.ordinary_get(unsafe { &*pattern_val.as_js_object_ptr() }, match_all_key, pattern_val) {
            Ok(v) => v,
            Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
        };
        // GetMethod 返 undefined/null：落构造路径（ToString(thisValue) →
        // RegExpCreate(pattern, "g") → Invoke 产物 @@matchAll）。
        if matcher.is_undefined() || matcher.is_null() {
            return match_all_construct_invoke(vm, this_val, pattern_val);
        }
        if !crate::iterator::is_callable(matcher) {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "String.prototype.matchAll: matcher is not a function",
            ));
        }
        // Call(matcher, pattern, « thisValue »)：实参为原始 this 值。
        return match vm.call_function_sync(matcher, pattern_val, &[this_val]) {
            Ok(v) => NativeResult::Ok(v),
            Err(e) => NativeResult::err(crate::iterator::engine_error(vm, &e)),
        };
    }

    // 非对象 pattern：对象分支整体跳过，按文本编译为 stub（非捕获组，免额外
    // capture）。
    {
        let pattern_units = try_string!(as_units(vm, pattern_val)).into_owned();
        let pattern_str = String::from_utf16_lossy(&pattern_units);
        let escaped = regress::escape(&pattern_str);
        let rx_str = if escaped.is_empty() {
            String::from("(?:)")
        } else {
            format!("(?:{})", escaped)
        };
        let compiled = match regress::Regex::new(&rx_str) {
            Ok(rx) => rx,
            Err(e) => {
                return NativeResult::Err(crate::error::create_syntax_error(vm, &format!("Invalid regex: {}", e)));
            }
        };
        let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
        let mut stub = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
        // 载体专型标签：native_fn 槽的 Box 经 RegExp 同一守卫释放/深拷贝，
        // 避免每次 matchAll 泄漏一个已编译正则。
        stub.type_tag = oxide_types::object::JsObject::OBJ_TYPE_REGEX_STUB;
        let boxed = Box::new(compiled);
        let raw = Box::into_raw(boxed) as *const u8;
        stub.set_native_fn(Some(unsafe { oxide_types::object::NativeFnPtr::from_raw(raw as *const ()) }));
        let stub_ptr = vm.alloc_object(stub);
        // string = ToString(thisValue)：构造路径消费点，仅此臂实际转换。
        let input_val = match oxide_runtime_api::to_string_value_full(this_val, vm) {
            Ok(v) => v,
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        };
        builder_wrapper(vm, input_val, JsValue::from_js_object(stub_ptr))
    }
}

/// 构造路径（规范步骤 4-6）：string = ToString(thisValue) →
/// regexp = RegExpCreate(pattern, "g") → Invoke(regexp, @@matchAll, « string »)。
///
/// # 边界与前提
/// - pattern 为 null/undefined 时构造器内部映射（null → "null" 文本、
///   undefined → 空模式），不在此处重复转换
/// - 构造产物无 callable @@matchAll（含原型链删除）时 Invoke 抛 TypeError
/// - 对象 toString/valueOf 抛出的原始异常原样传播
fn match_all_construct_invoke<H: VmHost>(vm: &mut H, this_val: JsValue, pattern_val: JsValue) -> NativeResult {
    // string = ToString(thisValue)。
    let input_val = match oxide_runtime_api::to_string_value_full(this_val, vm) {
        Ok(v) => v,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };

    // regexp = RegExpCreate(pattern, "g")。
    let g_str = vm.new_string("g");
    let rx_val = match vm.construct_ctor(
        JsValue::from_js_object(vm.session().builtin_world().regexp_constructor.as_ptr() as *mut JsObject),
        &[pattern_val, g_str],
    ) {
        Ok(v) => v,
        Err(e) => return NativeResult::Err(e),
    };
    if !rx_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "match pattern constructor must return an object",
        ));
    }

    // Invoke(regexp, @@matchAll, « string »)：产物全链 Get，无 callable 抛 TypeError。
    let match_all_key = oxide_types::private_key::make_well_known_symbol_key(7);
    let rx_ptr = rx_val.as_js_object_ptr();
    let rx_this = JsValue::from_js_object(rx_ptr);
    let matcher = match vm.ordinary_get(unsafe { &*rx_ptr }, match_all_key, rx_this) {
        Ok(v) => v,
        Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
    };
    if matcher.is_undefined() || matcher.is_null() || !crate::iterator::is_callable(matcher) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "RegExp.prototype[Symbol.matchAll] is not a function",
        ));
    }
    match vm.call_function_sync(matcher, rx_this, &[input_val]) {
        Ok(v) => NativeResult::Ok(v),
        Err(e) => NativeResult::err(crate::iterator::engine_error(vm, &e)),
    }
}

/// 构造 matchAll 包装对象：原型挂 `%RegExpStringIteratorPrototype%`，
/// own 属性为 input/index(0)/re 三槽，next 由原型提供。
fn builder_wrapper<H: VmHost>(vm: &mut H, input_val: JsValue, re_obj: JsValue) -> NativeResult {
    // matchAll 迭代器挂 %RegExpStringIteratorPrototype%（链到 %IteratorPrototype%），
    // next 由原型提供（不挂实例 own）。
    let regexp_iter_proto = vm.session().builtin_world().regexp_string_iterator_proto.as_ptr() as *mut JsObject;
    let wrapper = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(regexp_iter_proto)));

    let wrapper_obj = unsafe { &mut *wrapper };
    let input_si = vm.kernel_core().perm_interner().intern(MALL_INPUT).0;
    let index_si = vm.kernel_core().perm_interner().intern(MALL_INDEX).0;
    let re_si = vm.kernel_core().perm_interner().intern(MALL_RE).0;

    vm.set_or_create_prop_value(wrapper_obj, input_si, input_val);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, re_si, re_obj);

    NativeResult::Ok(JsValue::from_js_object(wrapper))
}

/// `String.prototype[Symbol.iterator]()`：返回按码元迭代字符的迭代器。
///
/// # 步骤
/// 1. null/undefined 抛 TypeError（RequireObjectCoercible）
/// 2. this 经 ToString 完整转换（对象取 toString 结果，Symbol 抛 TypeError）
/// 3. 包成统一迭代器包装（next 逐码元产出，耗尽后 done）
///
/// # 边界与前提
/// - 对象 toString 抛出的原始异常原样传播
/// - 包装器挂 `%StringIteratorPrototype%` → `%IteratorPrototype%` 链
pub fn string_symbol_iterator<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if this_val.is_null() || this_val.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "String.prototype[Symbol.iterator] called on null or undefined",
        ));
    }
    let s_val = match oxide_runtime_api::to_string_value_full(this_val, vm) {
        Ok(v) => v,
        Err(_) => {
            // ToString 触发对象 toString/valueOf 抛出的原生异常须原样传播。
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    // 直构 String 迭代器：内层即 ToString 结果，不再回读 @@iterator。本函数语义
    // 只由 this 决定，属性表被删除/置 null/被覆盖均不影响已保存引用的直调。
    // 迭代器挂 %StringIteratorPrototype%（链到 %IteratorPrototype%）；串内层不暴露 return。
    let string_iter_proto = vm.session().builtin_world().string_iterator_proto.as_ptr() as *mut JsObject;
    NativeResult::Ok(crate::iterator::build_iterator_wrapper(vm, s_val, None, true, Some(string_iter_proto)))
}

/// `matchAll` 迭代器的 `next`：返回 `{value: 匹配数组, done}`，耗尽后 done 为 true。
/// 游标为码元位置（两臂统一口径）；空匹配推进 1 个码元。
pub fn string_match_all_next<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "matchAll next called on non-object"));
    }
    let wrapper = unsafe { &mut *this_val.as_js_object_ptr() };
    let input_si = vm.kernel_core().perm_interner().intern(MALL_INPUT).0;
    let index_si = vm.kernel_core().perm_interner().intern(MALL_INDEX).0;
    let re_si = vm.kernel_core().perm_interner().intern(MALL_RE).0;

    let input_val = match vm.ordinary_get(wrapper, input_si, this_val) {
        Ok(v) if v.is_string() => v,
        _ => return make_match_done_result(vm, JsValue::undefined()),
    };
    let idx_val = match vm.ordinary_get(wrapper, index_si, this_val) {
        Ok(v) => v,
        Err(_) => return make_match_done_result(vm, JsValue::undefined()),
    };
    let idx_raw = if idx_val.is_int() { idx_val.as_int().max(0) } else { 0 };
    let idx = idx_raw as usize;
    let re_val = match vm.ordinary_get(wrapper, re_si, this_val) {
        Ok(v) if v.is_object() => v,
        _ => return make_match_done_result(vm, JsValue::undefined()),
    };
    let re_obj = unsafe { &*re_val.as_js_object_ptr() };
    let fn_ptr = match re_obj.native_fn() {
        Some(p) => p,
        None => return make_match_done_result(vm, JsValue::undefined()),
    };
    let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
    // d 标志门控 indices 产出（flags 串实例字段直读，与 exec 同口径）。
    let has_indices = crate::regexp::regexp_has_flag(vm, re_obj, 'd');

    // 按 input 载荷形态分臂：Flat 走字节通道（游标经码元↔字节换算），
    // 其余走单元通道（游标即码元）。
    // SAFETY: input_val 已校验为字符串值，裸指针借用压缩到单个表达式。
    let sp = unsafe { &*input_val.as_string_ptr() };
    // 耗尽守卫：空匹配正则（如 /(?:)/g、/a*/g）在串尾会反复命中同一末端空
    // 匹配，游标推进越过码元总数后须直接 done，否则同一末端空匹配死循环。
    let total_units = sp.utf16_len() as usize;
    if idx > total_units {
        return make_match_done_result(vm, JsValue::undefined());
    }
    // 元素按值混排：匹配片段物化字符串值，未匹配捕获组为 undefined。
    let (match_start, next_idx, parts, m_opt): (usize, usize, Vec<JsValue>, Option<regress::Match>) = if sp.is_flat() {
        let s = sp.as_str();
        match regex.find_from(s, unit_to_byte(s, idx)).next() {
            Some(m) => {
                let range = m.range();
                let mut parts = Vec::with_capacity(m.captures.len() + 1);
                parts.push(vm.new_string_units_owned(s[range.start..range.end].encode_utf16().collect()));
                for i in 1..=m.captures.len() {
                    match m.group(i) {
                        Some(g) => parts.push(vm.new_string_units_owned(s[g.start..g.end].encode_utf16().collect())),
                        None => parts.push(JsValue::undefined()),
                    }
                }
                // 空匹配（range 无推进）须推进至少一个码元，否则同一位置反复
                // 空匹配死循环；非空匹配游标落匹配末尾（码元口径）。
                let start_units = byte_to_unit(s, range.start);
                let end_units = byte_to_unit(s, range.end);
                let next_idx = if range.end > range.start { end_units } else { end_units + 1 };
                (start_units, next_idx, parts, Some(m))
            }
            None => (0, 0, Vec::new(), None),
        }
    } else {
        let u = sp.units();
        match regex.find_from_utf16(&u, idx).next() {
            Some(m) => {
                let range = m.range();
                let mut parts = Vec::with_capacity(m.captures.len() + 1);
                parts.push(vm.new_string_units_owned(u[range.start..range.end].to_vec()));
                for i in 1..=m.captures.len() {
                    match m.group(i) {
                        Some(g) => parts.push(vm.new_string_units_owned(u[g.start..g.end].to_vec())),
                        None => parts.push(JsValue::undefined()),
                    }
                }
                let next_idx = if range.end > range.start { range.end } else { range.end + 1 };
                (range.start, next_idx, parts, Some(m))
            }
            None => (0, 0, Vec::new(), None),
        }
    };

    if parts.is_empty() {
        return make_match_done_result(vm, JsValue::undefined());
    }
    vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(next_idx as i32));
    // 构建结果数组：按元素混排字符串值/undefined + 挂 index/input/groups 属性。
    let match_text = MatchText::from_value(input_val);
    let value =
        make_match_result_array(vm, parts, match_start as i32, input_val, m_opt.as_ref(), &match_text, has_indices);
    make_match_done_result(vm, value)
}

/// 构建 matchAll 结果数组：元素为匹配字符串值（未匹配捕获组为 undefined），
/// 附带 index/input/groups 属性（d 标志下另挂 indices，与 exec 结果同面）。
fn make_match_result_array<H: VmHost>(
    vm: &mut H, parts: Vec<JsValue>, match_index: i32, input_val: JsValue, m: Option<&regress::Match>,
    text: &MatchText, has_indices: bool,
) -> JsValue {
    let arr = make_string_array_values(vm, parts);
    let arr_ptr = arr.as_js_object_ptr();
    let arr_obj = unsafe { &mut *arr_ptr };
    let index_si = vm.kernel_core().perm_interner().intern("index").0;
    vm.set_or_create_prop_value(arr_obj, index_si, JsValue::int(match_index));
    let input_si = vm.kernel_core().perm_interner().intern("input").0;
    vm.set_or_create_prop_value(arr_obj, input_si, input_val);
    let groups_val = match m {
        Some(m) => build_groups_object(vm, m, text),
        None => JsValue::undefined(),
    };
    let groups_si = vm.kernel_core().perm_interner().intern("groups").0;
    vm.set_or_create_prop_value(arr_obj, groups_si, groups_val);
    // d 标志：挂 indices 属性（与 exec 结果同口径的码元对数组族）。
    if has_indices {
        if let Some(m) = m {
            let indices_val = crate::regexp::build_indices_array(vm, m, text);
            let indices_si = vm.kernel_core().perm_interner().intern("indices").0;
            vm.set_or_create_prop_value(arr_obj, indices_si, indices_val);
        }
    }
    arr
}

/// 构造迭代器结果对象 `{value, done}`（done 仅在 value 为 undefined 时为 true）。
fn make_match_done_result<H: VmHost>(vm: &mut H, value: JsValue) -> NativeResult {
    let done = value.is_undefined();
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let obj = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
    let value_si = vm.kernel_core().perm_interner().intern("value").0;
    let done_si = vm.kernel_core().perm_interner().intern("done").0;
    let obj_ref = unsafe { &mut *obj };
    vm.set_or_create_prop_value(obj_ref, value_si, value);
    vm.set_or_create_prop_value(obj_ref, done_si, JsValue::bool(done));
    NativeResult::Ok(JsValue::from_js_object(obj))
}

/// `String.prototype.replaceAll(pattern, replacement)`：替换全部匹配
/// （RegExp 或字符串模式）。
pub fn string_replace_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.replaceAll called with {} args", args.len());
    string_replace_impl(vm, args, true)
}

/// `String.raw(template, ...substitutions)`：从模板对象的 `raw` 数组元素与
/// substitutions 交错拼接，返回原始字符串（单元口径，lone surrogate 保真）。
///
/// # 步骤
/// 1. 取 template 的 `raw` 属性（必须为对象，否则抛 TypeError）
/// 2. 取 raw 的 `length` 属性（ToLength）
/// 3. 逐下标读 raw[i] 拼接；仅在段间（i + 1 < length）追加 substitutions[i]
///
/// # 边界与前提
/// - raw[i] 缺失时视为 undefined（ToString 为 "undefined"）
/// - substitutions 缺失时段间追加空串，末段后不追加任何内容
pub fn string_raw<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.raw called with {} args", args.len());
    // Step 1: 取 template 参数（args[1] 为 this 区域后的首个实参）。
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "String.raw: template is required"));
    }
    let template_val = vm.reg(args[1]);
    if !template_val.is_object() || template_val.as_js_object_ptr().is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "String.raw: template must be an object"));
    }
    let template_ptr = template_val.as_js_object_ptr();

    // Step 2: 取 template.raw 属性。
    let raw_si = vm.kernel_core().perm_interner().intern("raw").0;
    let template_obj = unsafe { &*template_ptr };
    let raw_val = match vm.ordinary_get(template_obj, raw_si, template_val) {
        Ok(v) => v,
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    if !raw_val.is_object() || raw_val.as_js_object_ptr().is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "String.raw: template.raw must be an object"));
    }
    let raw_ptr = raw_val.as_js_object_ptr();

    // Step 3: 取 raw.length（ToLength，Symbol 抛 TypeError）。
    let length_si = vm.kernel_core().perm_interner().intern("length").0;
    let raw_obj = unsafe { &*raw_ptr };
    let raw_len = match vm.ordinary_get(raw_obj, length_si, raw_val) {
        Ok(v) => {
            // ToLength：经 to_number_full（Symbol 抛 TypeError / 对象 ToPrimitive 异常
            // 原样传播），再按 ToLength 归一并截断（防超大 raw 数组导致 OOM）。
            match oxide_runtime_api::to_number_full(v, vm) {
                Ok(n) => {
                    let len = if n.is_nan() || n <= 0.0 { 0.0 } else { n.min(9_007_199_254_740_991.0) };
                    (len.trunc() as u64).min(0x1FFFFF) as usize
                }
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    return NativeResult::Err(exc);
                }
            }
        }
        Err(e) => {
            let exc = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_type_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };

    // Step 4: 逐下标拼接 raw[i] + substitutions[i]。
    let mut result: Vec<u16> = Vec::new();
    let raw_obj_ref = unsafe { &*raw_ptr };
    for i in 0..raw_len {
        // 读 raw[i]：用 property_key_si 把整数 i 映射为整数键 si，
        // 再经 ordinary_get 沿原型链查找（触发 getter）。
        let index_key = vm.property_key_si(JsValue::int(i as i32));
        let raw_elem = vm.ordinary_get(raw_obj_ref, index_key, raw_val);
        let raw_units = match raw_elem {
            // ToString 完整路径：对象经 ToPrimitive（异常原样传播），Symbol 抛 TypeError。
            Ok(v) => match oxide_runtime_api::to_string_value_full(v, vm) {
                Ok(sv) => vm.string_units(sv).into_owned(),
                Err(msg) => {
                    let exc = vm
                        .take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                    return NativeResult::Err(exc);
                }
            },
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                "undefined".encode_utf16().collect()
            }
        };
        result.extend(raw_units);

        // 段间（i + 1 < length）才追加 substitutions[i]；substitutions 缺失时
        // 追加空串，末段后不再追加任何内容。
        if i + 1 < raw_len {
            let sub_idx = i + 2;
            if sub_idx < args.len() {
                let sub_val = vm.reg(args[sub_idx]);
                // ToString 完整路径：对象 ToPrimitive 异常原样传播，Symbol 抛 TypeError。
                let sub_units = match oxide_runtime_api::to_string_value_full(sub_val, vm) {
                    Ok(sv) => vm.string_units(sv).into_owned(),
                    Err(msg) => {
                        let exc = vm
                            .take_uncaught_value()
                            .unwrap_or_else(|| crate::error::create_type_error(vm, &msg));
                        return NativeResult::Err(exc);
                    }
                };
                result.extend(sub_units);
            }
        }
    }
    NativeResult::Ok(vm.new_string_units_owned(result))
}
