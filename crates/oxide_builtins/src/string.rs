use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use memchr::memchr;
use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

fn this_string<H: VmHost>(vm: &H, args: &[u8]) -> String {
    oxide_runtime_api::to_string(vm.reg(args[0]))
}

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
    NativeResult::Ok(vm.new_string(&out))
}

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

pub fn string_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let s = if args.len() > 1 {
        oxide_runtime_api::to_string(vm.reg(args[1]))
    } else {
        String::new()
    };
    let str_val = vm.new_string(&s);

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

fn make_string_array<H: VmHost>(vm: &mut H, parts: &[String]) -> JsValue {
    let proto = vm.session().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let n = parts.len();
    let arr =
        vm.epoch()
            .alloc(JsObject::new_array(EMPTY_SHAPE_ID, JsValue::from_js_object(proto), n, vm.epoch().bump()));
    unsafe {
        for (i, s) in parts.iter().enumerate() {
            let sv = vm.new_string(s);
            (*arr).set_prop_at(i, sv);
        }
        (*arr).set_prop_count(n);
    }
    JsValue::from_js_object(arr)
}

fn as_string<H: VmHost>(_vm: &H, val: JsValue) -> String {
    oxide_runtime_api::to_string(val)
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn byte_index_at_char(s: &str, char_pos: usize) -> usize {
    if char_pos == 0 {
        return 0;
    }
    s.char_indices().nth(char_pos).map(|(idx, _)| idx).unwrap_or(s.len())
}

fn char_slice(s: &str, start: usize, end: usize) -> &str {
    let start_byte = byte_index_at_char(s, start);
    let end_byte = byte_index_at_char(s, end);
    &s[start_byte..end_byte]
}

fn take_chars(s: &str, count: usize) -> String {
    s.chars().take(count).collect()
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

pub fn string_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.indexOf called with {} args", args.len());
    let s = this_string(vm, args);
    let n = char_len(&s);
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let pos = if args.len() > 2 {
        (vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize).min(n)
    } else {
        0
    };

    if search.is_empty() {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }

    let start_byte = byte_index_at_char(&s, pos);
    let haystack = &s[start_byte..];
    if search.len() == 1 {
        if let Some(idx) = memchr(search.as_bytes()[0], haystack.as_bytes()) {
            let matched_byte = start_byte + idx;
            return NativeResult::Ok(JsValue::int(char_len(&s[..matched_byte]) as i32));
        }
    } else if let Some(idx) = haystack.find(&search) {
        let matched_byte = start_byte + idx;
        return NativeResult::Ok(JsValue::int(char_len(&s[..matched_byte]) as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

pub fn string_includes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.includes called with {} args", args.len());
    let s = this_string(vm, args);
    let n = char_len(&s);
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let pos = if args.len() > 2 {
        (vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize).min(n)
    } else {
        0
    };

    if search.is_empty() {
        return NativeResult::Ok(JsValue::bool(true));
    }

    let haystack = &s[byte_index_at_char(&s, pos)..];
    if search.len() == 1 {
        NativeResult::Ok(JsValue::bool(memchr(search.as_bytes()[0], haystack.as_bytes()).is_some()))
    } else {
        NativeResult::Ok(JsValue::bool(haystack.contains(&search)))
    }
}

pub fn string_char_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.charAt called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        if s.is_empty() {
            return NativeResult::Ok(vm.new_string(""));
        }
        return NativeResult::Ok(vm.new_string(&take_chars(&s, 1)));
    }
    let idx = vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as i32;
    if idx < 0 || idx as usize >= char_len(&s) {
        return NativeResult::Ok(vm.new_string(""));
    }
    let ch = s.chars().nth(idx as usize).map(|c| c.to_string()).unwrap_or_default();
    NativeResult::Ok(vm.new_string(&ch))
}

pub fn string_char_code_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.charCodeAt called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        if s.is_empty() {
            return NativeResult::Ok(JsValue::float(f64::NAN));
        }
        return NativeResult::Ok(JsValue::int(s.chars().next().unwrap() as i32));
    }
    let idx = vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as i32;
    if idx < 0 || idx as usize >= char_len(&s) {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    NativeResult::Ok(JsValue::int(s.chars().nth(idx as usize).unwrap() as i32))
}

pub fn string_concat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.concat called with {} args", args.len());
    let mut result = this_string(vm, args);
    for &arg_reg in args.iter().skip(1) {
        result.push_str(&oxide_runtime_api::to_string(vm.reg(arg_reg)));
    }
    NativeResult::Ok(vm.new_string(&result))
}

pub fn string_slice<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.slice called with {} args", args.len());
    let s = this_string(vm, args);
    let n = char_len(&s) as i32;
    let start = if args.len() > 1 {
        let v = vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as i32;
        if v < 0 {
            (n + v).max(0)
        } else {
            v.min(n)
        }
    } else {
        0
    };
    let end = if args.len() > 2 {
        let v = vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as i32;
        if v < 0 {
            (n + v).max(0)
        } else {
            v.min(n)
        }
    } else {
        n
    };
    let start = start as usize;
    let end = end as usize;
    let result = if start < end { char_slice(&s, start, end) } else { "" };
    NativeResult::Ok(vm.new_string(result))
}

pub fn string_substring<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.substring called with {} args", args.len());
    let s = this_string(vm, args);
    let n = char_len(&s) as i32;
    let mut start = if args.len() > 1 {
        (vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as i32)
            .max(0)
            .min(n)
    } else {
        0
    };
    let mut end = if args.len() > 2 {
        (vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as i32)
            .max(0)
            .min(n)
    } else {
        n
    };
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    let result = char_slice(&s, start as usize, end as usize);
    NativeResult::Ok(vm.new_string(result))
}

pub fn string_to_upper_case<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.toUpperCase called with {} args", args.len());
    let s = this_string(vm, args);
    NativeResult::Ok(vm.new_string(&s.to_uppercase()))
}

pub fn string_to_lower_case<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.toLowerCase called with {} args", args.len());
    let s = this_string(vm, args);
    NativeResult::Ok(vm.new_string(&s.to_lowercase()))
}

pub fn string_trim<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trim called with {} args", args.len());
    let s = this_string(vm, args);
    NativeResult::Ok(vm.new_string(s.trim()))
}

pub fn string_repeat<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.repeat called with {} args", args.len());
    let s = this_string(vm, args);
    let n = if args.len() > 1 {
        (vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as usize).min(10000)
    } else {
        1
    };
    NativeResult::Ok(vm.new_string(&s.repeat(n)))
}

pub fn string_pad_start<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.padStart called with {} args", args.len());
    let s = this_string(vm, args);
    let s_len = char_len(&s);
    let target = if args.len() > 1 {
        vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as usize
    } else {
        s_len
    };
    if target > 10000 {
        builtins_error!("String.prototype.padStart: invalid receiver");
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    let pad = if args.len() > 2 { as_string(vm, vm.reg(args[2])) } else { " ".to_string() };
    if s_len >= target || pad.is_empty() {
        return NativeResult::Ok(vm.new_string(&s));
    }
    let needed = target - s_len;
    let pad_len = char_len(&pad).max(1);
    let reps = needed.div_ceil(pad_len);
    let mut out = take_chars(&pad.repeat(reps), needed);
    out.push_str(&s);
    NativeResult::Ok(vm.new_string(&out))
}

pub fn string_pad_end<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.padEnd called with {} args", args.len());
    let s = this_string(vm, args);
    let s_len = char_len(&s);
    let target = if args.len() > 1 {
        vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as usize
    } else {
        s_len
    };
    if target > 10000 {
        builtins_error!("String.prototype.padEnd: invalid receiver");
        return NativeResult::Err(crate::error::create_range_error(vm, "Invalid string length"));
    }
    let pad = if args.len() > 2 { as_string(vm, vm.reg(args[2])) } else { " ".to_string() };
    if s_len >= target || pad.is_empty() {
        return NativeResult::Ok(vm.new_string(&s));
    }
    let needed = target - s_len;
    let pad_len = char_len(&pad).max(1);
    let reps = needed.div_ceil(pad_len);
    let mut out = s;
    out.push_str(&take_chars(&pad.repeat(reps), needed));
    NativeResult::Ok(vm.new_string(&out))
}

pub fn string_starts_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.startsWith called with {} args", args.len());
    let s = this_string(vm, args);
    let n = char_len(&s);
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let pos = if args.len() > 2 {
        (vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize).min(n)
    } else {
        0
    };
    NativeResult::Ok(JsValue::bool(s[byte_index_at_char(&s, pos)..].starts_with(&search)))
}

pub fn string_ends_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.endsWith called with {} args", args.len());
    let s = this_string(vm, args);
    let n = char_len(&s);
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let end_pos = if args.len() > 2 {
        (vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize).min(n)
    } else {
        n
    };
    NativeResult::Ok(JsValue::bool(s[..byte_index_at_char(&s, end_pos)].ends_with(&search)))
}

pub fn string_split<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.split called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        let parts = vec![s.clone()];
        return NativeResult::Ok(make_string_array(vm, &parts));
    }
    let sep_val = vm.reg(args[1]);
    let limit = if args.len() > 2 {
        vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as usize
    } else {
        usize::MAX
    };

    if is_regexp_obj(sep_val, vm) {
        let re_ptr = sep_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => {
                let parts = vec![s.clone()];
                return NativeResult::Ok(make_string_array(vm, &parts));
            }
        };
        // SAFETY: fn_ptr holds a Box<regex::Regex> pointer stored by regexp_constructor.
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
        let raw: Vec<&str> = regex.split(&s).collect();
        let parts: Vec<String> = raw.iter().map(|p| p.to_string()).take(limit).collect();
        return NativeResult::Ok(make_string_array(vm, &parts));
    }

    let sep = as_string(vm, sep_val);
    let parts: Vec<String> = if sep.is_empty() {
        s.chars().map(|c| c.to_string()).take(limit).collect()
    } else {
        s.split(&sep).map(|p| p.to_string()).take(limit).collect()
    };
    NativeResult::Ok(make_string_array(vm, &parts))
}

pub fn string_replace<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.replace called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        return NativeResult::Ok(vm.new_string(&s));
    }
    let pattern_val = vm.reg(args[1]);
    let replacement = if args.len() > 2 { as_string(vm, vm.reg(args[2])) } else { String::new() };

    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(vm.new_string(&s)),
        };
        // SAFETY: fn_ptr holds a Box<regex::Regex> pointer stored by regexp_constructor.
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
        let result = regex.replace_all(&s, replacement.as_str()).to_string();
        return NativeResult::Ok(vm.new_string(&result));
    }

    let pattern = as_string(vm, pattern_val);
    let result = s.replacen(&pattern, &replacement, 1);
    NativeResult::Ok(vm.new_string(&result))
}

pub fn string_match_fn<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.match called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::undefined());
    }
    let pattern_val = vm.reg(args[1]);

    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(JsValue::undefined()),
        };
        // SAFETY: fn_ptr holds a Box<regex::Regex> pointer stored by regexp_constructor.
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
        let matches: Vec<String> = regex.find_iter(&s).map(|m| m.as_str().to_string()).collect();
        return NativeResult::Ok(make_string_array(vm, &matches));
    }

    let pattern = as_string(vm, pattern_val);
    if let Some(pos) = s.find(&pattern) {
        return NativeResult::Ok(make_string_array(vm, &[s[pos..pos + pattern.len()].to_string()]));
    }
    NativeResult::Ok(JsValue::undefined())
}

pub fn string_search<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.search called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let pattern_val = vm.reg(args[1]);

    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(JsValue::int(-1)),
        };
        // SAFETY: fn_ptr holds a Box<regex::Regex> pointer stored by regexp_constructor.
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
        if let Some(m) = regex.find(&s) {
            return NativeResult::Ok(JsValue::int(m.start() as i32));
        }
        return NativeResult::Ok(JsValue::int(-1));
    }

    let pattern = as_string(vm, pattern_val);
    if let Some(pos) = s.find(&pattern) {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
}

pub fn string_trim_start<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trimStart called with {} args", args.len());
    let s = this_string(vm, args);
    NativeResult::Ok(vm.new_string(s.trim_start()))
}

pub fn string_trim_end<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.trimEnd called with {} args", args.len());
    let s = this_string(vm, args);
    NativeResult::Ok(vm.new_string(s.trim_end()))
}

pub fn string_code_point_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.codePointAt called with {} args", args.len());
    let s = this_string(vm, args);
    let pos = if args.len() > 1 {
        vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as usize
    } else {
        0
    };
    let chars: Vec<char> = s.chars().collect();
    if pos >= chars.len() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let c = chars[pos] as u32;
    if (0xD800..=0xDBFF).contains(&c) && pos + 1 < chars.len() {
        let next = chars[pos + 1] as u32;
        if (0xDC00..=0xDFFF).contains(&next) {
            let cp = 0x10000 + ((c - 0xD800) << 10) + (next - 0xDC00);
            return NativeResult::Ok(JsValue::int(cp as i32));
        }
    }
    NativeResult::Ok(JsValue::int(c as i32))
}

pub fn string_normalize<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.normalize called with {} args", args.len());
    use unicode_normalization::UnicodeNormalization;
    let s = this_string(vm, args);
    let form = if args.len() > 1 { as_string(vm, vm.reg(args[1])) } else { "NFC".to_string() };
    let result: String = match form.as_str() {
        "NFD" => s.nfd().collect(),
        "NFKC" => s.nfkc().collect(),
        "NFKD" => s.nfkd().collect(),
        _ => s.nfc().collect(),
    };
    NativeResult::Ok(vm.new_string(&result))
}

pub fn string_match_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.matchAll called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        builtins_error!("String.prototype.matchAll: invalid receiver");
        return NativeResult::Err(JsValue::undefined());
    }
    let pattern_val = vm.reg(args[1]);
    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(JsValue::undefined()),
        };
        // SAFETY: fn_ptr holds a Box<regex::Regex> pointer stored by regexp_constructor.
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
        let matches: Vec<String> = regex.find_iter(&s).map(|m| m.as_str().to_string()).collect();
        return NativeResult::Ok(make_string_array(vm, &matches));
    }
    NativeResult::Ok(JsValue::undefined())
}

pub fn string_replace_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.replaceAll called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        return NativeResult::Ok(vm.new_string(&s));
    }
    let pattern_val = vm.reg(args[1]);
    let replacement = if args.len() > 2 { as_string(vm, vm.reg(args[2])) } else { String::new() };
    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(vm.new_string(&s)),
        };
        // SAFETY: fn_ptr holds a Box<regex::Regex> pointer stored by regexp_constructor.
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
        let result = regex.replace_all(&s, replacement.as_str()).to_string();
        return NativeResult::Ok(vm.new_string(&result));
    }
    let pattern = as_string(vm, pattern_val);
    let result = s.replace(&pattern, &replacement);
    NativeResult::Ok(vm.new_string(&result))
}
