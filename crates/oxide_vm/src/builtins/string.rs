use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;
use memchr::memchr;
use regex::Regex;

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
    let sep = as_string(vm, vm.reg(args[1]));
    let limit = if args.len() > 2 {
        coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as usize
    } else {
        usize::MAX
    };

    let parts: Vec<String>;
    if sep.starts_with('/') && sep.len() > 2 && sep.ends_with('/') {
        let pattern = &sep[1..sep.len() - 1];
        if let Ok(re) = Regex::new(pattern) {
            let raw: Vec<&str> = re.split(&s).collect();
            parts = raw.iter().map(|p| p.to_string()).take(limit).collect();
        } else {
            parts = vec![s.clone()];
        }
    } else if sep.is_empty() {
        parts = s.chars().map(|c| c.to_string()).take(limit).collect();
    } else {
        parts = s.split(&sep).map(|p| p.to_string()).take(limit).collect();
    }
    Ok(make_string_array(vm, &parts))
}

pub fn string_replace(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(vm.intern(&s));
    }
    let pattern = as_string(vm, vm.reg(args[1]));
    let replacement = if args.len() > 2 {
        as_string(vm, vm.reg(args[2]))
    } else {
        String::new()
    };

    let result = if pattern.starts_with('/') && pattern.len() > 2 && pattern.ends_with('/') {
        let pat = &pattern[1..pattern.len() - 1];
        if let Ok(re) = Regex::new(pat) {
            re.replace_all(&s, replacement.as_str()).to_string()
        } else {
            s.clone()
        }
    } else {
        s.replacen(&pattern, &replacement, 1)
    };
    Ok(vm.intern(&result))
}

pub fn string_match_fn(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let s = this_string(vm, args);
    if args.len() < 2 {
        return Ok(JsValue::undefined());
    }
    let pattern = as_string(vm, vm.reg(args[1]));
    if pattern.starts_with('/') && pattern.len() > 2 && pattern.ends_with('/') {
        let pat = &pattern[1..pattern.len() - 1];
        if let Ok(re) = Regex::new(pat) {
            let matches: Vec<String> = re.find_iter(&s).map(|m| m.as_str().to_string()).collect();
            return Ok(make_string_array(vm, &matches));
        }
    } else if let Some(pos) = s.find(&pattern) {
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
    let pattern = as_string(vm, vm.reg(args[1]));
    if pattern.starts_with('/') && pattern.len() > 2 && pattern.ends_with('/') {
        let pat = &pattern[1..pattern.len() - 1];
        if let Ok(re) = Regex::new(pat) {
            if let Some(m) = re.find(&s) {
                return Ok(JsValue::int(m.start() as i32));
            }
        }
    } else if let Some(pos) = s.find(&pattern) {
        return Ok(JsValue::int(pos as i32));
    }
    Ok(JsValue::int(-1))
}
