use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use memchr::memchr;
use oxide_runtime_api::{NativeResult, VmHost};

use crate::builtins_debug;
use crate::builtins_error;

fn this_string<H: VmHost>(vm: &H, args: &[u8]) -> String {
    let this_val = vm.reg(args[0]);
    if this_val.is_null() || this_val.is_undefined() {
        String::new() // caller should check and throw TypeError
    } else {
        oxide_runtime_api::to_string(this_val)
    }
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

fn as_string<H: VmHost>(vm: &mut H, val: JsValue) -> String {
    oxide_runtime_api::to_string_full(val, vm).unwrap_or_else(|_| oxide_runtime_api::to_string(val))
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
        let v = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1]));
        if v.is_nan() || v < 0.0 {
            0
        } else {
            (v as i32).min(n)
        }
    } else {
        0
    };
    let mut end = if args.len() > 2 {
        let v = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2]));
        if v.is_nan() || v < 0.0 {
            0
        } else {
            (v as i32).min(n)
        }
    } else {
        n
    };
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    let result = char_slice(&s, start as usize, end as usize);
    NativeResult::Ok(vm.new_string(result))
}

pub fn string_substr<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.substr called with {} args", args.len());
    let s = this_string(vm, args);
    let len = char_len(&s) as isize;
    let start = if args.len() > 1 {
        let n = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1])) as isize;
        if n < 0 {
            (len + n).max(0)
        } else {
            n.min(len)
        }
    } else {
        0
    } as usize;
    let length = if args.len() > 2 {
        (oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2])) as isize).max(0) as usize
    } else {
        len as usize - start
    };
    let result = take_chars(&s[byte_index_at_char(&s, start)..], length.min(len as usize - start));
    NativeResult::Ok(vm.new_string(&result))
}

pub fn string_at<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.at called with {} args", args.len());
    let s = this_string(vm, args);
    let len = char_len(&s) as i32;
    let idx = if args.len() > 1 {
        oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1])) as i32
    } else {
        0
    };
    let idx = if idx < 0 { len + idx } else { idx };
    if idx < 0 || idx >= len {
        return NativeResult::Ok(JsValue::undefined());
    }
    let ch = s.chars().nth(idx as usize).unwrap().to_string();
    NativeResult::Ok(vm.new_string(&ch))
}

pub fn string_last_index_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.lastIndexOf called with {} args", args.len());
    let s = this_string(vm, args);
    let n = char_len(&s);
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::int(-1));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let pos = if args.len() > 2 {
        let p = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2]));
        if p.is_nan() {
            n
        } else {
            (p as usize).min(n)
        }
    } else {
        n
    };

    if search.is_empty() {
        return NativeResult::Ok(JsValue::int(pos as i32));
    }

    let end_byte = byte_index_at_char(&s, (pos + 1).min(n));
    let haystack = &s[..end_byte];

    if search.len() == 1 {
        if let Some(idx) = memchr::memrchr(search.as_bytes()[0], haystack.as_bytes()) {
            let result = char_len(&s[..idx]);
            return NativeResult::Ok(JsValue::int(result as i32));
        }
    } else if let Some(idx) = haystack.rfind(&search) {
        let result = char_len(&s[..idx]);
        return NativeResult::Ok(JsValue::int(result as i32));
    }
    NativeResult::Ok(JsValue::int(-1))
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
    // Spec: if separator is undefined, return [string]
    if args.len() < 2 || vm.reg(args[1]).is_undefined() {
        let parts = vec![s.clone()];
        return NativeResult::Ok(make_string_array(vm, &parts));
    }
    let sep_val = vm.reg(args[1]);
    // ToUint32(limit), default 2^32-1
    let limit = if args.len() > 2 {
        let l = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[2]));
        if l.is_infinite() { u32::MAX as usize } else { (l.max(0.0).trunc() as u64).min(u32::MAX as u64) as usize }
    } else {
        u32::MAX as usize
    };

    if is_regexp_obj(sep_val, vm) {
        let re_ptr = sep_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        if let Some(fn_ptr) = re.native_fn() {
            let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
            let mut parts: Vec<String> = Vec::new();
            let mut last_end = 0;
            for caps in regex.captures_iter(&s) {
                if parts.len() >= limit { break; }
                let full = caps.get(0).unwrap();
                parts.push(s[last_end..full.start()].to_string());
                if parts.len() >= limit { break; }
                for i in 1..caps.len() {
                    if parts.len() >= limit { break; }
                    parts.push(caps.get(i).map(|m| m.as_str()).unwrap_or("").to_string());
                }
                last_end = full.end();
            }
            if last_end <= s.len() && parts.len() < limit {
                parts.push(s[last_end..].to_string());
            }
            return NativeResult::Ok(make_string_array(vm, &parts));
        }
        // Fall back to string path for regex-like objects without native engine regex
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

    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(vm.new_string(&s)),
        };
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };

        if args.len() > 2 {
            let replacer_val = vm.reg(args[2]);
            if replacer_val.is_object() {
                let o = unsafe { &*replacer_val.as_js_object_ptr() };
                if o.is_function() {
                    builtins_debug!("string_replace: function replacer path");
                    if let Some(captures) = regex.captures(&s) {
                        let full = captures.get(0).unwrap();
                        let mut cb_args: Vec<JsValue> = Vec::with_capacity(captures.len() + 2);
                        cb_args.push(vm.new_string(full.as_str()));
                        for i in 1..captures.len() {
                            cb_args.push(vm.new_string(captures.get(i).map(|m| m.as_str()).unwrap_or("")));
                        }
                        cb_args.push(JsValue::int(full.start() as i32));
                        cb_args.push(vm.new_string(&s));
                        match vm.call_function_sync(replacer_val, JsValue::undefined(), &cb_args) {
                            Ok(result) => {
                                let result_str = oxide_runtime_api::to_string(result);
                                let output = format!("{}{}{}", &s[..full.start()], &result_str, &s[full.end()..]);
                                return NativeResult::Ok(vm.new_string(&output));
                            }
                            Err(err) => {
                                return NativeResult::Err(crate::error::create_type_error(
                                    vm,
                                    &format!("replace replacer: {}", err),
                                ));
                            }
                        }
                    }
                    return NativeResult::Ok(vm.new_string(&s));
                }
            }
        }

        let replacement = if args.len() > 2 { as_string(vm, vm.reg(args[2])) } else { String::new() };
        let is_global = re.hash_props_vec().and_then(|v| v.get(3)).map(|v| v.as_bool()).unwrap_or(false);
        let result = if is_global {
            regex.replace_all(&s, replacement.as_str()).to_string()
        } else {
            regex.replacen(&s, 1, replacement.as_str()).to_string()
        };
        return NativeResult::Ok(vm.new_string(&result));
    }

    let replacement = if args.len() > 2 { as_string(vm, vm.reg(args[2])) } else { String::new() };
    let pattern = as_string(vm, pattern_val);
    let result = s.replacen(&pattern, &replacement, 1);
    NativeResult::Ok(vm.new_string(&result))
}

pub fn string_match_fn<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.match called with {} args", args.len());
    let s = this_string(vm, args);
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::null());
    }
    let pattern_val = vm.reg(args[1]);

    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return NativeResult::Ok(JsValue::null()),
        };
        let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };
        let is_global = re.hash_props_vec().and_then(|v| v.get(3)).map(|v| v.as_bool()).unwrap_or(false);
        if is_global {
            let matches: Vec<String> = regex.find_iter(&s).map(|m| m.as_str().to_string()).collect();
            return NativeResult::Ok(make_string_array(vm, &matches));
        }
        if let Some(captures) = regex.captures(&s) {
            let mut parts: Vec<String> = Vec::with_capacity(captures.len());
            parts.push(captures.get(0).unwrap().as_str().to_string());
            for i in 1..captures.len() {
                parts.push(captures.get(i).map(|m| m.as_str()).unwrap_or("").to_string());
            }
            return NativeResult::Ok(make_string_array(vm, &parts));
        }
        return NativeResult::Ok(JsValue::null());
    }

    let pattern = as_string(vm, pattern_val);
    if let Some(pos) = s.find(&pattern) {
        return NativeResult::Ok(make_string_array(vm, &[s[pos..pos + pattern.len()].to_string()]));
    }
    NativeResult::Ok(JsValue::null())
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

const MALL_INPUT: &str = "__mal_input__";
const MALL_INDEX: &str = "__mal_index__";
const MALL_RE: &str = "__mal_re__";

pub fn string_match_all<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    builtins_debug!("String.prototype.matchAll called with {} args", args.len());
    let s = this_string(vm, args);
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
        let pattern_str = as_string(vm, pattern_val);
        let escaped = regex::escape(&pattern_str);
        let rx_str = if escaped.is_empty() { String::from("(?:)") } else { format!("({})", escaped) };
        let compiled = match regex::Regex::new(&rx_str) {
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

    // Build wrapper object
    builder_wrapper(vm, &s, re_obj)
}

fn builder_wrapper<H: VmHost>(vm: &mut H, input: &str, re_obj: JsValue) -> NativeResult {
    let object_proto = vm.session().builtin_world().object_proto.as_ptr() as *mut JsObject;
    let wrapper = vm
        .epoch()
        .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));

    let wrapper_obj = unsafe { &mut *wrapper };
    let input_si = vm.kernel_core().perm_interner().intern(MALL_INPUT).0;
    let index_si = vm.kernel_core().perm_interner().intern(MALL_INDEX).0;
    let re_si = vm.kernel_core().perm_interner().intern(MALL_RE).0;
    let next_si = vm.kernel_core().perm_interner().intern("next").0;

    let input_val = vm.new_string(input);
    vm.set_or_create_prop_value(wrapper_obj, input_si, input_val);
    vm.set_or_create_prop_value(wrapper_obj, index_si, JsValue::int(0));
    vm.set_or_create_prop_value(wrapper_obj, re_si, re_obj);

    let next_fn = make_public_native_fn(vm, "next", string_match_all_next::<H> as *const (), 0);
    vm.set_or_create_prop_value(wrapper_obj, next_si, next_fn);

    NativeResult::Ok(JsValue::from_js_object(wrapper))
}

fn make_public_native_fn<H: VmHost>(vm: &mut H, name: &str, native_fn: *const (), arg_count: u8) -> JsValue {
    let function_proto = vm.session().builtin_world().function_proto.as_ptr() as *mut JsObject;
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto));
    func.set_function(true);
    func.set_native_fn(Some(unsafe { oxide_types::object::NativeFnPtr::from_raw(native_fn) }));
    func.set_native_arg_count(arg_count);
    let func = vm.alloc_object(func);
    let name_si = vm.kernel_core().perm_interner().intern("name").0;
    let value = vm.new_string(name);
    let func_ref = unsafe { &mut *func };
    vm.set_or_create_prop_value(func_ref, name_si, value);
    JsValue::from_js_object(func)
}

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
    let regex = unsafe { &*(fn_ptr.as_ptr() as *const regex::Regex) };

    if let Some(m) = regex.find_at(&input_str, idx) {
        let mut parts: Vec<String> = Vec::new();
        if let Some(caps) = regex.captures_at(&input_str, idx) {
            parts.push(caps.get(0).unwrap().as_str().to_string());
            for i in 1..caps.len() {
                parts.push(caps.get(i).map(|c| c.as_str()).unwrap_or("").to_string());
            }
        }
        idx = m.end();
        vm.set_or_create_prop_value(wrapper, index_si, JsValue::int(idx as i32));
        let arr_val = make_string_array(vm, &parts);
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
