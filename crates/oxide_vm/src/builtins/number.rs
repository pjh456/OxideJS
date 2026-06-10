use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;

pub fn number_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let n = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref())
    } else {
        0.0
    };
    if n.fract() == 0.0 && n.is_finite() && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        NativeResult::Ok(JsValue::int(n as i32))
    } else {
        NativeResult::Ok(JsValue::float(n))
    }
}

pub fn number_is_nan(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let val = vm.reg(args[1]);
    if val.is_double() {
        NativeResult::Ok(JsValue::bool(val.as_double().is_nan()))
    } else {
        NativeResult::Ok(JsValue::bool(false))
    }
}

pub fn number_is_finite(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let val = vm.reg(args[1]);
    if val.is_int() {
        return NativeResult::Ok(JsValue::bool(true));
    }
    if val.is_double() {
        return NativeResult::Ok(JsValue::bool(val.as_double().is_finite()));
    }
    let n = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    NativeResult::Ok(JsValue::bool(n.is_finite()))
}

pub fn number_parse_int(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let sf = vm.kernel().string_forge().as_ref();
    let s = coercion::to_string(sf, vm.reg(args[1]));
    let s = s.trim();

    if s.is_empty() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }

    let radix = if args.len() > 2 {
        let r = coercion::to_number(vm.reg(args[2]), sf) as i32;
        if r == 0 {
            10
        } else {
            r.clamp(2, 36)
        }
    } else {
        10
    };

    let (rest, hex) = if s.starts_with("0x") || s.starts_with("0X") {
        if radix == 16 || radix == 0 || (args.len() <= 2) {
            (s[2..].to_string(), true)
        } else {
            (s.to_string(), false)
        }
    } else {
        (s.to_string(), false)
    };

    let actual_radix = if hex { 16u32 } else { radix as u32 };

    if let Ok(n) = i32::from_str_radix(&rest, actual_radix) {
        return NativeResult::Ok(JsValue::int(n));
    }

    NativeResult::Ok(JsValue::float(f64::NAN))
}

pub fn number_parse_float(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let sf = vm.kernel().string_forge().as_ref();
    let s = coercion::to_string(sf, vm.reg(args[1]));
    let s = s.trim();

    if s.is_empty() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }

    match fast_float::parse::<f64, _>(&s) {
        Ok(v) => NativeResult::Ok(JsValue::float(v)),
        Err(_) => NativeResult::Ok(JsValue::float(f64::NAN)),
    }
}

pub fn number_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let n = coercion::to_number(vm.reg(args[0]), vm.kernel().string_forge().as_ref());
    let radix = if args.len() > 1 {
        let r = coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as u32;
        r.clamp(2, 36)
    } else {
        10u32
    };

    if radix == 10 {
        if n.is_nan() {
            return NativeResult::Ok(vm.intern("NaN"));
        }
        if n.is_infinite() {
            if n.is_sign_positive() {
                return NativeResult::Ok(vm.intern("Infinity"));
            }
            return NativeResult::Ok(vm.intern("-Infinity"));
        }
        if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
            return NativeResult::Ok(vm.intern(&(n as i64).to_string()));
        }
        let mut buf = ryu::Buffer::new();
        NativeResult::Ok(vm.intern(buf.format(n)))
    } else {
        if n.is_nan() {
            return NativeResult::Ok(vm.intern("NaN"));
        }
        let nn = n as i64;
        let mut result = String::new();
        let mut value = nn.abs();
        if value == 0 {
            result.push('0');
        } else {
            let chars = "0123456789abcdefghijklmnopqrstuvwxyz";
            let mut digits = Vec::new();
            while value > 0 {
                digits.push(chars.as_bytes()[(value % radix as i64) as usize] as char);
                value /= radix as i64;
            }
            for ch in digits.iter().rev() {
                result.push(*ch);
            }
        }
        if nn < 0 {
            result.insert(0, '-');
        }
        NativeResult::Ok(vm.intern(&result))
    }
}

pub fn number_to_fixed(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let n = coercion::to_number(vm.reg(args[0]), vm.kernel().string_forge().as_ref());
    let digits = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as usize
    } else {
        0usize
    }
    .min(20);

    let formatted = format!("{:.digits$}", n);
    NativeResult::Ok(vm.intern(&formatted))
}

pub fn number_is_integer(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    let n = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    NativeResult::Ok(JsValue::bool(n.trunc() == n && n.is_finite()))
}

pub fn number_is_safe_integer(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    let n = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    let safe = n.trunc() == n && n.is_finite() && n >= -9007199254740991i64 as f64 && n <= 9007199254740991i64 as f64;
    NativeResult::Ok(JsValue::bool(safe))
}

pub fn number_to_exponential(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let n = coercion::to_number(vm.reg(args[0]), vm.kernel().string_forge().as_ref());
    let digits = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as usize
    } else {
        0usize
    }
    .min(100);
    if n.is_nan() {
        return NativeResult::Ok(vm.intern("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.intern(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    let formatted = format!("{:.digits$e}", n);
    NativeResult::Ok(vm.intern(&formatted))
}

pub fn number_to_precision(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let n = coercion::to_number(vm.reg(args[0]), vm.kernel().string_forge().as_ref());
    let precision = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as usize
    } else {
        0usize
    }
    .min(21);
    if n.is_nan() {
        return NativeResult::Ok(vm.intern("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.intern(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    let formatted = format!("{:.precision$}", n);
    NativeResult::Ok(vm.intern(&formatted))
}

pub fn number_value_of(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if this_val.is_int() || this_val.is_double() {
        return NativeResult::Ok(this_val);
    }
    NativeResult::Ok(JsValue::float(f64::NAN))
}
