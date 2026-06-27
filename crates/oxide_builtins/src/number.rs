use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

pub fn number_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = if args.len() > 1 {
        vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN)
    } else {
        0.0
    };
    let number_proto = vm.session().builtin_world().number_proto.as_ptr() as *mut oxide_types::object::JsObject;
    let is_ctor = if let Some(this_reg) = args.first().copied() {
        let this_val = vm.reg(this_reg);
        if this_val.is_object() {
            let ptr = this_val.as_js_object_ptr();
            if ptr.is_null() {
                false
            } else {
                let proto_ptr = unsafe { (*ptr).proto().as_js_object_ptr() };
                !proto_ptr.is_null() && std::ptr::eq(proto_ptr, number_proto)
            }
        } else {
            false
        }
    } else {
        false
    };

    if is_ctor {
        let this_val = vm.reg(args[0]);
        let obj = unsafe { &mut *this_val.as_js_object_ptr() };
        obj.type_tag = oxide_types::object::JsObject::OBJ_TYPE_NUMBER_OBJ;
        let boxed = if n.fract() == 0.0 && n.is_finite() && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
            JsValue::int(n as i32)
        } else {
            JsValue::float(n)
        };
        obj.set_prop_at(0, boxed);
        return NativeResult::Ok(this_val);
    }

    if n.fract() == 0.0 && n.is_finite() && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        NativeResult::Ok(JsValue::int(n as i32))
    } else {
        NativeResult::Ok(JsValue::float(n))
    }
}

pub fn number_is_nan<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(vm.reg(args[1]));
    NativeResult::Ok(JsValue::bool(n.is_nan()))
}

pub fn number_is_finite<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(vm.reg(args[1]));
    NativeResult::Ok(JsValue::bool(n.is_finite()))
}

pub fn number_parse_int<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let s = oxide_runtime_api::to_string(vm.reg(args[1]));
    let s = s.trim();

    if s.is_empty() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }

    let radix = if args.len() > 2 {
        let r = vm.coerce_number_bounded(vm.reg(args[2])).unwrap_or(f64::NAN) as i32;
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

pub fn number_parse_float<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let s = oxide_runtime_api::to_string(vm.reg(args[1]));
    let s = s.trim();

    if s.is_empty() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }

    match fast_float::parse::<f64, _>(&s) {
        Ok(v) => NativeResult::Ok(JsValue::float(v)),
        Err(_) => NativeResult::Ok(JsValue::float(f64::NAN)),
    }
}

pub fn number_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = vm.coerce_number_bounded(vm.reg(args[0])).unwrap_or(f64::NAN);
    let radix = if args.len() > 1 {
        let r = vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN) as u32;
        r.clamp(2, 36)
    } else {
        10u32
    };

    if radix == 10 {
        if n.is_nan() {
            return NativeResult::Ok(vm.new_string("NaN"));
        }
        if n.is_infinite() {
            if n.is_sign_positive() {
                return NativeResult::Ok(vm.new_string("Infinity"));
            }
            return NativeResult::Ok(vm.new_string("-Infinity"));
        }
        if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
            return NativeResult::Ok(vm.new_string(&(n as i64).to_string()));
        }
        let mut buf = ryu::Buffer::new();
        NativeResult::Ok(vm.new_string(buf.format(n)))
    } else {
        if n.is_nan() {
            return NativeResult::Ok(vm.new_string("NaN"));
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
        NativeResult::Ok(vm.new_string(&result))
    }
}

pub fn number_to_fixed<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = vm.coerce_number_bounded(vm.reg(args[0])).unwrap_or(f64::NAN);
    if n.is_nan() {
        return NativeResult::Ok(vm.new_string("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.new_string(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    let fraction_digits = if args.len() > 1 {
        let raw = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1]));
        if !(0.0..=100.0).contains(&raw) {
            return NativeResult::Err(crate::error::create_range_error(
                vm,
                "toFixed() fractionDigits must be between 0 and 100",
            ));
        }
        raw as usize
    } else {
        0usize
    };
    let n_abs = if n == 0.0 && n.is_sign_negative() { 0.0 } else { n };
    let formatted = format!("{:.precision$}", n_abs, precision = fraction_digits);
    NativeResult::Ok(vm.new_string(&formatted))
}

pub fn number_is_integer<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    if !val.is_int() && !val.is_double() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(val);
    NativeResult::Ok(JsValue::bool(n.trunc() == n && n.is_finite()))
}

pub fn number_is_safe_integer<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    if !val.is_int() && !val.is_double() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(val);
    let safe = n.trunc() == n && n.is_finite() && n >= -9007199254740991i64 as f64 && n <= 9007199254740991i64 as f64;
    NativeResult::Ok(JsValue::bool(safe))
}

pub fn number_to_precision<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() <= 1 {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "toPrecision() requires a precision argument between 1 and 100",
        ));
    }
    let n = vm.coerce_number_bounded(vm.reg(args[0])).unwrap_or(f64::NAN);
    let raw = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1]));
    if !(1.0..=100.0).contains(&raw) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "toPrecision() precision must be between 1 and 100",
        ));
    }
    let precision = raw as usize;
    if n.is_nan() {
        return NativeResult::Ok(vm.new_string("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.new_string(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    let n_abs = if n == 0.0 && n.is_sign_negative() { 0.0 } else { n.abs() };
    let is_neg = n.is_sign_negative() && !(n == 0.0);
    let e = if n_abs == 0.0 { 0i32 } else { n_abs.log10().floor() as i32 };
    let formatted = if e >= -6 && e < precision as i32 {
        let dec = ((precision as i32 - e - 1).max(0)) as usize;
        format!("{:.dec$}", n_abs, dec = dec)
    } else {
        format!("{:.dec$e}", n_abs, dec = precision - 1)
    };
    if is_neg {
        NativeResult::Ok(vm.new_string(&format!("-{}", formatted)))
    } else {
        NativeResult::Ok(vm.new_string(&formatted))
    }
}

pub fn number_to_exponential<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = vm.coerce_number_bounded(vm.reg(args[0])).unwrap_or(f64::NAN);
    if n.is_nan() {
        return NativeResult::Ok(vm.new_string("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.new_string(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    let n_abs = if n == 0.0 && n.is_sign_negative() { 0.0 } else { n.abs() };
    let sign_prefix = if n.is_sign_negative() && !n.is_nan() && !(n == 0.0) { "-" } else { "" };
    if args.len() <= 1 {
        let formatted = format!("{:e}", n_abs);
        let mut s = formatted;
        if let Some(e_pos) = s.find('e') {
            let mut mantissa = s[..e_pos].to_string();
            while mantissa.ends_with('0') && mantissa.len() > 1 {
                mantissa.pop();
            }
            mantissa = mantissa.trim_end_matches('.').to_string();
            if mantissa.contains('.') {
                s = format!("{}{}", mantissa, &s[e_pos..]);
            }
        }
        let result = format!("{}{}", sign_prefix, s);
        return NativeResult::Ok(vm.new_string(&result));
    }
    let raw = oxide_runtime_api::to_integer_or_infinity(vm.reg(args[1]));
    if !(0.0..=100.0).contains(&raw) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "toExponential() fractionDigits must be between 0 and 100",
        ));
    }
    let fraction_digits = raw as usize;
    let formatted = format!("{:.digits$e}", n_abs, digits = fraction_digits);
    NativeResult::Ok(vm.new_string(&format!("{}{}", sign_prefix, formatted)))
}

pub fn number_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if this_val.is_int() || this_val.is_double() {
        return NativeResult::Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_number_obj() {
                return NativeResult::Ok(obj.get_prop_at(0));
            }
        }
    }
    NativeResult::Err(crate::error::create_type_error(
        vm,
        "Number.prototype.valueOf called on incompatible receiver",
    ))
}
