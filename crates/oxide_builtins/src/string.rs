use std::borrow::Cow;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use memchr::memchr;
use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

macro_rules! try_string {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(e),
        }
    };
}

/// 取 this 的字符串内容供只读扫描：原始字符串零拷贝借用，对象经完整 ToString。
/// null/undefined/symbol 抛 TypeError；对象 ToString 抛出的原生异常原样传播。
///
/// # 注意事项
/// 返回借用绑定本次 `&mut` 借用，存活期内禁止任何 `&mut` 调用；需与参数字符串
/// 并存时，调用方先把参数转换为 owned（见 `as_string` 借用纪律）。
fn this_string<'a, H: VmHost>(vm: &'a mut H, args: &[u8]) -> Result<Cow<'a, str>, JsValue> {
    let this_val = vm.reg(args[0]);
    if this_val.is_null() || this_val.is_undefined() {
        return Err(crate::error::create_type_error(vm, "String.prototype method called on null or undefined"));
    }
    if this_val.is_symbol() {
        return Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"));
    }
    if this_val.is_string() {
        return Ok(Cow::Borrowed(vm.string_ref(this_val)));
    }
    // 对象 this 须经 ToString 完整转换（boxed Number/String 等取内部原始值）。
    match oxide_runtime_api::to_string_full(this_val, vm) {
        Ok(s) => Ok(Cow::Owned(s)),
        Err(_) => {
            // ToString 触发对象 toString/valueOf 抛出的原生异常须原样传播，
            // 否则会被展平为普通 Error 丢失原始异常对象。
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            Err(crate::error::create_type_error(vm, "Cannot convert this value to a string"))
        }
    }
}

/// `String.fromCharCode(...codes)`：把各参数按低 16 位转成字符拼接为字符串。
pub fn string_from_char_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.fromCharCode called with {} args", args.len());
    let mut out = String::new();
    for &arg_reg in args.iter().skip(1) {
        let code = oxide_runtime_api::to_uint32(vm.reg(arg_reg)) & 0xFFFF;
        if let Some(ch) = char::from_u32(code) {
            out.push(ch);
        } else {
            out.push('\u{FFFD}');
        }
    }
    NativeResult::Ok(vm.new_string_owned(out))
}

/// `String.fromCodePoint(...codes)`：把各参数按 ToNumber 语义转成 code point
/// （0..0x10FFFF 的整数）拼接为字符串；非整数、NaN 或越界抛 RangeError，
/// Symbol 抛 TypeError。surrogate 区间按现行规范接受（孤立 surrogate 无法在
/// UTF-8 表示，与字面量编码一致输出 U+FFFD + 四位小写 hex 文本）。
pub fn string_from_code_point<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.fromCodePoint called with {} args", args.len());
    let mut out = String::new();
    // 经 native_arg_count/native_arg_at 读取：大实参集（如 harness 的
    // `String.fromCodePoint.apply(null, codePoints)` 每块 10000 码位）在寄存器
    // 窗口外走 spill 溢出区，小实参集与既有寄存器路径一致。
    for i in 0..vm.native_arg_count(args) {
        let n = match oxide_runtime_api::to_number_full(vm.native_arg_at(args, i), vm) {
            Ok(n) => n,
            Err(_) => {
                // ToNumber 触发对象 valueOf/toString 抛出的原生异常须原样传播，
                // 否则会被展平为普通 Error 丢失原始异常对象。
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
            }
        };
        if !n.is_finite() || n.trunc() != n || n < 0.0 || n > 0x10FFFF as f64 {
            return NativeResult::Err(crate::error::create_range_error(vm, "Invalid code point"));
        }
        let code = n as u32;
        if (0xD800..=0xDFFF).contains(&code) {
            out.push('\u{FFFD}');
            out.push_str(&format!("{code:04x}"));
        } else {
            out.push(char::from_u32(code).unwrap());
        }
    }
    NativeResult::Ok(vm.new_string_owned(out))
}

/// `String.prototype.valueOf`：返回包装对象的原始字符串；其它 this 抛 TypeError。
pub fn string_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.valueOf called with {} args", args.len());
    let this_val = vm.reg(args[0]);
    if this_val.is_string() {
        return NativeResult::Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_string_obj() {
                return NativeResult::Ok(obj.get_prop_at(0));
            }
        }
    }
    builtins_error!("String.prototype.valueOf: invalid receiver");
    NativeResult::Err(crate::error::create_type_error(
        vm,
        "String.prototype.valueOf called on non-String object",
    ))
}

/// JS `String()` 构造逻辑：把参数转成字符串；new 语义返回 `[[StringData]]` 包装对象。
pub fn string_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let s = if args.len() > 1 {
        // 对象参数须经 ToPrimitive/ToString 完整转换（数组 → join，对象 → toString）。
        match oxide_runtime_api::to_string_for_string_constructor(vm.reg(args[1]), vm) {
            Ok(s) => s,
            Err(_) => {
                // ToString on an object may throw via toString/valueOf; propagate the original exception.
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        }
    } else {
        String::new()
    };
    let str_val = vm.new_string_owned(s);

    let string_proto = vm.session().builtin_world().string_proto.as_ptr() as *mut JsObject;
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let is_ctor = if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if ptr.is_null() {
            false
        } else {
            let proto_ptr = unsafe { (*ptr).proto().as_js_object_ptr() };
            !proto_ptr.is_null() && std::ptr::eq(proto_ptr, string_proto)
        }
    } else {
        false
    };

    if !is_ctor {
        return NativeResult::Ok(str_val);
    }

    let obj = unsafe { &mut *this_val.as_js_object_ptr() };
    obj.type_tag = JsObject::OBJ_TYPE_STRING_OBJ;
    obj.push_prop(str_val);
    NativeResult::Ok(this_val)
}

/// `String.prototype.toString`：返回包装对象的原始字符串；其它 this 抛 TypeError。
pub fn string_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if this_val.is_string() {
        return NativeResult::Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_string_obj() {
                return NativeResult::Ok(obj.get_prop_at(0));
            }
        }
    }
    NativeResult::Err(crate::error::create_type_error(
        vm,
        "String.prototype.toString called on non-String object",
    ))
}

pub(crate) fn make_string_array<H: VmHost>(vm: &mut H, parts: Vec<String>) -> JsValue {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let n = parts.len();
    let arr =
        vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto), n, vm.epoch().bump()));
    unsafe {
        for (i, s) in parts.into_iter().enumerate() {
            let sv = vm.new_string_owned(s);
            (*arr).set_prop_at(i, sv);
        }
        (*arr).set_prop_count(n);
    }
    JsValue::from_js_object(arr)
}

/// 以已构造的字符串值构建字符串数组（元素零拷贝落地），供逐字符产出路径
/// （空分隔 split）复用，跳过 `Vec<String>` 中间层。
pub(crate) fn make_string_array_values<H: VmHost>(vm: &mut H, parts: Vec<JsValue>) -> JsValue {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let n = parts.len();
    let arr =
        vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto), n, vm.epoch().bump()));
    unsafe {
        for (i, sv) in parts.into_iter().enumerate() {
            (*arr).set_prop_at(i, sv);
        }
        (*arr).set_prop_count(n);
    }
    JsValue::from_js_object(arr)
}

/// 取参数字符串内容：原始字符串零拷贝借用，其余经完整 ToString（对象 ToPrimitive）。
/// Symbol 与其余转换失败按规范抛 TypeError；对象 ToString 抛出的原生异常原样传播。
///
/// # 边界与前提
/// - 失败返回异常值，调用方经 `try_string!` 原样抛出。
///
/// # 注意事项
/// 返回借用绑定本次 `&mut` 借用；需与 `this_string` 借用并存时先 `into_owned` 落地。
fn as_string<'a, H: VmHost>(vm: &'a mut H, val: JsValue) -> Result<Cow<'a, str>, JsValue> {
    if val.is_string() {
        return Ok(Cow::Borrowed(vm.string_ref(val)));
    }
    match oxide_runtime_api::to_string_full(val, vm) {
        Ok(s) => Ok(Cow::Owned(s)),
        Err(_) => {
            // ToString 触发对象 toString/valueOf 抛出的原生异常须原样传播，
            // 否则会被展平为普通 Error 丢失原始异常对象。
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a string"))
        }
    }
}

/// 正则替换的手动实现：按 JS 语义展开 replacement 中的 `$` 引用
/// （`$$`、`$&`、`` $` ``、`$'`、`$n`），global 全替换否则首个。
pub(crate) fn regex_replace_manual(regex: &regress::Regex, text: &str, replacement: &str, global: bool) -> String {
    let mut out = String::new();
    let mut last_end = 0;
    let matches: Vec<regress::Match> = if global {
        regex.find_iter(text).collect()
    } else {
        regex.find(text).into_iter().collect()
    };
    for m in matches {
        let range = m.range();
        out.push_str(&text[last_end..range.start]);
        out.push_str(&expand_dollar(text, &m, replacement));
        last_end = range.end;
    }
    out.push_str(&text[last_end..]);
    out
}

/// 展开单个匹配的 replacement `$` 引用。
fn expand_dollar(text: &str, m: &regress::Match, replacement: &str) -> String {
    let mut out = String::new();
    let bytes = replacement.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            out.push(replacement[i..].chars().next().unwrap_or('\u{FFFD}'));
            i += 1;
            continue;
        }
        let range = m.range();
        let rest = &replacement[i..];
        if rest.starts_with("$$") {
            out.push('$');
            i += 2;
        } else if rest.starts_with("$&") {
            out.push_str(&text[range.start..range.end]);
            i += 2;
        } else if rest.starts_with("$`") {
            out.push_str(&text[..range.start]);
            i += 2;
        } else if rest.starts_with("$'") {
            out.push_str(&text[range.end..]);
            i += 2;
        } else {
            // $n（1-2 位数字）→ 捕获组；未匹配/越界 → 空串。
            let mut j = 1;
            while j < bytes.len().saturating_sub(i) && bytes[i + j].is_ascii_digit() {
                j += 1;
            }
            let digits = &replacement[i + 1..i + j];
            if digits.is_empty() {
                out.push('$');
                i += 1;
            } else {
                let n: usize = digits.parse().unwrap_or(0);
                if let Some(g) = m.group(n) {
                    out.push_str(&text[g.start..g.end]);
                }
                i += 1 + digits.len();
            }
        }
    }
    out
}

/// 调用函数 replacer 并把返回值 ToString 为替换文本；调用抛出的异常原样恢复。
fn call_replacer<H: VmHost>(vm: &mut H, replacer: JsValue, cb_args: &[JsValue]) -> Result<String, JsValue> {
    match vm.call_function_sync(replacer, JsValue::undefined(), cb_args) {
        Ok(result) => Ok(oxide_runtime_api::to_string(result)),
        Err(err) => Err(vm
            .take_uncaught_value()
            .unwrap_or_else(|| crate::error::create_type_error(vm, &format!("replace replacer: {}", err)))),
    }
}

/// 函数 replacer 的回调参数：匹配串、各捕获组（未匹配为 undefined）、position、原字符串。
/// position 按 UTF-16 单元计数（非字节偏移）。原字符串参数由调用方预构传入（`text_arg`），
/// 避免每匹配复制整个源串——字符串不可变，同一 `JsValue` 可安全复用。
fn replacer_cb_args<H: VmHost>(vm: &mut H, text: &str, m: &regress::Match, text_arg: JsValue) -> Vec<JsValue> {
    let range = m.range();
    let mut cb_args: Vec<JsValue> = Vec::with_capacity(m.captures.len() + 2);
    cb_args.push(vm.new_string(&text[range.start..range.end]));
    for i in 1..=m.captures.len() {
        match m.group(i) {
            Some(g) => cb_args.push(vm.new_string(&text[g.start..g.end])),
            None => cb_args.push(JsValue::undefined()),
        }
    }
    cb_args.push(JsValue::int(byte_to_unit(text, range.start) as i32));
    cb_args.push(text_arg);
    cb_args
}

/// 正则模式 + 函数 replacer：global 全替换否则替换首个，逐匹配调用回调，
/// 返回值 ToString 作为替换文本（不展开 `$` 引用）。`text_arg` 为回调第 4 参
/// （原字符串），调用方预构一次，回调期按值复用。
pub(crate) fn regex_replace_fn<H: VmHost>(
    vm: &mut H, regex: &regress::Regex, text: &str, replacer: JsValue, global: bool, text_arg: JsValue,
) -> NativeResult {
    let matches: Vec<regress::Match> = if global {
        regex.find_iter(text).collect()
    } else {
        regex.find(text).into_iter().collect()
    };
    let mut out = String::new();
    let mut last_end = 0;
    for m in matches {
        let range = m.range();
        out.push_str(&text[last_end..range.start]);
        let cb_args = replacer_cb_args(vm, text, &m, text_arg);
        let repl = try_string!(call_replacer(vm, replacer, &cb_args));
        out.push_str(&repl);
        last_end = range.end;
    }
    out.push_str(&text[last_end..]);
    NativeResult::Ok(vm.new_string_owned(out))
}

/// 字符串模式 + 函数 replacer：all 全替换否则替换首个。回调参数
/// `(match, position, string)`（无捕获组）。空模式在每个字符边界匹配一次。
/// `text_arg` 为回调第 4 参（原字符串），调用方预构一次。
fn string_replace_fn<H: VmHost>(
    vm: &mut H, text: &str, pattern: &str, replacer: JsValue, all: bool, text_arg: JsValue,
) -> NativeResult {
    let search_length = pattern.len();
    let mut positions: Vec<usize> = Vec::new();
    if search_length == 0 {
        positions.push(0);
        if all {
            let mut byte = 0;
            for ch in text.chars() {
                byte += ch.len_utf8();
                positions.push(byte);
            }
        }
    } else {
        let mut start = 0;
        while let Some(rel) = text[start..].find(pattern) {
            let p = start + rel;
            positions.push(p);
            if !all {
                break;
            }
            start = p + search_length;
        }
    }
    let mut out = String::new();
    let mut last_end = 0;
    for p in positions {
        out.push_str(&text[last_end..p]);
        let cb_args = [
            vm.new_string(&text[p..p + search_length]),
            JsValue::int(byte_to_unit(text, p) as i32),
            text_arg,
        ];
        let repl = try_string!(call_replacer(vm, replacer, &cb_args));
        out.push_str(&repl);
        last_end = p + search_length;
    }
    out.push_str(&text[last_end..]);
    NativeResult::Ok(vm.new_string_owned(out))
}

/// Unicode 标量数（供 pad 族按字符计数；索引/切片族须用 [`utf16_len`]）。
fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// 取串头 count 个 Unicode 标量（供 pad 族填充计数）。
fn take_chars(s: &str, count: usize) -> String {
    s.chars().take(count).collect()
}

/// UTF-16 code unit 数：astral 字符计 2 单元；ASCII 串与字节数一致直接短路。
fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

/// 字节偏移前缀的 UTF-16 单元数（索引型返回值统一换算入口）。
/// 调用方须保证 `byte` 落在字符边界（well-formed 匹配恒对齐边界）。
fn byte_to_unit(s: &str, byte: usize) -> usize {
    if s.is_ascii() {
        byte
    } else {
        s[..byte].encode_utf16().count()
    }
}

/// UTF-16 单元位置所在字符的起始字节偏移（字符访问/切片起点定位：位置在
/// 字符边界时即该边界，落在代理对中间时取所在 astral 字符起始）。
fn unit_to_char_start(s: &str, unit: usize) -> usize {
    if s.is_ascii() {
        return unit.min(s.len());
    }
    let mut acc = 0usize;
    for (byte, ch) in s.char_indices() {
        if unit < acc + ch.len_utf16() {
            return byte;
        }
        acc += ch.len_utf16();
    }
    s.len()
}

/// UTF-16 单元位置起最近的字符边界字节偏移（查找窗口起点：位置在边界时
/// 即该边界，落在代理对中间时取所在字符末尾——well-formed 匹配不可能从
/// 代理对内部起始）。
fn unit_to_next_boundary(s: &str, unit: usize) -> usize {
    if s.is_ascii() {
        return unit.min(s.len());
    }
    let mut acc = 0usize;
    for (byte, ch) in s.char_indices() {
        let next = acc + ch.len_utf16();
        if unit <= next {
            return if unit <= acc { byte } else { byte + ch.len_utf8() };
        }
        acc = next;
    }
    s.len()
}

/// UTF-16 单元位置是否为字符边界（返回该边界的字节偏移）；落在代理对
/// 中间时返回 None（该位置无法对齐任何 well-formed 子串边界）。
fn unit_to_boundary(s: &str, unit: usize) -> Option<usize> {
    if s.is_ascii() {
        return Some(unit.min(s.len()));
    }
    let mut acc = 0usize;
    for (byte, ch) in s.char_indices() {
        if unit == acc {
            return Some(byte);
        }
        acc += ch.len_utf16();
    }
    (unit == acc).then_some(s.len())
}

/// 按 UTF-16 单元区间取子串：起点取所在字符起始、终点取最近字符边界，
/// 保证结果恒为 well-formed（代理对整体保留）。
fn unit_slice(s: &str, start: usize, end: usize) -> &str {
    if start >= end {
        return "";
    }
    &s[unit_to_char_start(s, start)..unit_to_next_boundary(s, end)]
}

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

/// `String.prototype.indexOf(searchString, position)`：按 UTF-16 单元查找首次出现位置。
pub fn string_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.indexOf called with {} args", args.len());
    // 参数转换先行：search 缺省为 undefined（经 ToString 得 "undefined" 参与查找），
    // position 缺省 0；均可能触发对象 ToString/ToNumber（&mut 路径）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search = try_string!(as_string(vm, search_val)).into_owned();
    let pos_raw = if args.len() > 2 {
        vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize
    } else {
        0
    };
    // 借用 this 原始字符串零拷贝，扫描期内不再有 &mut 调用。
    let s = try_string!(this_string(vm, args));
    let n = utf16_len(&s);
    let pos = pos_raw.min(n);

    if search.is_empty() {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }

    let start_byte = unit_to_next_boundary(&s, pos);
    let haystack = &s[start_byte..];
    if search.len() == 1 {
        if let Some(idx) = memchr(search.as_bytes()[0], haystack.as_bytes()) {
            let matched_byte = start_byte + idx;
            return NativeResult::Ok(JsValue::int(byte_to_unit(&s, matched_byte) as i32));
        }
    } else if let Some(idx) = haystack.find(&search) {
        let matched_byte = start_byte + idx;
        return NativeResult::Ok(JsValue::int(byte_to_unit(&s, matched_byte) as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `String.prototype.includes(searchString, position)`：是否包含子串。
pub fn string_includes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.includes called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search = try_string!(as_string(vm, search_val)).into_owned();
    let pos_raw = if args.len() > 2 {
        vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize
    } else {
        0
    };
    let s = try_string!(this_string(vm, args));
    let n = utf16_len(&s);
    let pos = pos_raw.min(n);

    if search.is_empty() {
        return NativeResult::Ok(JsValue::bool(true));
    }

    let haystack = &s[unit_to_next_boundary(&s, pos)..];
    if search.len() == 1 {
        NativeResult::Ok(JsValue::bool(memchr(search.as_bytes()[0], haystack.as_bytes()).is_some()))
    } else {
        NativeResult::Ok(JsValue::bool(haystack.contains(&search)))
    }
}

/// `String.prototype.charAt(index)`：返回指定 UTF-16 单元位置的单字符；越界返回空串。
pub fn string_char_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.charAt called with {} args", args.len());
    // index 可能触发对象 ToNumber（&mut 路径），先行转换。
    let idx = if args.len() >= 2 {
        vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as i32
    } else {
        0
    };
    let s = try_string!(this_string(vm, args));
    if args.len() < 2 {
        if s.is_empty() {
            return NativeResult::Ok(vm.new_string(""));
        }
        let first = s.chars().next().unwrap();
        // ASCII 走单字符缓存零分配，其余回落普通创建（借用已随 char 提取结束）。
        return match vm.single_char(first) {
            Some(v) => NativeResult::Ok(v),
            None => NativeResult::Ok(vm.new_string(&first.to_string())),
        };
    }
    if idx < 0 || idx as usize >= utf16_len(&s) {
        return NativeResult::Ok(vm.new_string(""));
    }
    // 按 UTF-16 单元定位；落在代理对中间时返回所在 astral 字符（孤立代理
    // 无法在 UTF-8 表示，见架构限制）。
    let byte = unit_to_char_start(&s, idx as usize);
    let ch = s[byte..].chars().next().unwrap();
    match vm.single_char(ch) {
        Some(v) => NativeResult::Ok(v),
        None => NativeResult::Ok(vm.new_string(&ch.to_string())),
    }
}

/// `String.prototype.charCodeAt(index)`：返回指定 UTF-16 单元位置的 code unit；越界返回 NaN。
pub fn string_char_code_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.charCodeAt called with {} args", args.len());
    // index 可能触发对象 ToNumber（&mut 路径），先行转换。
    let idx = if args.len() >= 2 {
        vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as i32
    } else {
        -1
    };
    let s = try_string!(this_string(vm, args));
    if args.len() < 2 {
        if s.is_empty() {
            return NativeResult::Ok(JsValue::float(f64::NAN));
        }
        // 缺省 index=0：返回首单元（astral 首字符为高代理）。
        return NativeResult::Ok(JsValue::int(s.encode_utf16().next().unwrap() as i32));
    }
    if idx < 0 || idx as usize >= utf16_len(&s) {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    // 按 UTF-16 单元展开取码元：astral 字符的高/低代理分别精确返回。
    let unit = s.encode_utf16().nth(idx as usize).unwrap();
    NativeResult::Ok(JsValue::int(unit as i32))
}

/// `String.prototype.concat(...strings)`：拼接 this 与各参数返回新字符串。
pub fn string_concat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.concat called with {} args", args.len());
    let mut result = try_string!(this_string(vm, args)).into_owned();
    for &arg_reg in args.iter().skip(1) {
        match oxide_runtime_api::to_string_full(vm.reg(arg_reg), vm) {
            Ok(s) => result.push_str(&s),
            Err(_) => {
                // ToString on an object may throw via toString/valueOf; propagate the original exception.
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
            }
        }
    }
    NativeResult::Ok(vm.new_string_owned(result))
}

/// `String.prototype.slice(start, end)`：按 UTF-16 单元区间（支持负索引）取子串。
pub fn string_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.slice called with {} args", args.len());
    // 位置参数先行（&mut 转换），后借 this 取子串。
    let start_raw = if args.len() > 1 {
        Some(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as i32)
    } else {
        None
    };
    let end_raw = if args.len() > 2 {
        Some(vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as i32)
    } else {
        None
    };
    let s = try_string!(this_string(vm, args));
    let n = utf16_len(&s) as i32;
    let start = match start_raw {
        Some(v) => {
            if v < 0 {
                (n + v).max(0)
            } else {
                v.min(n)
            }
        }
        None => 0,
    };
    let end = match end_raw {
        Some(v) => {
            if v < 0 {
                (n + v).max(0)
            } else {
                v.min(n)
            }
        }
        None => n,
    };
    let result = if start < end { unit_slice(&s, start as usize, end as usize) } else { "" };
    let owned = result.to_string();
    NativeResult::Ok(vm.new_string_owned(owned))
}

/// `String.prototype.substring(start, end)`：取子串，start/end 自动对调且取非负。
pub fn string_substring<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.substring called with {} args", args.len());
    // 寄存器取值先行（纯函数），后借 this 取子串。
    let start_arg = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    let end_arg = if args.len() > 2 { Some(vm.reg(args[2])) } else { None };
    let s = try_string!(this_string(vm, args));
    let n = utf16_len(&s) as i32;
    let mut start = match start_arg {
        Some(v) => {
            let v = oxide_runtime_api::to_integer_or_infinity(v);
            if v.is_nan() || v < 0.0 {
                0
            } else {
                (v as i32).min(n)
            }
        }
        None => 0,
    };
    let mut end = match end_arg {
        Some(v) => {
            let v = oxide_runtime_api::to_integer_or_infinity(v);
            if v.is_nan() || v < 0.0 {
                0
            } else {
                (v as i32).min(n)
            }
        }
        None => n,
    };
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    let result = unit_slice(&s, start as usize, end as usize);
    let owned = result.to_string();
    NativeResult::Ok(vm.new_string_owned(owned))
}

/// `String.prototype.substr(start, length)`：从 start 起取 length 个 UTF-16 单元
/// （Annex B，支持负 start）。
pub fn string_substr<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.substr called with {} args", args.len());
    // 寄存器取值先行（纯函数），后借 this 截取。
    let start_arg = if args.len() > 1 { Some(vm.reg(args[1])) } else { None };
    let length_arg = if args.len() > 2 { Some(vm.reg(args[2])) } else { None };
    let s = try_string!(this_string(vm, args));
    let len = utf16_len(&s) as isize;
    let start = match start_arg {
        Some(v) => {
            let n = oxide_runtime_api::to_integer_or_infinity(v) as isize;
            if n < 0 {
                (len + n).max(0)
            } else {
                n.min(len)
            }
        }
        None => 0,
    } as usize;
    let length = match length_arg {
        Some(v) => (oxide_runtime_api::to_integer_or_infinity(v) as isize).max(0) as usize,
        None => len as usize - start,
    };
    // 起点按 UTF-16 单元定位取窗口（起点落在代理对中间时取所在 astral 字符，
    // 孤立代理无法在 UTF-8 表示，见架构限制）。
    let count = length.min(len as usize - start);
    let result = unit_slice(&s, start, start + count);
    let owned = result.to_string();
    NativeResult::Ok(vm.new_string_owned(owned))
}

/// `String.prototype.at(index)`：按 UTF-16 单元索引取字符（支持负索引）；越界返回 undefined。
pub fn string_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.at called with {} args", args.len());
    let idx = if args.len() > 1 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1])) as i32
    } else {
        0
    };
    let s = try_string!(this_string(vm, args));
    let len = utf16_len(&s) as i32;
    let idx = if idx < 0 { len + idx } else { idx };
    if idx < 0 || idx >= len {
        return NativeResult::Ok(JsValue::undefined());
    }
    // 按 UTF-16 单元定位；落在代理对中间时返回所在 astral 字符（架构限制）。
    let byte = unit_to_char_start(&s, idx as usize);
    let ch = s[byte..].chars().next().unwrap().to_string();
    NativeResult::Ok(vm.new_string(&ch))
}

/// `String.prototype.lastIndexOf(searchString, position)`：从后往前查找首次出现位置。
pub fn string_last_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.lastIndexOf called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search = try_string!(as_string(vm, search_val)).into_owned();
    let pos_raw = if args.len() > 2 {
        let p = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2]));
        if p.is_nan() {
            None
        } else {
            Some(p as usize)
        }
    } else {
        None
    };
    let s = try_string!(this_string(vm, args));
    let n = utf16_len(&s);
    let pos = match pos_raw {
        Some(p) => p.min(n),
        None => n,
    };

    if search.is_empty() {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }

    let end_byte = unit_to_next_boundary(&s, (pos + 1).min(n));
    let haystack = &s[..end_byte];

    if search.len() == 1 {
        if let Some(idx) = memchr::memrchr(search.as_bytes()[0], haystack.as_bytes()) {
            return NativeResult::Ok(JsValue::int(byte_to_unit(&s, idx) as i32));
        }
    } else if let Some(idx) = haystack.rfind(&search) {
        return NativeResult::Ok(JsValue::int(byte_to_unit(&s, idx) as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `String.prototype.toUpperCase`：全大写转换。
pub fn string_to_upper_case<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.toUpperCase called with {} args", args.len());
    let s = try_string!(this_string(vm, args));
    let upper = s.to_uppercase();
    NativeResult::Ok(vm.new_string_owned(upper))
}

/// `String.prototype.toLowerCase`：全小写转换。
pub fn string_to_lower_case<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.toLowerCase called with {} args", args.len());
    let s = try_string!(this_string(vm, args));
    let lower = s.to_lowercase();
    NativeResult::Ok(vm.new_string_owned(lower))
}

/// `String.prototype.trim`：去除两端空白。
pub fn string_trim<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trim called with {} args", args.len());
    let s = try_string!(this_string(vm, args));
    let trimmed = s.trim().to_string();
    NativeResult::Ok(vm.new_string_owned(trimmed))
}

/// `String.prototype.repeat(count)`：重复字符串 count 次（当前上限 10000 防滥用）。
pub fn string_repeat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.repeat called with {} args", args.len());
    // count 转换先行（&mut 路径），后借 this 重复。
    let n = if args.len() > 1 {
        (vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as usize).min(10000)
    } else {
        1
    };
    let s = try_string!(this_string(vm, args));
    let repeated = s.repeat(n);
    NativeResult::Ok(vm.new_string_owned(repeated))
}

/// `String.prototype.padStart(targetLength, padString)`：在头部补足 padString 到目标长度。
pub fn string_pad_start<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.padStart called with {} args", args.len());
    // 参数转换先行（&mut 路径）：targetLength 与 padString 均可能触发对象转换。
    let target_arg = if args.len() > 1 {
        Some(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as usize)
    } else {
        None
    };
    let pad = if args.len() > 2 {
        try_string!(as_string(vm, vm.reg(args[2]))).into_owned()
    } else {
        " ".to_string()
    };
    let s = try_string!(this_string(vm, args));
    let s_len = char_len(&s);
    let target = target_arg.unwrap_or(s_len);
    if target > 10000 {
        builtins_error!("String.prototype.padStart: invalid receiver");
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    if s_len >= target || pad.is_empty() {
        let owned = s.into_owned();
        return NativeResult::Ok(vm.new_string_owned(owned));
    }
    let needed = target - s_len;
    let pad_len = char_len(&pad).max(1);
    let reps = needed.div_ceil(pad_len);
    let mut out = take_chars(&pad.repeat(reps), needed);
    out.push_str(&s);
    NativeResult::Ok(vm.new_string_owned(out))
}

/// `String.prototype.padEnd(targetLength, padString)`：在尾部补足 padString 到目标长度。
pub fn string_pad_end<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.padEnd called with {} args", args.len());
    // 参数转换先行（&mut 路径）：targetLength 与 padString 均可能触发对象转换。
    let target_arg = if args.len() > 1 {
        Some(vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as usize)
    } else {
        None
    };
    let pad = if args.len() > 2 {
        try_string!(as_string(vm, vm.reg(args[2]))).into_owned()
    } else {
        " ".to_string()
    };
    let s = try_string!(this_string(vm, args));
    let s_len = char_len(&s);
    let target = target_arg.unwrap_or(s_len);
    if target > 10000 {
        builtins_error!("String.prototype.padEnd: invalid receiver");
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    if s_len >= target || pad.is_empty() {
        let owned = s.into_owned();
        return NativeResult::Ok(vm.new_string_owned(owned));
    }
    let needed = target - s_len;
    let pad_len = char_len(&pad).max(1);
    let reps = needed.div_ceil(pad_len);
    let mut out = s.into_owned();
    out.push_str(&take_chars(&pad.repeat(reps), needed));
    NativeResult::Ok(vm.new_string_owned(out))
}

/// `String.prototype.startsWith(searchString, position)`：是否以指定子串开头。
pub fn string_starts_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.startsWith called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search = try_string!(as_string(vm, search_val)).into_owned();
    let pos_raw = if args.len() > 2 {
        vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize
    } else {
        0
    };
    let s = try_string!(this_string(vm, args));
    let n = utf16_len(&s);
    let pos = pos_raw.min(n);
    // 位置须对齐字符边界才有 well-formed 前缀可比；落在代理对中间时仅空串可匹配。
    let result = match unit_to_boundary(&s, pos) {
        Some(byte) => s[byte..].starts_with(&search),
        None => search.is_empty(),
    };
    NativeResult::Ok(JsValue::bool(result))
}

/// `String.prototype.endsWith(searchString, endPosition)`：是否以指定子串结尾。
pub fn string_ends_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.endsWith called with {} args", args.len());
    // 参数转换先行（&mut 路径）：search 缺省为 undefined（经 ToString 得 "undefined"）。
    let search_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };
    let search = try_string!(as_string(vm, search_val)).into_owned();
    let end_pos_raw = if args.len() > 2 {
        vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize
    } else {
        usize::MAX
    };
    let s = try_string!(this_string(vm, args));
    let n = utf16_len(&s);
    let end_pos = end_pos_raw.min(n);
    // 截断位置须对齐字符边界才有 well-formed 后缀可比；落在代理对中间时仅空串可匹配。
    let result = match unit_to_boundary(&s, end_pos) {
        Some(byte) => s[..byte].ends_with(&search),
        None => search.is_empty(),
    };
    NativeResult::Ok(JsValue::bool(result))
}

/// `String.prototype.split(separator, limit)`：按分隔符拆分为字符串数组；
/// 分隔符可为 RegExp（含捕获组）或字符串。
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
        let re_ptr = sep_val.unwrap().as_js_object_ptr();
        // SAFETY: is_re 已保证 sep_val 为非空对象且 proto 恒等 RegExp.prototype。
        let re = unsafe { &*re_ptr };
        re.native_fn().is_some()
    };
    let sep = if is_undefined_sep || has_native_re {
        String::new()
    } else {
        try_string!(as_string(vm, sep_val.unwrap())).into_owned()
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
    let s = try_string!(this_string(vm, args));
    // 规范：separator 为 undefined 时返回 [this]。
    if is_undefined_sep {
        let owned = s.into_owned();
        return NativeResult::Ok(make_string_array(vm, vec![owned]));
    }
    if is_re {
        let re_ptr = sep_val.unwrap().as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        if let Some(fn_ptr) = re.native_fn() {
            let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
            let mut parts: Vec<String> = Vec::new();
            let mut last_end = 0;
            for m in regex.find_iter(&s) {
                if parts.len() >= limit {
                    break;
                }
                let range = m.range();
                parts.push(s[last_end..range.start].to_string());
                if parts.len() >= limit {
                    break;
                }
                for i in 1..=m.captures.len() {
                    if parts.len() >= limit {
                        break;
                    }
                    match m.group(i) {
                        Some(g) => parts.push(s[g.start..g.end].to_string()),
                        None => parts.push(String::new()),
                    }
                }
                last_end = range.end;
            }
            if last_end <= s.len() && parts.len() < limit {
                parts.push(s[last_end..].to_string());
            }
            return NativeResult::Ok(make_string_array(vm, parts));
        }
        // 无原生正则的类正则对象回退到字符串路径。
    }
    if sep.is_empty() {
        // 两段式：先借 this 收集字符（Copy），借用结束后再逐字符产出——
        // ASCII 走单字符缓存零分配，其余回落普通创建。
        let chars: Vec<char> = s.chars().take(limit).collect();
        let mut values = Vec::with_capacity(chars.len());
        for c in chars {
            values.push(match vm.single_char(c) {
                Some(v) => v,
                None => vm.new_string(&c.to_string()),
            });
        }
        return NativeResult::Ok(make_string_array_values(vm, values));
    }
    let parts: Vec<String> = s.split(&sep).map(|p| p.to_string()).take(limit).collect();
    NativeResult::Ok(make_string_array(vm, parts))
}

/// `replace`/`replaceAll` 共用的分叉实现：按 replacer 类型与参数形态选择路径，
/// 全原始字符串场景零拷贝（共享借用三提取），函数 replacer 场景 receiver 走
/// owned（回调跨 `&mut` 的硬约束）。
///
/// # 步骤
/// 1. receiver 前置校验（RequireObjectCoercible，纯 is_* 读取）。
/// 2. 纯对象头读取判定分叉：正则身份/编译正则/global 标志/函数 replacer。
/// 3. 按分支执行替换。
///
/// # 边界与前提
/// - 缺省 searchValue/replaceValue 均按 ToString(undefined)="undefined" 处理
///   （`s.replace()` 全缺省等价于把 "undefined" 替换为 "undefined"，结果与
///   原串一致；`s.replace("b")` 得到 "aundefinedc" 而非 "ac"）。
/// - replaceAll 遇非 global 正则抛 TypeError；类正则对象（proto 恒等
///   RegExp.prototype 但无编译正则）replace/replaceAll 统一按 ToString 文本走
///   字符串路径。
///
/// # 注意事项
/// - 分支 B 的 `&H` 共享借用与 `&mut` 互斥由编译器强制，三 `&str` 同源可共存
///   （E0499 仅发生在 `&mut` 派生 Cow 并存）。
/// - 函数 replacer 分支 receiver 必须 owned：`call_function_sync` 是 `&mut`，
///   文本若借自 vm 会 E0499。
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
            let g = re.hash_props_vec().and_then(|v| v.get(3)).map(|v| v.as_bool()).unwrap_or(false);
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
    // 类正则对象（无编译正则）无 flags 概念，统一走 as_string 文本路径，不在此检查。
    if all && has_native_re && !is_global {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "String.prototype.replaceAll called with a non-global RegExp",
        ));
    }

    // 分支 A：函数 replacer。回调经 call_function_sync（&mut），文本若借自 vm
    // 会 E0499，receiver 必须 owned 本地 String（跨回调的硬约束）。
    if let Some(replacer_val) = replacer_val {
        let s = try_string!(this_string(vm, args)).into_owned();
        // 回调第 4 参（原字符串）预构一次：原始字符串 receiver 直接复用
        // this_val 零拷贝，对象 receiver 提升为单次会话串（原每匹配整串复制）。
        let text_arg = if this_val.is_string() { this_val } else { vm.new_string(&s) };
        if has_native_re {
            let re_ptr = pattern_val.as_js_object_ptr();
            let re = unsafe { &*re_ptr };
            let fn_ptr = re.native_fn().unwrap();
            // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
            let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
            return regex_replace_fn(vm, regex, &s, replacer_val, is_global, text_arg);
        }
        let pattern = try_string!(as_string(vm, pattern_val)).into_owned();
        return string_replace_fn(vm, &s, &pattern, replacer_val, all, text_arg);
    }

    let replacement_is_string = args.len() <= 2 || replacement_val.is_string();

    // 分支 B：全原始字符串快路径——共享借用三提取，receiver/pattern/replacement
    // 零拷贝（`&H` 共享借用派生多个 `&str` 可共存，E0499 只发生在 `&mut` 派生）。
    if this_val.is_string() && pattern_val.is_string() && replacement_is_string {
        let h = &*vm;
        let s = h.string_ref(this_val);
        let p = h.string_ref(pattern_val);
        let r: &str = if replacement_val.is_string() {
            h.string_ref(replacement_val)
        } else {
            "undefined"
        };
        let result = if all { s.replace(p, r) } else { s.replacen(p, r, 1) };
        return NativeResult::Ok(vm.new_string_owned(result));
    }

    // 分支 B2：正则 pattern 快路径——regex 为对象内裸指针（不占 vm 借用），
    // receiver/replacement 共享借用零拷贝。
    if this_val.is_string() && has_native_re && replacement_is_string {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = re.native_fn().unwrap();
        // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
        let h = &*vm;
        let s = h.string_ref(this_val);
        let r: &str = if replacement_val.is_string() {
            h.string_ref(replacement_val)
        } else {
            "undefined"
        };
        let result = regex_replace_manual(regex, s, r, is_global);
        return NativeResult::Ok(vm.new_string_owned(result));
    }

    // 分支 C：一般路径（对象参与转换）。
    // 原始字符串 receiver：参数转换前置（&mut 路径）再借 receiver 零拷贝扫描；
    // 对象 receiver：先 ToString 得 owned（into_owned 即结束借用），转换顺序与
    // 改造前一致。
    let (s, pattern, replacement) = if this_val.is_string() {
        let pattern = if has_native_re {
            String::new()
        } else if args.len() < 2 {
            "undefined".to_string()
        } else {
            try_string!(as_string(vm, pattern_val)).into_owned()
        };
        let replacement = if args.len() > 2 {
            try_string!(as_string(vm, replacement_val)).into_owned()
        } else {
            "undefined".to_string()
        };
        let s = try_string!(this_string(vm, args));
        (s, pattern, replacement)
    } else {
        let s = try_string!(this_string(vm, args)).into_owned();
        let pattern = if has_native_re {
            String::new()
        } else if args.len() < 2 {
            "undefined".to_string()
        } else {
            try_string!(as_string(vm, pattern_val)).into_owned()
        };
        let replacement = if args.len() > 2 {
            try_string!(as_string(vm, replacement_val)).into_owned()
        } else {
            "undefined".to_string()
        };
        (Cow::Owned(s), pattern, replacement)
    };

    // 统一扫描：命中编译正则走手动 $ 展开，否则字符串子串替换。
    if has_native_re {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = re.native_fn().unwrap();
        // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
        let result = regex_replace_manual(regex, &s, &replacement, is_global);
        return NativeResult::Ok(vm.new_string_owned(result));
    }
    let result = if all {
        s.replace(&pattern, &replacement)
    } else {
        s.replacen(&pattern, &replacement, 1)
    };
    NativeResult::Ok(vm.new_string_owned(result))
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
    let pattern = match pattern_val {
        Some(v) if !is_re => try_string!(as_string(vm, v)).into_owned(),
        _ => String::new(),
    };
    let s = try_string!(this_string(vm, args));
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::null());
    }
    let pattern_val = pattern_val.unwrap();
    if is_re {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(JsValue::null()),
        };
        // SAFETY: fn_ptr 持有 regexp_constructor 存放的 `Box<regress::Regex>` 指针。
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regress::Regex) };
        let is_global = re.hash_props_vec().and_then(|v| v.get(3)).map(|v| v.as_bool()).unwrap_or(false);
        if is_global {
            let matches: Vec<String> = regex.find_iter(&s).map(|m| s[m.range()].to_string()).collect();
            return NativeResult::Ok(make_string_array(vm, matches));
        }
        if let Some(m) = regex.find(&s) {
            let range = m.range();
            let mut parts: Vec<String> = Vec::with_capacity(m.captures.len() + 1);
            parts.push(s[range.start..range.end].to_string());
            for i in 1..=m.captures.len() {
                match m.group(i) {
                    Some(g) => parts.push(s[g.start..g.end].to_string()),
                    None => parts.push(String::new()),
                }
            }
            return NativeResult::Ok(make_string_array(vm, parts));
        }
        return NativeResult::Ok(JsValue::null());
    }
    if let Some(pos) = s.find(&pattern) {
        let matched = s[pos..pos + pattern.len()].to_string();
        return NativeResult::Ok(make_string_array(vm, vec![matched]));
    }
    NativeResult::Ok(JsValue::null())
}

/// `String.prototype.search(pattern)`：返回首个匹配位置（UTF-16 单元），无匹配返回 -1。
pub fn string_search<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.search called with {} args", args.len());
    // 参数读取守卫先行：无参调用缺省 searchString 为 undefined（args 仅含 this 槽）。
    let pattern_val = if args.len() >= 2 { vm.reg(args[1]) } else { JsValue::undefined() };

    // 正则判定与参数转换先行（&mut 路径），后借 this 扫描。
    let is_re = args.len() >= 2 && is_regexp_obj(pattern_val, vm);
    let pattern = if args.len() >= 2 && !is_re {
        try_string!(as_string(vm, pattern_val)).into_owned()
    } else {
        String::new()
    };
    let s = try_string!(this_string(vm, args));
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
        if let Some(m) = regex.find(&s) {
            return NativeResult::Ok(JsValue::int(byte_to_unit(&s, m.range().start) as i32));
        }
        return NativeResult::Ok(JsValue::int(-1));
    }
    if let Some(pos) = s.find(&pattern) {
        return NativeResult::Ok(JsValue::int(byte_to_unit(&s, pos) as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

/// `String.prototype.trimStart`：去除头部空白。
pub fn string_trim_start<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trimStart called with {} args", args.len());
    let s = try_string!(this_string(vm, args));
    let trimmed = s.trim_start().to_string();
    NativeResult::Ok(vm.new_string_owned(trimmed))
}

/// `String.prototype.trimEnd`：去除尾部空白。
pub fn string_trim_end<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trimEnd called with {} args", args.len());
    let s = try_string!(this_string(vm, args));
    let trimmed = s.trim_end().to_string();
    NativeResult::Ok(vm.new_string_owned(trimmed))
}

/// `String.prototype.codePointAt(pos)`：按 UTF-16 code unit 位置取 code point
/// （surrogate pair 合并）；越界或孤立代理返回 undefined。
pub fn string_code_point_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.codePointAt called with {} args", args.len());
    // pos 转换先行（&mut 路径），后借 this 按 code unit 定位。
    let pos = if args.len() > 1 {
        let pos_val = vm.reg(args[1]);
        if pos_val.is_symbol() {
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert a Symbol value to a number"));
        }
        match vm.coerce_number_bounded(pos_val) {
            Ok(n) => n,
            Err(_) => {
                // pos 对象 valueOf/toString 抛出的异常须原样传播。
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert argument to a number"));
            }
        }
    } else {
        0.0
    };
    let pos = if pos.is_nan() { 0.0 } else { pos.trunc() };
    if pos < 0.0 || pos > u32::MAX as f64 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let pos = pos as usize;
    let s = try_string!(this_string(vm, args));
    // JS 规范索引是 UTF-16 code unit 位置，须按代理对展开定位（astral 字符占两单元）。
    let units: Vec<u16> = s.encode_utf16().collect();
    if pos >= units.len() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let first = units[pos];
    if (0xD800..=0xDBFF).contains(&first) && pos + 1 < units.len() {
        let second = units[pos + 1];
        if (0xDC00..=0xDFFF).contains(&second) {
            let cp = 0x10000 + (((first - 0xD800) as u32) << 10) + (second - 0xDC00) as u32;
            return NativeResult::Ok(JsValue::int(cp as i32));
        }
    }
    NativeResult::Ok(JsValue::int(first as i32))
}

/// `String.prototype.isWellFormed()`：字符串无孤立 surrogate（每个 UTF-16
/// 码元要么是合法字符，要么与相邻码元成代理对）时返回 true。
pub fn string_is_well_formed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.isWellFormed called with {} args", args.len());
    let s = try_string!(this_string(vm, args));
    let mut iter = s.encode_utf16().peekable();
    while let Some(&first) = iter.peek() {
        if (0xDC00..=0xDFFF).contains(&first) {
            return NativeResult::Ok(JsValue::bool(false));
        }
        if (0xD800..=0xDBFF).contains(&first) {
            if !matches!(iter.nth(1), Some(second) if (0xDC00..=0xDFFF).contains(&second)) {
                return NativeResult::Ok(JsValue::bool(false));
            }
        } else {
            iter.next();
        }
    }
    NativeResult::Ok(JsValue::bool(true))
}

/// `String.prototype.toWellFormed()`：把孤立 surrogate 替换为 U+FFFD 返回新字符串。
pub fn string_to_well_formed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.toWellFormed called with {} args", args.len());
    let s = try_string!(this_string(vm, args));
    // 引擎字符串为合法 UTF-8，Rust char 不可能落在 surrogate 区间；遍历保留
    // 通用来正确编码未来可能出现的替换路径。
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if (0xD800..=0xDFFF).contains(&(c as u32)) {
            out.push('\u{FFFD}');
        } else {
            out.push(c);
        }
    }
    NativeResult::Ok(vm.new_string_owned(out))
}

/// `String.prototype.normalize(form)`：按 NFC/NFD/NFKC/NFKD 规范化为 Unicode 规范形式。
pub fn string_normalize<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.normalize called with {} args", args.len());
    use unicode_normalization::UnicodeNormalization;
    let form = if args.len() > 1 {
        try_string!(as_string(vm, vm.reg(args[1]))).into_owned()
    } else {
        "NFC".to_string()
    };
    let s = try_string!(this_string(vm, args));
    let result: String = match form.as_str() {
        "NFD" => s.nfd().collect(),
        "NFKC" => s.nfkc().collect(),
        "NFKD" => s.nfkd().collect(),
        _ => s.nfc().collect(),
    };
    NativeResult::Ok(vm.new_string_owned(result))
}

pub(crate) const MALL_INPUT: &str = "__mal_input__";
pub(crate) const MALL_INDEX: &str = "__mal_index__";
pub(crate) const MALL_RE: &str = "__mal_re__";

/// `String.prototype.matchAll(pattern)`：返回带 `next` 的迭代器，逐步产出全部匹配
/// （要求 RegExp 带 global 标志；普通字符串会被转义成等效正则）。
pub fn string_match_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.matchAll called with {} args", args.len());
    let s = try_string!(this_string(vm, args)).into_owned();
    if args.len() < 2 {
        builtins_error!("String.prototype.matchAll: invalid receiver");
        return NativeResult::Err(JsValue::undefined());
    }
    let pattern_val = vm.reg(args[1]);

    let re_obj = if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let is_global = re.hash_props_vec().and_then(|v| v.get(3)).map(|v| v.as_bool()).unwrap_or(false);
        if !is_global {
            return NativeResult::Err(crate::error::create_type_error(
                vm,
                "String.prototype.matchAll: regex must have global flag",
            ));
        }
        pattern_val
    } else {
        let pattern_str = try_string!(as_string(vm, pattern_val));
        let escaped = regress::escape(pattern_str.as_ref());
        let rx_str = if escaped.is_empty() { String::from("(?:)") } else { format!("({})", escaped) };
        let compiled = match regress::Regex::new(&rx_str) {
            Ok(rx) => rx,
            Err(e) => {
                return NativeResult::Err(crate::error::create_syntax_error(vm, &format!("Invalid regex: {}", e)));
            }
        };
        let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
        let mut stub = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto));
        let boxed = Box::new(compiled);
        let raw = Box::into_raw(boxed) as *const u8;
        stub.set_native_fn(Some(unsafe { oxide_types::object::NativeFnPtr::from_raw(raw as *const ()) }));
        let stub_ptr = vm.alloc_object(stub);
        JsValue::from_js_object(stub_ptr)
    };

    // 构建包装对象。
    builder_wrapper(vm, &s, re_obj)
}

fn builder_wrapper<H: VmHost>(vm: &mut H, input: &str, re_obj: JsValue) -> NativeResult {
    // matchAll 迭代器挂 %RegExpStringIteratorPrototype%（链到 %IteratorPrototype%），
    // next 由原型提供（不挂实例 own）。
    let regexp_iter_proto = vm.session().builtin_world().regexp_string_iterator_proto.as_ptr() as *mut JsObject;
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(regexp_iter_proto)));

    let wrapper_obj = unsafe { &mut *wrapper };
    let input_si = vm.kernel_core().perm_interner().intern(MALL_INPUT).0;
    let index_si = vm.kernel_core().perm_interner().intern(MALL_INDEX).0;
    let re_si = vm.kernel_core().perm_interner().intern(MALL_RE).0;

    let input_val = vm.new_string(input);
    vm.set_or_create_prop_value(wrapper_obj, input_si, input_val);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, re_si, re_obj);

    NativeResult::Ok(JsValue::from_js_object(wrapper))
}

/// `String.prototype[Symbol.iterator]()`：返回按 Unicode code point 迭代字符的迭代器。
///
/// # 步骤
/// 1. null/undefined 抛 TypeError（RequireObjectCoercible）
/// 2. this 经 ToString 完整转换（对象取 toString 结果，Symbol 抛 TypeError）
/// 3. 包成统一迭代器包装（next 逐 code point 产出，耗尽后 done）
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
    let s = match oxide_runtime_api::to_string_full(this_val, vm) {
        Ok(s) => s,
        Err(_) => {
            // ToString 触发对象 toString/valueOf 抛出的原生异常须原样传播。
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    let s_val = vm.new_string_owned(s);
    // String 迭代器挂 %StringIteratorPrototype%（链到 %IteratorPrototype%）：
    // 包装机制复用通用路径，仅替换原型为中继层。
    let string_iter_proto = vm.session().builtin_world().string_iterator_proto.as_ptr() as *mut JsObject;
    match crate::iterator::make_iterator_for_value_with_proto(vm, s_val, string_iter_proto) {
        Ok(iterator) => NativeResult::Ok(iterator),
        Err(err) => NativeResult::Err(err),
    }
}

/// `matchAll` 迭代器的 `next`：返回 `{value: 匹配数组, done}`，耗尽后 done 为 true。
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
        Ok(v) => v,
        Err(_) => return make_match_done_result(vm, JsValue::undefined()),
    };
    let input_str = oxide_runtime_api::to_string(input_val);
    let idx_val = match vm.ordinary_get(wrapper, index_si, this_val) {
        Ok(v) => v,
        Err(_) => return make_match_done_result(vm, JsValue::undefined()),
    };
    let mut idx = if idx_val.is_int() { idx_val.as_int().max(0) as usize } else { 0 };
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

    if let Some(m) = regex.find_from(&input_str, idx).next() {
        let range = m.range();
        let mut parts: Vec<String> = Vec::new();
        parts.push(input_str[range.start..range.end].to_string());
        for i in 1..=m.captures.len() {
            match m.group(i) {
                Some(g) => parts.push(input_str[g.start..g.end].to_string()),
                None => parts.push(String::new()),
            }
        }
        // 空匹配（range.start == range.end）必须推进至少一个 UTF-8 字符，否则同一
        // 位置反复空匹配死循环；空匹配落在串尾时把游标推到串尾之后，保证下一轮
        // 直接耗尽（与 regress find_iter 的 next_right_pos 推进语义一致）。
        idx = if range.end > range.start {
            range.end
        } else {
            range.end
                + input_str
                    .get(range.end..)
                    .and_then(|s| s.chars().next())
                    .map_or(1, char::len_utf8)
        };
        vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(idx as i32));
        let arr_val = make_string_array(vm, parts);
        make_match_done_result(vm, arr_val)
    } else {
        make_match_done_result(vm, JsValue::undefined())
    }
}

fn make_match_done_result<H: VmHost>(vm: &mut H, value: JsValue) -> NativeResult {
    let done = value.is_undefined();
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let obj = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
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
