use std::borrow::Cow;

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
    this_text, this_units, unit_to_byte,
};

// ── 正则替换（单元口径） ────────────────────────────────────────────────

/// 展开单个匹配的 replacement `$` 引用（`$$`、`$&`、`` $` ``、`$'`、`$n`），
/// 输入/输出均为单元序列。
fn expand_dollar_units(text: &[u16], m: &regress::Match, replacement: &[u16]) -> Vec<u16> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < replacement.len() {
        if replacement[i] != 0x24 {
            out.push(replacement[i]);
            i += 1;
            continue;
        }
        let range = m.range();
        let rest = &replacement[i..];
        if rest.starts_with(&[0x24, 0x24]) {
            out.push(0x24);
            i += 2;
        } else if rest.starts_with(&[0x24, 0x26]) {
            out.extend_from_slice(&text[range.start..range.end]);
            i += 2;
        } else if rest.starts_with(&[0x24, 0x60]) {
            out.extend_from_slice(&text[..range.start]);
            i += 2;
        } else if rest.starts_with(&[0x24, 0x27]) {
            out.extend_from_slice(&text[range.end..]);
            i += 2;
        } else if i + 1 < replacement.len() && (0x30..=0x39).contains(&replacement[i + 1]) {
            // $digits：digitCount 取 2（后续尚有数字）否则 1；两位数值越界回退
            // 一位（次位留给后续按字面处理）；仍越界（含 $0）→ 整段 ref 字面。
            let digit_count = if i + 2 < replacement.len() && (0x30..=0x39).contains(&replacement[i + 2]) {
                2
            } else {
                1
            };
            let d1 = (replacement[i + 1] - 0x30) as u32;
            let mut index = if digit_count == 2 { d1 * 10 + (replacement[i + 2] - 0x30) as u32 } else { d1 };
            let mut ref_len = 1 + digit_count;
            let capture_len = (m.groups().count() - 1) as u32;
            if index > capture_len && digit_count == 2 {
                index = d1;
                ref_len = 2;
            }
            if (1..=capture_len).contains(&index) {
                if let Some(g) = m.group(index as usize) {
                    out.extend_from_slice(&text[g.start..g.end]);
                }
            } else {
                out.extend_from_slice(&replacement[i..i + ref_len]);
            }
            i += ref_len;
        } else {
            out.push(0x24);
            i += 1;
        }
    }
    out
}

/// 正则替换的手动实现（单元口径）：按 JS 语义展开 replacement 中的 `$` 引用，
/// global 全替换否则首个。
pub(crate) fn regex_replace_manual_units(
    regex: &regress::Regex, text: &[u16], replacement: &[u16], global: bool,
) -> Vec<u16> {
    let mut out = Vec::new();
    let mut last_end = 0;
    let matches: Vec<regress::Match> = if global {
        regex.find_from_utf16(text, 0).collect()
    } else {
        regex.find_from_utf16(text, 0).take(1).collect()
    };
    for m in &matches {
        let range = m.range();
        out.extend_from_slice(&text[last_end..range.start]);
        out.extend_from_slice(&expand_dollar_units(text, m, replacement));
        last_end = range.end;
    }
    out.extend_from_slice(&text[last_end..]);
    out
}

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

/// 函数 replacer 的回调参数：匹配串、各捕获组（未匹配为 undefined）、position
/// （码元口径）、原字符串。原字符串参数由调用方预构传入（`text_arg`），
/// 避免每匹配复制整个源串——字符串不可变，同一 `JsValue` 可安全复用。
fn replacer_cb_args<H: VmHost>(vm: &mut H, text: &[u16], m: &regress::Match, text_arg: JsValue) -> Vec<JsValue> {
    let range = m.range();
    let mut cb_args: Vec<JsValue> = Vec::with_capacity(m.captures.len() + 2);
    cb_args.push(vm.new_string_units(&text[range.start..range.end]));
    for i in 1..=m.captures.len() {
        match m.group(i) {
            Some(g) => cb_args.push(vm.new_string_units(&text[g.start..g.end])),
            None => cb_args.push(JsValue::undefined()),
        }
    }
    cb_args.push(JsValue::int(range.start as i32));
    cb_args.push(text_arg);
    cb_args
}

/// 正则模式 + 函数 replacer：global 全替换否则替换首个，逐匹配调用回调，
/// 返回值转单元序列作为替换文本（不展开 `$` 引用）。`text_arg` 为回调第 4 参
/// （原字符串），调用方预构一次，回调期按值复用。
pub(crate) fn regex_replace_fn<H: VmHost>(
    vm: &mut H, regex: &regress::Regex, text: &[u16], replacer: JsValue, global: bool, text_arg: JsValue,
) -> NativeResult {
    let matches: Vec<regress::Match> = if global {
        regex.find_from_utf16(text, 0).collect()
    } else {
        regex.find_from_utf16(text, 0).take(1).collect()
    };
    let mut out = Vec::new();
    let mut last_end = 0;
    for m in &matches {
        let range = m.range();
        out.extend_from_slice(&text[last_end..range.start]);
        let cb_args = replacer_cb_args(vm, text, m, text_arg);
        let repl = try_string!(call_replacer(vm, replacer, &cb_args));
        out.extend_from_slice(&repl);
        last_end = range.end;
    }
    out.extend_from_slice(&text[last_end..]);
    NativeResult::Ok(vm.new_string_units_owned(out))
}

/// 单元序列上的整体替换（空模式在每个单元边界插入替换文本，与 String::replace
/// 口径一致）。
fn replace_units_all(hay: &[u16], from: &[u16], to: &[u16]) -> Vec<u16> {
    if from.is_empty() {
        let mut out = Vec::with_capacity(hay.len() + (hay.len() + 1) * to.len());
        out.extend_from_slice(to);
        for &u in hay {
            out.push(u);
            out.extend_from_slice(to);
        }
        return out;
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut start = 0;
    while let Some(p) = find_units(hay, from, start) {
        out.extend_from_slice(&hay[start..p]);
        out.extend_from_slice(to);
        start = p + from.len();
    }
    out.extend_from_slice(&hay[start..]);
    out
}

/// 单元序列上的首个替换；未命中返回原序列拷贝。空模式在位置 0 命中
/// （插入替换文本于串首，与 String::replace 口径一致）。
fn replace_units_first(hay: &[u16], from: &[u16], to: &[u16]) -> Vec<u16> {
    if from.is_empty() {
        let mut out = Vec::with_capacity(hay.len() + to.len());
        out.extend_from_slice(to);
        out.extend_from_slice(hay);
        return out;
    }
    if let Some(rel) = find_units(hay, from, 0) {
        let mut out = Vec::with_capacity(hay.len());
        out.extend_from_slice(&hay[..rel]);
        out.extend_from_slice(to);
        out.extend_from_slice(&hay[rel + from.len()..]);
        return out;
    }
    hay.to_vec()
}

/// 字符串模式 + 函数 replacer：all 全替换否则替换首个。回调参数
/// `(match, position, string)`（无捕获组）。空模式在每个单元边界匹配一次。
/// `text_arg` 为回调第 4 参（原字符串），调用方预构一次。
fn string_replace_fn<H: VmHost>(
    vm: &mut H, text: &[u16], pattern: &[u16], replacer: JsValue, all: bool, text_arg: JsValue,
) -> NativeResult {
    let search_length = pattern.len();
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
        while let Some(p) = find_units(text, pattern, start) {
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
        let cb_args = [vm.new_string_units(&text[p..p + search_length]), JsValue::int(p as i32), text_arg];
        let repl = try_string!(call_replacer(vm, replacer, &cb_args));
        out.extend_from_slice(&repl);
        last_end = p + search_length;
    }
    out.extend_from_slice(&text[last_end..]);
    NativeResult::Ok(vm.new_string_units_owned(out))
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
// ── 拆分 / 正则匹配 ─────────────────────────────────────────────────────

/// `String.prototype.split(separator, limit)`：按分隔符拆分为字符串数组；
/// 分隔符可为 RegExp（含捕获组）或字符串。空分隔按单元逐个产出（规格口径，
/// 孤立 surrogate 为 1 单元元素）。
pub fn string_split<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.split called with {} args", args.len());
    // 参数转换先行：separator 为对象时 ToString 可能触发用户代码（&mut 路径），
    // limit 仅纯函数计算；之后借 this 源串扫描切分。
    let sep_val = if args.len() >= 2 { Some(vm.reg(args[1])) } else { None };
    let is_undefined_sep = sep_val.map(|v| v.is_undefined()).unwrap_or(true);
    let is_re = match sep_val {
        Some(v) if !v.is_undefined() => is_regexp_obj(v, vm),
        _ => false,
    };
    // 类正则对象是否持有编译正则：native_fn 存在才是真 RegExp 实例，命中正则切分路径；
    // 无编译正则的类正则对象（如 Object.create(RegExp.prototype)）回退字符串路径，
    // 须按 ToString 文本切分，且转换（&mut 路径）须先于 this 借用完成。
    let has_native_re = is_re && {
        let sep = sep_val.expect("separator present when is_re is true");
        let re_ptr = sep.as_js_object_ptr();
        // SAFETY: is_re 已保证 sep_val 为非空对象且 proto 恒等 RegExp.prototype。
        let re = unsafe { &*re_ptr };
        re.native_fn().is_some()
    };
    // ToUint32(limit)，缺省为 2^32-1。
    let limit = if args.len() > 2 {
        let l = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2]));
        if l.is_infinite() {
            u32::MAX as usize
        } else {
            (l.max(0.0).trunc() as u64).min(u32::MAX as u64) as usize
        }
    } else {
        u32::MAX as usize
    };
    let sep_units: Vec<u16> = if is_undefined_sep || has_native_re {
        Vec::new()
    } else {
        let sep = sep_val.expect("separator present when not undefined and not native regexp");
        try_string!(as_units(vm, sep)).into_owned()
    };
    let s: Vec<u16> = try_string!(this_units(vm, args)).into_owned();
    // 规范：separator 为 undefined 时返回 [this]。
    if is_undefined_sep {
        return NativeResult::Ok(make_units_array(vm, vec![s.to_vec()]));
    }
    if is_re {
        let re_ptr = sep_val.unwrap().as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        if let Some(fn_ptr) = re.native_fn() {
            let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
            let mut parts: Vec<Vec<u16>> = Vec::new();
            let mut last_end = 0;
            for m in regex.find_from_utf16(&s, 0) {
                if parts.len() >= limit {
                    break;
                }
                let range = m.range();
                parts.push(s[last_end..range.start].to_vec());
                if parts.len() >= limit {
                    break;
                }
                for i in 1..=m.captures.len() {
                    if parts.len() >= limit {
                        break;
                    }
                    match m.group(i) {
                        Some(g) => parts.push(s[g.start..g.end].to_vec()),
                        None => parts.push(Vec::new()),
                    }
                }
                last_end = range.end;
            }
            if last_end <= s.len() && parts.len() < limit {
                parts.push(s[last_end..].to_vec());
            }
            return NativeResult::Ok(make_units_array(vm, parts));
        }
        // 无原生正则的类正则对象回退到字符串路径。
    }
    if sep_units.is_empty() {
        // 每单元一个元素（ASCII 走单字符缓存零分配，其余 1 单元串创建）。
        let mut values = Vec::with_capacity(s.len().min(limit));
        for &u in s.iter().take(limit) {
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
        if parts.len() >= limit {
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

/// `replace`/`replaceAll` 共用的分叉实现：按 replacer 类型与参数形态选择路径。
///
/// # 步骤
/// 1. receiver 前置校验（RequireObjectCoercible，纯 is_* 读取）。
/// 2. 纯对象头读取判定分叉：正则身份/编译正则/global 标志/函数 replacer。
/// 3. 按分支执行替换（单元口径；良形内容的结果与旧文本路径逐位一致）。
///
/// # 边界与前提
/// - 缺省 searchValue/replaceValue 均按 ToString(undefined)="undefined" 处理
///   （`s.replace()` 全缺省等价于把 "undefined" 替换为 "undefined"，结果与
///   原串一致；`s.replace("b")` 得到 "aundefinedc" 而非 "ac"）。
/// - replaceAll 遇非 global 正则抛 TypeError；类正则对象（proto 恒等
///   RegExp.prototype 但无编译正则）replace/replaceAll 统一按 ToString 文本走
///   字符串路径。
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

    // 分叉判定（全部纯读取/纯对象头读，无用户代码）：正则对象身份、是否持有
    // 编译正则、global 标志、replacer 是否为函数。
    let is_re = is_regexp_obj(pattern_val, vm);
    let (has_native_re, is_global) = if is_re {
        let re_ptr = pattern_val.as_js_object_ptr();
        // SAFETY: is_re 已保证 pattern_val 为非空对象且 proto 恒等 RegExp.prototype。
        let re = unsafe { &*re_ptr };
        if re.native_fn().is_some() {
            let g = crate::regexp::regexp_has_flag(vm, re, 'g');
            (true, g)
        } else {
            (false, false)
        }
    } else {
        (false, false)
    };
    let replacer_val = if replacement_val.is_object() {
        let o = unsafe { &*replacement_val.as_js_object_ptr() };
        if o.is_function() {
            Some(replacement_val)
        } else {
            None
        }
    } else {
        None
    };

    // replaceAll 要求正则带 global：非 global 正则直接抛 TypeError（规范 flags 检查）。
    // 类正则对象（无编译正则）无 flags 概念，统一走 as_units 文本路径，不在此检查。
    if all && has_native_re && !is_global {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "String.prototype.replaceAll called with a non-global RegExp",
        ));
    }

    // 分支 A：函数 replacer。回调经 call_function_sync（&mut），receiver 单元
    // 序列先落地为 owned（跨回调的硬约束）。
    if let Some(replacer_val) = replacer_val {
        let s = try_string!(this_units(vm, args)).into_owned();
        // 回调第 4 参（原字符串）预构一次：原始字符串 receiver 直接复用 this_val
        // 零拷贝，对象 receiver 提升为单次转换值（原每匹配整串复制）。
        let text_arg = match this_val.is_string() {
            true => this_val,
            false => match oxide_runtime_api::to_string_value_full(this_val, vm) {
                Ok(v) => v,
                Err(_) => {
                    // ToString 触发对象 toString/valueOf 抛出的原生异常须原样传播。
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
                }
            },
        };
        if has_native_re {
            let re_ptr = pattern_val.as_js_object_ptr();
            let re = unsafe { &*re_ptr };
            let fn_ptr = match re.native_fn() {
                Some(p) => p,
                None => return NativeResult::Err(crate::error::create_type_error(vm, "expected compiled regexp")),
            };
            // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
            let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
            return regex_replace_fn(vm, regex, &s, replacer_val, is_global, text_arg);
        }
        let pattern = try_string!(as_units(vm, pattern_val)).into_owned();
        return string_replace_fn(vm, &s, &pattern, replacer_val, all, text_arg);
    }

    let replacement_is_string = args.len() <= 2 || replacement_val.is_string();

    // 分支 B：全原始字符串快路径——三值按载荷形态共享借用（`&H` 共享借用
    // 派生多个单元借用可共存，E0499 只发生在 `&mut` 派生）。
    if this_val.is_string() && pattern_val.is_string() && replacement_is_string {
        let h = &*vm;
        let s = h.string_units(this_val);
        let p = h.string_units(pattern_val);
        let r: Cow<'_, [u16]> = if replacement_val.is_string() {
            h.string_units(replacement_val)
        } else {
            Cow::Owned("undefined".encode_utf16().collect())
        };
        let result = if all {
            replace_units_all(&s, &p, &r)
        } else {
            replace_units_first(&s, &p, &r)
        };
        return NativeResult::Ok(vm.new_string_units_owned(result));
    }

    // 分支 B2：正则 pattern 快路径——regex 为对象内裸指针（不占 vm 借用），
    // receiver/replacement 共享借用零拷贝。
    if this_val.is_string() && has_native_re && replacement_is_string {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Err(crate::error::create_type_error(vm, "expected compiled regexp")),
        };
        // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
        let h = &*vm;
        let s = h.string_units(this_val);
        let r: Cow<'_, [u16]> = if replacement_val.is_string() {
            h.string_units(replacement_val)
        } else {
            Cow::Owned("undefined".encode_utf16().collect())
        };
        let result = regex_replace_manual_units(regex, &s, &r, is_global);
        return NativeResult::Ok(vm.new_string_units_owned(result));
    }

    // 分支 C：一般路径（对象参与转换）。原始字符串 receiver 与对象 receiver
    // 统一：参数转换前置（&mut 路径）后按单元序列扫描。
    let s = try_string!(this_units(vm, args)).into_owned();
    let pattern = if has_native_re {
        Vec::new()
    } else if args.len() < 2 {
        "undefined".encode_utf16().collect()
    } else {
        try_string!(as_units(vm, pattern_val)).into_owned()
    };
    let replacement = if args.len() > 2 {
        try_string!(as_units(vm, replacement_val)).into_owned()
    } else {
        "undefined".encode_utf16().collect()
    };

    // 统一扫描：命中编译正则走手动 $ 展开，否则字符串子串替换。
    if has_native_re {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Err(crate::error::create_type_error(vm, "expected compiled regexp")),
        };
        // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
        let result = regex_replace_manual_units(regex, &s, &replacement, is_global);
        return NativeResult::Ok(vm.new_string_units_owned(result));
    }
    let result = if all {
        replace_units_all(&s, &pattern, &replacement)
    } else {
        replace_units_first(&s, &pattern, &replacement)
    };
    NativeResult::Ok(vm.new_string_units_owned(result))
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
    let pattern: Vec<u16> = match pattern_val {
        Some(v) if !is_re => try_string!(as_units(vm, v)).into_owned(),
        _ => Vec::new(),
    };
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
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::null());
    }
    let pattern_val = match pattern_val {
        Some(v) => v,
        None => return NativeResult::Err(crate::error::create_type_error(vm, "expected pattern")),
    };
    if is_re {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(JsValue::null()),
        };
        // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
        if is_global {
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
        if let Some(m) = text.find_from_units(regex, 0) {
            let range = m.range();
            let mut parts: Vec<Vec<u16>> = Vec::with_capacity(m.captures.len() + 1);
            parts.push(text.slice(range.start, range.end).into_owned());
            for i in 1..=m.captures.len() {
                match m.group(i) {
                    Some(g) => parts.push(text.slice(g.start, g.end).into_owned()),
                    None => parts.push(Vec::new()),
                }
            }
            return NativeResult::Ok(make_units_array(vm, parts));
        }
        return NativeResult::Ok(JsValue::null());
    }
    // 空模式命中串头：产出单元素空串数组（规格口径）。
    if pattern.is_empty() {
        return NativeResult::Ok(make_units_array(vm, vec![Vec::new()]));
    }
    let s = text.units();
    if let Some(pos) = find_units(&s, &pattern, 0) {
        let matched = s[pos..pos + pattern.len()].to_vec();
        return NativeResult::Ok(make_units_array(vm, vec![matched]));
    }
    NativeResult::Ok(JsValue::null())
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
pub fn string_match_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.matchAll called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    if this_val.is_null() || this_val.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "String.prototype method called on null or undefined",
        ));
    }
    if this_val.is_symbol() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
    }
    let input_val = match oxide_runtime_api::to_string_value_full(this_val, vm) {
        Ok(v) => v,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    if args.len() < 2 {
        builtins_error!("String.prototype.matchAll: invalid receiver");
        return NativeResult::Err(JsValue::undefined());
    }
    let pattern_val = vm.reg(args[1]);

    // 规范步骤 2-5：pattern 为 null/undefined 时跳过 IsRegExp 分支，
    // 直接 RegExpCreate(this, "g") + Invoke(rx, @@matchAll, « S »)。
    if pattern_val.is_null() || pattern_val.is_undefined() {
        // rx = new RegExp(ToString(this), "g")。
        let g_str = vm.new_string("g");
        let rx_val = match vm.construct_ctor(
            JsValue::from_js_object(vm.session().builtin_world().regexp_constructor.as_ptr() as *mut JsObject),
            &[input_val, g_str],
        ) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        };
        // Invoke(rx, @@matchAll, « S »)。
        let match_all_key = oxide_types::private_key::make_well_known_symbol_key(7);
        let rx_ptr = rx_val.as_js_object_ptr();
        let proto_val = unsafe { &*rx_ptr }.proto();
        let rx_this = JsValue::from_js_object(rx_ptr);
        let matcher = match vm.ordinary_get(unsafe { &*rx_ptr }, match_all_key, rx_this) {
            Ok(v) => v,
            Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
        };
        if matcher.is_undefined() || matcher.is_null() {
            // 查原型链。
            if proto_val.is_object() {
                let p_ptr = proto_val.as_js_object_ptr();
                let p_this = proto_val;
                let proto_matcher = match vm.ordinary_get(unsafe { &*p_ptr }, match_all_key, p_this) {
                    Ok(v) => v,
                    Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
                };
                if crate::iterator::is_callable(proto_matcher) {
                    return match vm.call_function_sync(proto_matcher, rx_this, &[input_val]) {
                        Ok(v) => NativeResult::Ok(v),
                        Err(e) => NativeResult::err(crate::iterator::engine_error(vm, &e)),
                    };
                }
            }
            // 无 callable @@matchAll：构建包装器。
            return builder_wrapper(vm, input_val, rx_this);
        }
        if crate::iterator::is_callable(matcher) {
            return match vm.call_function_sync(matcher, rx_this, &[input_val]) {
                Ok(v) => NativeResult::Ok(v),
                Err(e) => NativeResult::err(crate::iterator::engine_error(vm, &e)),
            };
        }
        return builder_wrapper(vm, input_val, rx_this);
    }

    // 规范：IsRegExp(pattern) 时先检查 g 标志（Node.js 先行校验，早于
    // GetMethod 调用），不满足则抛 TypeError；满足后查 GetMethod(pattern,
    // @@matchAll)，可调用则直调，否则构建包装器。
    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        // 规范步骤 3.a-c：Get(rx, "flags") → RequireObjectCoercible →
        // ToString 含 "g" 判定；getter 抛错传播。
        let flags = match crate::regexp::rx_get_flags(vm, re_ptr, pattern_val) {
            Ok(f) => f,
            Err(e) => return NativeResult::Err(e),
        };
        // RequireObjectCoercible：flags 为 undefined/null 时抛 TypeError。
        // rx_get_flags 已将 undefined 转为 "undefined" 字符串，需额外检查。
        let flags_val =
            match vm.ordinary_get(unsafe { &*re_ptr }, vm.kernel_core().perm_interner().intern("flags").0, pattern_val)
            {
                Ok(v) => v,
                Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
            };
        if flags_val.is_undefined() || flags_val.is_null() {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "String.prototype.matchAll: regex must have global flag",
            ));
        }
        if !flags.contains('g') {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "String.prototype.matchAll: regex must have global flag",
            ));
        }
        // 标志检查通过：查 GetMethod(pattern, @@matchAll)。
        let match_all_key = oxide_types::private_key::make_well_known_symbol_key(7);
        let re_obj = unsafe { &*re_ptr };
        // 先查实例 own。
        let own_match_all = match vm.get_own_property_slot(re_obj, match_all_key) {
            Some(_) => match vm.ordinary_get(re_obj, match_all_key, pattern_val) {
                Ok(v) => v,
                Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
            },
            None => JsValue::undefined(),
        };
        if crate::iterator::is_callable(own_match_all) {
            return match vm.call_function_sync(own_match_all, pattern_val, &[input_val]) {
                Ok(v) => NativeResult::Ok(v),
                Err(e) => NativeResult::err(crate::iterator::engine_error(vm, &e)),
            };
        }
        // 实例 @@matchAll 不存在/undefined/null：回退原型链（GetMethod 对 null/undefined 均返 undefined）。
        if own_match_all.is_undefined() || own_match_all.is_null() {
            let proto_val = re_obj.proto();
            if proto_val.is_object() {
                let proto_ptr = proto_val.as_js_object_ptr();
                let proto_match_all = match vm.ordinary_get(unsafe { &*proto_ptr }, match_all_key, proto_val) {
                    Ok(v) => v,
                    Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
                };
                if crate::iterator::is_callable(proto_match_all) {
                    return match vm.call_function_sync(proto_match_all, pattern_val, &[input_val]) {
                        Ok(v) => NativeResult::Ok(v),
                        Err(e) => NativeResult::err(crate::iterator::engine_error(vm, &e)),
                    };
                }
                // 原型链也无 callable @@matchAll：规范 Invoke 抛 TypeError。
                return NativeResult::Err(crate::error::create_type_error(
                    vm,
                    "RegExp.prototype[Symbol.matchAll] is not a function",
                ));
            }
        }
        // 无 callable @@matchAll：规范 Invoke 抛 TypeError。
        NativeResult::Err(crate::error::create_type_error(vm, "RegExp.prototype[Symbol.matchAll] is not a function"))
    } else {
        // 非 RegExp：先查 @@matchAll（规范步骤 6-8，IsRegExp 为 false 时
        // 先 ToString(pattern) 转正则，但规范步骤 8 是 Invoke(rx, @@matchAll)。
        // 实际上，若 pattern 是对象且有 callable @@matchAll，应直调（类似 RegExp 分支）。
        if pattern_val.is_object() {
            let match_all_key = oxide_types::private_key::make_well_known_symbol_key(7);
            let p_ptr = pattern_val.as_js_object_ptr();
            let p_this = pattern_val;
            let matcher = match vm.ordinary_get(unsafe { &*p_ptr }, match_all_key, p_this) {
                Ok(v) => v,
                Err(e) => return NativeResult::err(crate::iterator::engine_error(vm, &e)),
            };
            if crate::iterator::is_callable(matcher) {
                return match vm.call_function_sync(matcher, p_this, &[input_val]) {
                    Ok(v) => NativeResult::Ok(v),
                    Err(e) => NativeResult::err(crate::iterator::engine_error(vm, &e)),
                };
            }
        }
        // 按文本编译为 stub（非捕获组，免额外 capture）。
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
        builder_wrapper(vm, input_val, JsValue::from_js_object(stub_ptr))
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
    let (match_start, next_idx, parts, m_opt): (usize, usize, Vec<Vec<u16>>, Option<regress::Match>) = if sp.is_flat() {
        let s = sp.as_str();
        match regex.find_from(s, unit_to_byte(s, idx)).next() {
            Some(m) => {
                let range = m.range();
                let mut parts = Vec::with_capacity(m.captures.len() + 1);
                parts.push(s[range.start..range.end].encode_utf16().collect());
                for i in 1..=m.captures.len() {
                    match m.group(i) {
                        Some(g) => parts.push(s[g.start..g.end].encode_utf16().collect()),
                        None => parts.push(Vec::new()),
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
                parts.push(u[range.start..range.end].to_vec());
                for i in 1..=m.captures.len() {
                    match m.group(i) {
                        Some(g) => parts.push(u[g.start..g.end].to_vec()),
                        None => parts.push(Vec::new()),
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
    // 构建结果数组：按元素物化字符串值 + 挂 index/input/groups 属性。
    let match_text = MatchText::from_value(input_val);
    let value = make_match_result_array(vm, parts, match_start as i32, input_val, m_opt.as_ref(), &match_text);
    make_match_done_result(vm, value)
}

/// 构建 matchAll 结果数组：元素为匹配字符串值，附带 index/input/groups 属性。
fn make_match_result_array<H: VmHost>(
    vm: &mut H, parts: Vec<Vec<u16>>, match_index: i32, input_val: JsValue, m: Option<&regress::Match>,
    text: &MatchText,
) -> JsValue {
    let values: Vec<JsValue> = parts.into_iter().map(|u| vm.new_string_units_owned(u)).collect();
    let arr = make_string_array_values(vm, values);
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
