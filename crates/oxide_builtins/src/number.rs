use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

/// JS `Number()` 构造逻辑：把参数按 ToNumber 语义转换。
/// 普通调用返回原始 number（整数走 int 表示）；new 语义返回 `[[NumberData]]` 包装对象。
pub fn number_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = if args.len() > 1 {
        match vm.coerce_number_bounded(vm.reg(args[1])) {
            Ok(n) => n,
            Err(_) => {
                // ToNumber on an object may throw via toString/valueOf; propagate the original exception.
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
            }
        }
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

/// `Number.isNaN`：参数严格等于 NaN 才返回 true（不做隐式类型转换）。
pub fn number_is_nan<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(vm.reg(args[1]));
    NativeResult::Ok(JsValue::bool(n.is_nan()))
}

/// `Number.isFinite`：参数为有限数才返回 true（不做隐式类型转换）。
pub fn number_is_finite<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(vm.reg(args[1]));
    NativeResult::Ok(JsValue::bool(n.is_finite()))
}

/// ECMA-262 WhiteSpace / LineTerminator 判定（TrimString 用）。
///
/// 与 Rust `char::is_whitespace` 的差异：规范集合不含 U+0085（NEL），手工
/// 按白名单匹配避免误剥。
fn is_js_ws(c: char) -> bool {
    matches!(
        c,
        '\u{0009}' | '\u{000B}' | '\u{000C}' | '\u{0020}' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
                | '\u{000A}'
                | '\u{000D}'
                | '\u{2028}'
                | '\u{2029}'
    )
}

/// ECMA-262 ToInt32（§7.1.6）：f64 → mod 2^32 回绕的有符号 i32。
///
/// NaN/±0/±∞ 归 0；先截断再对 2^32 取余（余数非负），保证超大输入
/// （如 2^40 → 0）按回绕而非饱和。
fn to_int32(n: f64) -> i32 {
    if n.is_nan() || n.is_infinite() || n == 0.0 {
        return 0;
    }
    (n.trunc().rem_euclid(4294967296.0) as u32) as i32
}

/// `parseInt(string, radix)`：按 ECMA-262 §19.2.5 前缀解析。
///
/// 规范白名单 trim 后读 `+`/`-` 符号；radix 经 ToInt32（mod 2^32 回绕，
/// NaN/undefined → 0），R≠0 且不在 [2,36] 返回 NaN；仅当原 R 为 0 或 16 时
/// `0x`/`0X` 前缀按十六进制剥除。随后按进制收集最长连续有效数字前缀并转
/// f64（十进制正确舍入、其余进制数学累加，均允许超 2^53 舍入），无有效
/// 数字返回 NaN，结果在 i32 域内用 int 表示，`-0` 保留负零。
pub fn number_parse_int<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    // 参数按 ToString 完整转换：对象经 ToPrimitive(string hint)，Symbol 抛
    // TypeError；对象方法抛出的原生异常原样传播。
    let s = match oxide_runtime_api::to_string_full(vm.reg(args[1]), vm) {
        Ok(s) => s,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    let s = s.trim_start_matches(is_js_ws).trim_end_matches(is_js_ws);

    // radix 经 ToInt32：缺省参数视为 undefined（ToInt32 → 0）。
    let raw = if args.len() > 2 {
        match vm.coerce_number_bounded(vm.reg(args[2])) {
            Ok(n) => n,
            Err(_) => {
                // coercion 触发用户 valueOf/toString 抛出的异常经
                // last_uncaught_value 恢复后原样重新抛出。
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert radix to a number"));
            }
        }
    } else {
        f64::NAN
    };
    let r = to_int32(raw);

    // 读符号并跳过；R≠0 且越出 [2,36] 直接 NaN（保留 0 参与后续 0x 判定）。
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let radix_default: u32 = if r == 0 {
        10
    } else if (2..=36).contains(&r) {
        r as u32
    } else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };

    // 0x/0X 前缀：仅当原 R 为 0（缺省/undefined/NaN/0）或 16 时剥前缀转十六进制。
    let (digits, radix) = if (r == 0 || r == 16)
        && rest.len() >= 2
        && rest.as_bytes()[0] == b'0'
        && (rest.as_bytes()[1] == b'x' || rest.as_bytes()[1] == b'X')
    {
        (&rest[2..], 16u32)
    } else {
        (rest, radix_default)
    };

    // 按进制收集最长连续有效数字前缀；十进制交 Rust 正确舍入解析
    // （逐位 f64 累加对 20+ 位数字会产生 1 ulp 级偏差），其余进制
    // 数学累加（2 的幂进制在 53 位内精确，超出按规范允许近似舍入）。
    let mut end = 0usize;
    for (i, c) in digits.char_indices() {
        if c.to_digit(radix).is_none() {
            break;
        }
        end = i + c.len_utf8();
    }
    if end == 0 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let prefix = &digits[..end];
    let acc = if radix == 10 {
        match prefix.parse::<f64>() {
            Ok(v) => v,
            Err(_) => return NativeResult::Ok(JsValue::float(f64::NAN)),
        }
    } else {
        let mut acc = 0.0f64;
        for c in prefix.chars() {
            acc = acc * radix as f64 + c.to_digit(radix).unwrap() as f64;
        }
        acc
    };

    let acc = if neg { -acc } else { acc };
    if acc == 0.0 {
        // 负零保留符号（parseInt("-0") → -0）。
        return NativeResult::Ok(JsValue::float(if neg { -0.0 } else { 0.0 }));
    }
    if acc.fract() == 0.0 && acc >= i32::MIN as f64 && acc <= i32::MAX as f64 {
        NativeResult::Ok(JsValue::int(acc as i32))
    } else {
        NativeResult::Ok(JsValue::float(acc))
    }
}

/// `parseFloat(string)`：按 ECMA-262 §19.2.4 前缀解析。
///
/// 规范白名单 trim 后读 `+`/`-` 符号，特判精确大小写的 `Infinity`；随后按
/// StrDecimalLiteral 文法扫描最长合法十进制前缀（整数 + 可选小数 + 可选
/// 指数，`0x10` 在 'x' 处停止得 0，`1.2.3` 得 1.2），前缀子串交给
/// fast_float 解析（溢出归 ±Infinity），无合法前缀返回 NaN。
pub fn number_parse_float<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    // 参数按 ToString 完整转换：对象经 ToPrimitive(string hint)，Symbol 抛
    // TypeError；对象方法抛出的原生异常原样传播。
    let s = match oxide_runtime_api::to_string_full(vm.reg(args[1]), vm) {
        Ok(s) => s,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    let s = s.trim_start_matches(is_js_ws).trim_end_matches(is_js_ws);

    // 读符号；`Infinity` 大小写敏感，其后可带任意后缀（取最长合法前缀）。
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if rest.starts_with("Infinity") {
        return NativeResult::Ok(JsValue::float(if neg { f64::NEG_INFINITY } else { f64::INFINITY }));
    }

    // 扫描 mantissa：数字与至多一个 '.'（'.' 后必须有数字，整数部分可为空）。
    let b = rest.as_bytes();
    let mut i = 0usize;
    let mut dot = false;
    let mut mantissa_digits = 0usize;
    let mut end = 0usize;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() {
            mantissa_digits += 1;
            end = i + 1;
            i += 1;
        } else if c == b'.' && !dot {
            dot = true;
            i += 1;
        } else {
            break;
        }
    }
    // 指数部分：e/E [+/-] 数字，且 mantissa 必须先有数字；指数无数字则不含 'e'。
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') && mantissa_digits > 0 {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            end = j;
        }
    }
    if mantissa_digits == 0 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }

    match fast_float::parse::<f64, _>(&rest[..end]) {
        Ok(v) => NativeResult::Ok(JsValue::float(if neg { -v } else { v })),
        Err(_) => NativeResult::Ok(JsValue::float(f64::NAN)),
    }
}

/// `Number.prototype.toString(radix)`：按指定进制（2..36）转字符串。
///
/// 十进制走共享的 ECMA-262 Number::toString 格式化；非十进制对截断后的整数
/// 部分做进制转换（小数部分按近似处理）。radix 经 ToInteger 后越界抛 RangeError，
/// NaN/Infinity 输出专名。
pub fn number_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let n = vm.coerce_number_bounded(vm.reg(args[0])).unwrap_or(f64::NAN);
    let radix = if args.len() > 1 {
        let radix_arg = vm.reg(args[1]);
        if radix_arg.is_undefined() {
            10u32
        } else {
            // radix 走 ToIntegerOrInfinity：先经对象 coercion（poisoned valueOf
            // 需传播其异常），NaN/±0 归 0 后越界抛 RangeError。
            let raw = match vm.coerce_number_bounded(radix_arg) {
                Ok(n) => n,
                Err(_) => {
                    // coercion 触发用户 valueOf/toString 抛出的异常经
                    // last_uncaught_value 恢复后原样重新抛出。
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert radix to a number"));
                }
            };
            let r = if raw.is_nan() || raw == 0.0 {
                0.0
            } else if raw.is_infinite() {
                raw
            } else {
                raw.trunc()
            };
            if !(2.0..=36.0).contains(&r) {
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "toString() radix must be between 2 and 36",
                ));
            }
            r as u32
        }
    } else {
        10u32
    };

    if radix == 10 {
        return NativeResult::Ok(vm.new_string_owned(oxide_runtime_api::js_number_to_string(n)));
    }

    if n.is_nan() {
        return NativeResult::Ok(vm.new_string("NaN"));
    }
    if n.is_infinite() {
        return NativeResult::Ok(vm.new_string(if n.is_sign_positive() { "Infinity" } else { "-Infinity" }));
    }
    if n.abs() >= u128::MAX as f64 {
        // 超出 u128 可精确表示的整数范围，退化为十进制近似。
        return NativeResult::Ok(vm.new_string_owned(oxide_runtime_api::js_number_to_string(n)));
    }
    let neg = n.is_sign_negative();
    let mut value = n.abs().trunc() as u128;
    let mut result = String::new();
    if value == 0 {
        result.push('0');
    } else {
        let chars = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut digits = Vec::new();
        while value > 0 {
            digits.push(chars[(value % radix as u128) as usize] as char);
            value /= radix as u128;
        }
        for ch in digits.iter().rev() {
            result.push(*ch);
        }
    }
    if neg {
        result.insert(0, '-');
    }
    NativeResult::Ok(vm.new_string_owned(result))
}

/// `Number.prototype.toFixed(digits)`：固定小数位数（0..100）输出字符串，
/// 超出范围抛 RangeError；NaN/Infinity 输出专名。
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

/// `Number.isInteger`：参数是有限且无小数部分的数值才返回 true。
pub fn number_is_integer<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    if !val.is_int() && !val.is_double() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(val);
    NativeResult::Ok(JsValue::bool(n.trunc() == n && n.is_finite()))
}

/// `Number.isSafeInteger`：参数是安全整数范围（±2^53-1）内的整数才返回 true。
pub fn number_is_safe_integer<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = vm.reg(if args.len() > 1 { args[1] } else { args[0] });
    if !val.is_int() && !val.is_double() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let n = oxide_runtime_api::to_number(val);
    let safe = n.trunc() == n && n.is_finite() && n >= -9007199254740991i64 as f64 && n <= 9007199254740991i64 as f64;
    NativeResult::Ok(JsValue::bool(safe))
}

/// `Number.prototype.toPrecision(precision)`：按有效数字位数（1..100）输出，
/// 科学计数法与定点表示按指数自动切换，超范围抛 RangeError。
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

/// `Number.prototype.toExponential(digits)`：按科学计数法输出；
/// digits 缺省时自动决定小数位数，超范围抛 RangeError。
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
        return NativeResult::Ok(vm.new_string_owned(result));
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

/// `Number.prototype.valueOf`：返回包装对象的原始 number；
/// 原始 number 直接返回，非 Number 对象抛 TypeError。
pub fn number_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if this_val.is_int() || this_val.is_double() {
        return NativeResult::Ok(this_val);
    }
    if this_val.is_object() {
        let ptr = this_val.as_js_object_ptr();
        if !ptr.is_null() {
            // Number.prototype 本身是 Number 对象，其 [[NumberData]] 为 +0。
            let number_proto = vm.session().builtin_world().number_proto.as_ptr() as *mut oxide_types::object::JsObject;
            if ptr == number_proto {
                return NativeResult::Ok(JsValue::int(0));
            }
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
