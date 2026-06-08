use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;
use memchr::memchr;

fn this_string(vm: &Vm, args: &[u8]) -> String {
    coercion::to_string(vm.kernel().string_forge().as_ref(), vm.reg(args[0]))
}

fn make_string_array(vm: &mut Vm, parts: &[String]) -> JsValue {
    let proto = vm.kernel().builtin_world().array_proto.as_ptr() as *mut JsObject;
    let n = parts.len().min(31);
    let arr = vm.epoch().alloc(JsObject::new_array(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(proto),
        n,
        vm.epoch().bump(),
    ));
    unsafe {
        for (i, s) in parts.iter().enumerate().take(n) {
            let sv = vm.intern(s);
            (*arr).set_prop_at(i as u8, sv);
        }
        (*arr).set_prop_count(n as u8);
    }
    JsValue::from_js_object(arr)
}

fn as_string(vm: &Vm, val: JsValue) -> String {
    coercion::to_string(vm.kernel().string_forge().as_ref(), val)
}

fn is_regexp_obj(val: JsValue, vm: &Vm) -> bool {
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
    let rp = vm.kernel().builtin_world().regexp_proto.as_ptr() as *mut JsObject;
    std::ptr::eq(proto_ptr, rp)
}

pub fn string_index_of(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(JsValue::int(-1));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let pos = if args.len() > 2 {
        (coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as usize)
            .min(s.len())
    } else {
        0
    };

    if search.is_empty() {
        return Ok(JsValue::int(pos as i32));
    }

    let haystack = &s.as_bytes()[pos..];
    if search.len() == 1 {
        if let Some(idx) = memchr(search.as_bytes()[0], haystack) {
            return Ok(JsValue::int((pos + idx) as i32));
        }
    } else if let Some(idx) = s[pos..].find(&search) {
        return Ok(JsValue::int((pos + idx) as i32));
    }
    Ok(JsValue::int(-1))
}

pub fn string_includes(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(JsValue::bool(false));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let pos = if args.len() > 2 {
        (coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as usize)
            .min(s.len())
    } else {
        0
    };

    if search.is_empty() {
        return Ok(JsValue::bool(true));
    }

    let haystack = &s.as_bytes()[pos..];
    if search.len() == 1 {
        Ok(JsValue::bool(
            memchr(search.as_bytes()[0], haystack).is_some(),
        ))
    } else {
        Ok(JsValue::bool(s[pos..].contains(&search)))
    }
}

pub fn string_char_at(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        if s.is_empty() {
            return Ok(vm.intern(""));
        }
        return Ok(vm.intern(&s[0..1]));
    }
    let idx = coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32;
    if idx < 0 || idx as usize >= s.len() {
        return Ok(vm.intern(""));
    }
    let ch = &s[(idx as usize)..(idx as usize + 1)];
    Ok(vm.intern(ch))
}

pub fn string_char_code_at(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        if s.is_empty() {
            return Ok(JsValue::float(f64::NAN));
        }
        return Ok(JsValue::int(s.as_bytes()[0] as i32));
    }
    let idx = coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32;
    if idx < 0 || idx as usize >= s.len() {
        return Ok(JsValue::float(f64::NAN));
    }
    Ok(JsValue::int(s.as_bytes()[idx as usize] as i32))
}

pub fn string_concat(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let mut result = this_string(vm, args);
    let sf = vm.kernel().string_forge().as_ref();
    for &arg_reg in args.iter().skip(1) {
        result.push_str(&coercion::to_string(sf, vm.reg(arg_reg)));
    }
    Ok(vm.intern(&result))
}

pub fn string_slice(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    let n = s.len() as i32;
    let start = if args.len() > 1 {
        let v = coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32;
        if v < 0 {
            (n + v).max(0)
        } else {
            v.min(n)
        }
    } else {
        0
    };
    let end = if args.len() > 2 {
        let v = coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as i32;
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
    let result = if start < end { &s[start..end] } else { "" };
    Ok(vm.intern(result))
}

pub fn string_substring(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    let n = s.len() as i32;
    let mut start = if args.len() > 1 {
        (coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32)
            .max(0)
            .min(n)
    } else {
        0
    };
    let mut end = if args.len() > 2 {
        (coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as i32)
            .max(0)
            .min(n)
    } else {
        n
    };
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    let result = &s[start as usize..end as usize];
    Ok(vm.intern(result))
}

pub fn string_to_upper_case(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    Ok(vm.intern(&s.to_uppercase()))
}

pub fn string_to_lower_case(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    Ok(vm.intern(&s.to_lowercase()))
}

pub fn string_trim(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    Ok(vm.intern(s.trim()))
}

pub fn string_repeat(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    let n = if args.len() > 1 {
        (coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as usize)
            .min(10000)
    } else {
        1
    };
    Ok(vm.intern(&s.repeat(n)))
}

pub fn string_pad_start(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    let target = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as usize
    } else {
        s.len()
    };
    let pad = if args.len() > 2 {
        as_string(vm, vm.reg(args[2]))
    } else {
        " ".to_string()
    };
    if s.len() >= target || pad.is_empty() {
        return Ok(vm.intern(&s));
    }
    let needed = target - s.len();
    let reps = needed.div_ceil(pad.len());
    let mut out = pad.repeat(reps);
    out.truncate(needed);
    out.push_str(&s);
    Ok(vm.intern(&out))
}

pub fn string_pad_end(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    let target = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as usize
    } else {
        s.len()
    };
    let pad = if args.len() > 2 {
        as_string(vm, vm.reg(args[2]))
    } else {
        " ".to_string()
    };
    if s.len() >= target || pad.is_empty() {
        return Ok(vm.intern(&s));
    }
    let needed = target - s.len();
    let reps = needed.div_ceil(pad.len());
    let mut out = s;
    let pad_repeated = pad.repeat(reps);
    out.push_str(&pad_repeated[..needed]);
    Ok(vm.intern(&out))
}

pub fn string_starts_with(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(JsValue::bool(false));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let pos = if args.len() > 2 {
        (coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as usize)
            .min(s.len())
    } else {
        0
    };
    Ok(JsValue::bool(s[pos..].starts_with(&search)))
}

pub fn string_ends_with(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(JsValue::bool(false));
    }
    let search = as_string(vm, vm.reg(args[1]));
    let end_pos = if args.len() > 2 {
        (coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as usize)
            .min(s.len())
    } else {
        s.len()
    };
    Ok(JsValue::bool(s[..end_pos].ends_with(&search)))
}

pub fn string_split(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        let parts = vec![s.clone()];
        return Ok(make_string_array(vm, &parts));
    }
    let sep_val = vm.reg(args[1]);
    let limit = if args.len() > 2 {
        coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as usize
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
                return Ok(make_string_array(vm, &parts));
            }
        };
        let regex = unsafe { &*(fn_ptr as *const regex::Regex) };
        let raw: Vec<&str> = regex.split(&s).collect();
        let parts: Vec<String> = raw.iter().map(|p| p.to_string()).take(limit).collect();
        return Ok(make_string_array(vm, &parts));
    }

    let sep = as_string(vm, sep_val);
    let parts: Vec<String> = if sep.is_empty() {
        s.chars().map(|c| c.to_string()).take(limit).collect()
    } else {
        s.split(&sep).map(|p| p.to_string()).take(limit).collect()
    };
    Ok(make_string_array(vm, &parts))
}

pub fn string_replace(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(vm.intern(&s));
    }
    let pattern_val = vm.reg(args[1]);
    let replacement = if args.len() > 2 {
        as_string(vm, vm.reg(args[2]))
    } else {
        String::new()
    };

    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return Ok(vm.intern(&s)),
        };
        let regex = unsafe { &*(fn_ptr as *const regex::Regex) };
        let result = regex.replace_all(&s, replacement.as_str()).to_string();
        return Ok(vm.intern(&result));
    }

    let pattern = as_string(vm, pattern_val);
    let result = s.replacen(&pattern, &replacement, 1);
    Ok(vm.intern(&result))
}

pub fn string_match_fn(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(JsValue::undefined());
    }
    let pattern_val = vm.reg(args[1]);

    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return Ok(JsValue::undefined()),
        };
        let regex = unsafe { &*(fn_ptr as *const regex::Regex) };
        let matches: Vec<String> = regex.find_iter(&s).map(|m| m.as_str().to_string()).collect();
        return Ok(make_string_array(vm, &matches));
    }

    let pattern = as_string(vm, pattern_val);
    if let Some(pos) = s.find(&pattern) {
        return Ok(make_string_array(
            vm,
            &[s[pos..pos + pattern.len()].to_string()],
        ));
    }
    Ok(JsValue::undefined())
}

pub fn string_search(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(JsValue::int(-1));
    }
    let pattern_val = vm.reg(args[1]);

    if is_regexp_obj(pattern_val, vm) {
        let re_ptr = pattern_val.as_js_object_ptr();
        let re = unsafe { &*re_ptr };
        let fn_ptr = match re.native_fn() {
            Some(p) => p,
            None => return Ok(JsValue::int(-1)),
        };
        let regex = unsafe { &*(fn_ptr as *const regex::Regex) };
        if let Some(m) = regex.find(&s) {
            return Ok(JsValue::int(m.start() as i32));
        }
        return Ok(JsValue::int(-1));
    }

    let pattern = as_string(vm, pattern_val);
    if let Some(pos) = s.find(&pattern) {
        return Ok(JsValue::int(pos as i32));
    }
    Ok(JsValue::int(-1))
}

pub fn string_trim_start(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    Ok(vm.intern(s.trim_start()))
}

pub fn string_trim_end(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    Ok(vm.intern(s.trim_end()))
}

pub fn string_code_point_at(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    let pos = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as usize
    } else { 0 };
    let chars: Vec<char> = s.chars().collect();
    if pos >= chars.len() { return Ok(JsValue::undefined()); }
    let c = chars[pos] as u32;
    if (0xD800..=0xDBFF).contains(&c) && pos + 1 < chars.len() {
        let next = chars[pos + 1] as u32;
        if (0xDC00..=0xDFFF).contains(&next) {
            let cp = 0x10000 + ((c - 0xD800) << 10) + (next - 0xDC00);
            return Ok(JsValue::int(cp as i32));
        }
    }
    Ok(JsValue::int(c as i32))
}

pub fn string_normalize(vm: &mut Vm, args: &[u8]) -> NativeResult {
    use unicode_normalization::UnicodeNormalization;
    let s = this_string(vm, args);
    let form = if args.len() > 1 { as_string(vm, vm.reg(args[1])) } else { "NFC".to_string() };
    let result: String = match form.as_str() {
        "NFD" => s.nfd().collect(),
        "NFKC" => s.nfkc().collect(),
        "NFKD" => s.nfkd().collect(),
        _ => s.nfc().collect(),
    };
    Ok(vm.intern(&result))
}
