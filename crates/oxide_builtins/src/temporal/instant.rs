//! Temporal.Instant 与 Temporal.Now：ISO 解析、构造、getter、算术、舍入、差值与格式化。

use chrono::Utc;
use num_traits::ToPrimitive;
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::common::native_try;
use super::{
    balance_instant_difference, canonical_time_zone, civil_from_days, days_from_civil, days_in_month,
    duration_like_values, duration_time_nanoseconds, ensure_instant, format_iso_year, get_instant_epoch_ns,
    initialize_temporal_receiver, is_ctor_call, make_duration, make_instant, make_zoned_date_time, native_engine_error,
    parse_digits, parse_fractional_second_digits, receiver_obj, round_instant_difference, temporal_option_number,
    temporal_option_string, temporal_option_value, FractionalSecondDigitsInput, MAX_INSTANT_NS,
};

pub(crate) fn instant_string_without_annotations(input: &str) -> Option<&str> {
    let Some(first_annotation) = input.find('[') else {
        return Some(input);
    };
    let mut rest = &input[first_annotation..];
    let mut saw_calendar = false;
    let mut saw_critical_calendar = false;
    let mut saw_time_zone = false;
    while !rest.is_empty() {
        let body_start = rest.strip_prefix('[')?;
        let close = body_start.find(']')?;
        let body = &body_start[..close];
        rest = &body_start[close + 1..];
        if body.is_empty() || (!rest.is_empty() && !rest.starts_with('[')) {
            return None;
        }
        let (critical, annotation) = match body.strip_prefix('!') {
            Some(value) => (true, value),
            None => (false, body),
        };
        if let Some((key, _value)) = annotation.split_once('=') {
            if key.is_empty()
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
            {
                return None;
            }
            if key == "u-ca" {
                if saw_calendar && (critical || saw_critical_calendar) {
                    return None;
                }
                saw_calendar = true;
                saw_critical_calendar |= critical;
            } else if critical {
                return None;
            }
        } else {
            if saw_time_zone || annotation.is_empty() {
                return None;
            }
            if matches!(annotation.as_bytes().first(), Some(b'+' | b'-')) {
                let bytes = annotation.as_bytes();
                let hour_only = bytes.len() == 3 && bytes[1..3].iter().all(u8::is_ascii_digit);
                let hour_minute = bytes.len() == 6
                    && bytes[3] == b':'
                    && bytes[1..3].iter().all(u8::is_ascii_digit)
                    && bytes[4..6].iter().all(u8::is_ascii_digit);
                if !hour_only && !hour_minute {
                    return None;
                }
                let hour = (bytes[1] - b'0') * 10 + bytes[2] - b'0';
                let minute = if hour_minute { (bytes[4] - b'0') * 10 + bytes[5] - b'0' } else { 0 };
                if hour > 23 || minute > 59 {
                    return None;
                }
            }
            saw_time_zone = true;
        }
    }
    Some(&input[..first_annotation])
}

/// 解析带 UTC 标识或数值偏移的 Temporal ISO 日期时间到纪元纳秒。
///
/// # 边界与前提
/// - 支持扩展年份、compact 格式、annotation 和亚分钟 offset。
/// - 闰秒按前一秒解释；超出 Instant 范围或语法无效时返回 `None`。
pub(crate) fn parse_instant_string(s: &str) -> Option<i128> {
    let input = s.trim();
    let bytes = instant_string_without_annotations(input)?.as_bytes();

    // 解析公历日期；带符号年份固定为六位，负零扩展年份无效。
    let mut cursor = 0usize;
    let negative_extended_year = bytes.first() == Some(&b'-');
    let year_sign = match bytes.first().copied() {
        Some(b'+') => {
            cursor += 1;
            1_i128
        }
        Some(b'-') => {
            cursor += 1;
            -1_i128
        }
        _ => 1_i128,
    };
    let year_digits = if cursor == 0 { 4 } else { 6 };
    let year_magnitude = parse_digits(bytes, &mut cursor, year_digits)?;
    if negative_extended_year && year_magnitude == 0 {
        return None;
    }
    let year = year_sign * year_magnitude;
    let separated_date = bytes.get(cursor) == Some(&b'-');
    if separated_date {
        cursor += 1;
    }
    let month = parse_digits(bytes, &mut cursor, 2)?;
    if separated_date {
        if bytes.get(cursor) != Some(&b'-') {
            return None;
        }
        cursor += 1;
    }
    let day = parse_digits(bytes, &mut cursor, 2)?;

    // 时间允许小时、分钟或秒精度，日期与时间可用 T、t 或空格分隔。
    if !matches!(bytes.get(cursor), Some(b'T' | b't' | b' ')) {
        return None;
    }
    cursor += 1;
    let hour = parse_digits(bytes, &mut cursor, 2)?;
    let colon_time = bytes.get(cursor) == Some(&b':');
    let minute = if colon_time {
        cursor += 1;
        parse_digits(bytes, &mut cursor, 2)?
    } else if matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
        parse_digits(bytes, &mut cursor, 2)?
    } else {
        0
    };
    let (second, has_second) = if bytes.get(cursor) == Some(&b':') {
        cursor += 1;
        (parse_digits(bytes, &mut cursor, 2)?, true)
    } else if !colon_time && matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
        (parse_digits(bytes, &mut cursor, 2)?, true)
    } else {
        (0, false)
    };

    let mut subsecond_ns = 0_i128;
    if matches!(bytes.get(cursor), Some(b'.' | b',')) {
        if !has_second {
            return None;
        }
        cursor += 1;
        let fraction_start = cursor;
        while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
            cursor += 1;
        }
        let fraction_len = cursor - fraction_start;
        if fraction_len == 0 || fraction_len > 9 {
            return None;
        }
        let mut fraction_cursor = fraction_start;
        subsecond_ns = parse_digits(bytes, &mut fraction_cursor, fraction_len)?;
        subsecond_ns *= 10_i128.pow((9 - fraction_len) as u32);
    }

    // 数值 offset 支持 basic/extended 形式及秒以下精度。
    let offset_ns = match bytes.get(cursor).copied() {
        Some(b'Z' | b'z') => {
            cursor += 1;
            0_i128
        }
        Some(sign @ (b'+' | b'-')) => {
            cursor += 1;
            let offset_hour = parse_digits(bytes, &mut cursor, 2)?;
            let colon_format = bytes.get(cursor) == Some(&b':');
            let offset_minute = if colon_format {
                cursor += 1;
                parse_digits(bytes, &mut cursor, 2)?
            } else if matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
                parse_digits(bytes, &mut cursor, 2)?
            } else {
                0
            };
            let (offset_second, has_offset_second) = if colon_format && bytes.get(cursor) == Some(&b':') {
                cursor += 1;
                (parse_digits(bytes, &mut cursor, 2)?, true)
            } else if !colon_format && matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
                (parse_digits(bytes, &mut cursor, 2)?, true)
            } else {
                (0, false)
            };
            let mut offset_subsecond_ns = 0_i128;
            if matches!(bytes.get(cursor), Some(b'.' | b',')) {
                if !has_offset_second {
                    return None;
                }
                cursor += 1;
                let fraction_start = cursor;
                while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
                    cursor += 1;
                }
                let fraction_len = cursor - fraction_start;
                if fraction_len == 0 || fraction_len > 9 {
                    return None;
                }
                let mut fraction_cursor = fraction_start;
                offset_subsecond_ns = parse_digits(bytes, &mut fraction_cursor, fraction_len)?;
                offset_subsecond_ns *= 10_i128.pow((9 - fraction_len) as u32);
            }
            if offset_hour > 23 || offset_minute > 59 || offset_second > 59 {
                return None;
            }
            let magnitude =
                (offset_hour * 3_600 + offset_minute * 60 + offset_second) * 1_000_000_000 + offset_subsecond_ns;
            if sign == b'-' {
                -magnitude
            } else {
                magnitude
            }
        }
        _ => return None,
    };
    if cursor != bytes.len() || hour > 23 || minute > 59 || second > 60 || day == 0 || day > days_in_month(year, month)?
    {
        return None;
    }

    // 使用 proleptic Gregorian 日数精确换算，并在纳秒层应用 offset。
    let epoch_seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second.min(59))?;
    let epoch_ns = epoch_seconds
        .checked_mul(1_000_000_000)?
        .checked_add(subsecond_ns)?
        .checked_sub(offset_ns)?;
    (epoch_ns.unsigned_abs() <= MAX_INSTANT_NS as u128).then_some(epoch_ns)
}

pub(crate) fn primitive_to_bigint<H: VmHost>(vm: &mut H, raw: JsValue) -> Result<i128, JsValue> {
    let primitive = oxide_runtime_api::to_primitive(raw, oxide_runtime_api::ToPrimitiveHint::Number, vm)
        .map_err(|error| native_engine_error(vm, &error))?;
    if primitive.is_bigint() {
        return Ok(vm.bigint_value(primitive).to_i128().unwrap_or(i128::MAX));
    }
    if primitive.is_bool() {
        return Ok(i128::from(primitive.as_bool()));
    }
    if primitive.is_string() {
        let input = to_string(primitive);
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(0);
        }
        return trimmed.parse::<i128>().map_err(|_| {
            crate::error::create_syntax_error(vm, "Cannot convert string to BigInt: invalid integer literal")
        });
    }
    Err(crate::error::create_type_error(vm, "Cannot convert value to a BigInt"))
}

fn instant_like_epoch_ns<H: VmHost>(vm: &mut H, value: JsValue) -> Result<i128, JsValue> {
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_instant_obj() {
                return get_instant_epoch_ns(obj)
                    .ok_or_else(|| crate::error::create_range_error(vm, "invalid Instant"));
            }
            if obj.is_function() {
                return Err(crate::error::create_range_error(vm, "invalid ISO 8601 string"));
            }
        }
        // 对象按规范走 ToString -> ParseTemporalInstantString（解析失败 RangeError）。
        let input = oxide_runtime_api::to_string_full(value, vm).map_err(|error| native_engine_error(vm, &error))?;
        return parse_instant_string(&input)
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid ISO 8601 string"));
    }
    // 非字符串原始值（undefined/null/boolean/number/bigint/symbol）按规范直接抛 TypeError。
    if !value.is_string() {
        return Err(crate::error::create_type_error(vm, "cannot convert to Instant"));
    }
    let input = to_string(value);
    parse_instant_string(&input).ok_or_else(|| crate::error::create_range_error(vm, "invalid ISO 8601 string"))
}

/// `Temporal.Now.instant()`：返回当前时刻的 Temporal.Instant。
pub fn now_instant<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let now = Utc::now();
    let epoch_ns = now.timestamp() as i128 * 1_000_000_000 + now.timestamp_subsec_nanos() as i128;
    make_instant(vm, epoch_ns)
}

/// `Temporal.Now.timeZoneId()`：引擎固定使用 UTC。
pub fn now_time_zone_id<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Ok(vm.new_string("UTC"))
}

/// `Temporal.Instant` 构造器：把参数按 ToBigInt 转换为纪元纳秒。
pub fn instant_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().instant_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.Instant cannot be invoked without 'new'",
        ));
    }
    let raw = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let epoch_ns = native_try!(primitive_to_bigint(vm, raw));
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    let epoch_value = vm.new_bigint(num_bigint::BigInt::from(epoch_ns));
    initialize_temporal_receiver(vm, args, JsObject::OBJ_TYPE_INSTANT, [epoch_value])
}

/// `Temporal.Instant.from(value)`：接受 Instant、ISO 字符串或可转换为字符串的对象。
pub fn instant_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let epoch_ns = native_try!(instant_like_epoch_ns(vm, val));
    make_instant(vm, epoch_ns)
}

/// `Temporal.Instant.fromEpochMilliseconds(epochMilliseconds)`：从整数毫秒创建 Instant。
pub fn instant_from_epoch_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let raw = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let primitive = match oxide_runtime_api::to_primitive(raw, oxide_runtime_api::ToPrimitiveHint::Number, vm) {
        Ok(value) => value,
        Err(error) => return NativeResult::Err(native_engine_error(vm, &error)),
    };
    if primitive.is_bigint() || primitive.is_symbol() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
    }
    let epoch_ms = to_number(primitive);
    if !epoch_ms.is_finite() || epoch_ms.fract() != 0.0 || epoch_ms.abs() > 8_640_000_000_000_000.0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid epoch milliseconds"));
    }
    make_instant(vm, (epoch_ms as i128) * 1_000_000)
}

/// `Temporal.Instant.fromEpochNanoseconds(epochNanoseconds)`：从 BigInt 纳秒创建 Instant。
pub fn instant_from_epoch_nanoseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let raw = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let epoch_ns = native_try!(primitive_to_bigint(vm, raw));
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    make_instant(vm, epoch_ns)
}

/// `Temporal.Instant.compare(one, two)`：按纪元纳秒返回 -1、0 或 1。
pub fn instant_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let one = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let two = if args.len() < 3 { JsValue::undefined() } else { vm.reg(args[2]) };
    let one_ns = native_try!(instant_like_epoch_ns(vm, one));
    let two_ns = native_try!(instant_like_epoch_ns(vm, two));
    let result = match one_ns.cmp(&two_ns) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    NativeResult::Ok(JsValue::int(result))
}

/// `Temporal.Instant.prototype.epochSeconds` getter。
pub fn instant_epoch_seconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let epoch_ns = match get_instant_epoch_ns(obj) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    };
    NativeResult::Ok(JsValue::float(epoch_ns.div_euclid(1_000_000_000) as f64))
}

/// `Temporal.Instant.prototype.epochMilliseconds` getter。
pub fn instant_epoch_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let epoch_ns = match get_instant_epoch_ns(obj) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    };
    NativeResult::Ok(JsValue::float(epoch_ns.div_euclid(1_000_000) as f64))
}

/// `Temporal.Instant.prototype.epochMicroseconds` getter。
pub fn instant_epoch_microseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let epoch_ns = match get_instant_epoch_ns(obj) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    };
    NativeResult::Ok(JsValue::float(epoch_ns.div_euclid(1_000) as f64))
}

/// `Temporal.Instant.prototype.epochNanoseconds` getter：返回精确 BigInt。
pub fn instant_epoch_nanoseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let value = obj.get_prop_at(0);
    if value.is_bigint() {
        NativeResult::Ok(value)
    } else {
        match get_instant_epoch_ns(obj) {
            Some(epoch_ns) => NativeResult::Ok(vm.new_bigint(num_bigint::BigInt::from(epoch_ns))),
            None => NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
        }
    }
}

/// `Temporal.Instant.prototype.equals(other)`：比较两个 Instant 的纪元纳秒。
pub fn instant_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };

    let other = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other_ns = native_try!(instant_like_epoch_ns(vm, other));
    NativeResult::Ok(JsValue::bool(epoch_ns == other_ns))
}

fn instant_add_duration<H: VmHost>(vm: &mut H, args: &[u8], direction: i128) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };

    let duration_like = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let values = native_try!(duration_like_values(vm, duration_like));
    if values[..4].iter().any(|value| *value != 0.0) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "Instant arithmetic does not support date units",
        ));
    }
    let Some(delta_ns) = duration_time_nanoseconds(&values) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid duration"));
    };
    let Some(result_ns) = delta_ns.checked_mul(direction).and_then(|delta| epoch_ns.checked_add(delta)) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    };
    if result_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    make_instant(vm, result_ns)
}

/// `Temporal.Instant.prototype.add(durationLike)`：精确增加仅含时间单位的时长。
pub fn instant_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_add_duration(vm, args, 1)
}

/// `Temporal.Instant.prototype.subtract(durationLike)`：精确减去仅含时间单位的时长。
pub fn instant_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_add_duration(vm, args, -1)
}

#[derive(Clone, Copy)]
pub(crate) enum InstantRoundingMode {
    Ceil,
    Expand,
    Floor,
    HalfCeil,
    HalfEven,
    HalfExpand,
    HalfFloor,
    HalfTrunc,
    Trunc,
}

pub(crate) fn instant_rounding_mode(value: &str) -> Option<InstantRoundingMode> {
    match value {
        "ceil" => Some(InstantRoundingMode::Ceil),
        "expand" => Some(InstantRoundingMode::Expand),
        "floor" => Some(InstantRoundingMode::Floor),
        "halfCeil" => Some(InstantRoundingMode::HalfCeil),
        "halfEven" => Some(InstantRoundingMode::HalfEven),
        "halfExpand" => Some(InstantRoundingMode::HalfExpand),
        "halfFloor" => Some(InstantRoundingMode::HalfFloor),
        "halfTrunc" => Some(InstantRoundingMode::HalfTrunc),
        "trunc" => Some(InstantRoundingMode::Trunc),
        _ => None,
    }
}

fn instant_round_unit(value: &str) -> Option<(i128, i128)> {
    match value {
        "hour" | "hours" => Some((3_600_000_000_000, 24)),
        "minute" | "minutes" => Some((60_000_000_000, 1_440)),
        "second" | "seconds" => Some((1_000_000_000, 86_400)),
        "millisecond" | "milliseconds" => Some((1_000_000, 86_400_000)),
        "microsecond" | "microseconds" => Some((1_000, 86_400_000_000)),
        "nanosecond" | "nanoseconds" => Some((1, 86_400_000_000_000)),
        _ => None,
    }
}

/// 按 Temporal 的 `RoundNumberToIncrementAsIfPositive` 语义舍入纳秒。
pub(crate) fn round_instant_ns(value: i128, increment: i128, mode: InstantRoundingMode) -> Option<i128> {
    let quotient = value.div_euclid(increment);
    let remainder = value.rem_euclid(increment);
    if remainder == 0 {
        return Some(value);
    }
    let use_upper = match mode {
        InstantRoundingMode::Ceil | InstantRoundingMode::Expand => true,
        InstantRoundingMode::Floor | InstantRoundingMode::Trunc => false,
        _ => match (remainder * 2).cmp(&increment) {
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => match mode {
                InstantRoundingMode::HalfCeil | InstantRoundingMode::HalfExpand => true,
                InstantRoundingMode::HalfFloor | InstantRoundingMode::HalfTrunc => false,
                InstantRoundingMode::HalfEven => quotient.rem_euclid(2) != 0,
                _ => unreachable!(),
            },
        },
    };
    quotient
        .checked_add(i128::from(use_upper))
        .and_then(|rounded| rounded.checked_mul(increment))
}

/// `Temporal.Instant.prototype.round(roundTo)`：按给定单位、增量和模式精确舍入。
pub fn instant_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };

    let round_to = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (increment_value, mode_value, unit_value) = if round_to.is_string() {
        (1.0, "halfExpand".to_string(), Some(to_string(round_to)))
    } else {
        if !round_to.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "roundTo must be a string or object"));
        }
        let options_ptr = round_to.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "roundTo must be a string or object"));
        }
        let options = unsafe { &*options_ptr };
        let increment_raw = native_try!(temporal_option_value(vm, options, round_to, "roundingIncrement"));
        let increment = if increment_raw.is_undefined() {
            1.0
        } else {
            native_try!(temporal_option_number(vm, increment_raw))
        };
        let mode_raw = native_try!(temporal_option_value(vm, options, round_to, "roundingMode"));
        let mode = if mode_raw.is_undefined() {
            "halfExpand".to_string()
        } else {
            native_try!(temporal_option_string(vm, mode_raw))
        };
        let unit_raw = native_try!(temporal_option_value(vm, options, round_to, "smallestUnit"));
        let unit = if unit_raw.is_undefined() {
            None
        } else {
            Some(native_try!(temporal_option_string(vm, unit_raw)))
        };
        (increment, mode, unit)
    };

    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
    };
    let Some(unit_value) = unit_value else {
        return NativeResult::Err(crate::error::create_range_error(vm, "smallestUnit is required"));
    };
    let Some((unit_ns, units_per_day)) = instant_round_unit(&unit_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid smallest unit"));
    };
    if !increment_value.is_finite() {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment_value.trunc();
    if !(1.0..=1_000_000_000.0).contains(&increment) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment as i128;
    if units_per_day % increment != 0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "rounding increment must divide a day"));
    }
    let Some(quantum_ns) = unit_ns.checked_mul(increment) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    };
    let Some(rounded_ns) = round_instant_ns(epoch_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    };
    if rounded_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    make_instant(vm, rounded_ns)
}

fn instant_difference_unit(value: &str) -> Option<(usize, i128, i128)> {
    match value {
        "hour" | "hours" => Some((0, 3_600_000_000_000, 24)),
        "minute" | "minutes" => Some((1, 60_000_000_000, 60)),
        "second" | "seconds" => Some((2, 1_000_000_000, 60)),
        "millisecond" | "milliseconds" => Some((3, 1_000_000, 1_000)),
        "microsecond" | "microseconds" => Some((4, 1_000, 1_000)),
        "nanosecond" | "nanoseconds" => Some((5, 1, 1_000)),
        _ => None,
    }
}

fn instant_difference<H: VmHost>(vm: &mut H, args: &[u8], since: bool) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };

    // other 必须先完成 Instant 转换，之后才读取 options。
    let other = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other_ns = native_try!(instant_like_epoch_ns(vm, other));
    let options_value = if args.len() < 3 { JsValue::undefined() } else { vm.reg(args[2]) };

    let (largest_value, increment_value, mode_value, smallest_value) = if options_value.is_undefined() {
        (None, 1.0, "trunc".to_string(), "nanosecond".to_string())
    } else {
        if !options_value.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let largest_raw = native_try!(temporal_option_value(vm, options, options_value, "largestUnit"));
        let largest = if largest_raw.is_undefined() {
            None
        } else {
            Some(native_try!(temporal_option_string(vm, largest_raw)))
        };
        let increment_raw = native_try!(temporal_option_value(vm, options, options_value, "roundingIncrement"));
        let increment = if increment_raw.is_undefined() {
            1.0
        } else {
            native_try!(temporal_option_number(vm, increment_raw))
        };
        let mode_raw = native_try!(temporal_option_value(vm, options, options_value, "roundingMode"));
        let mode = if mode_raw.is_undefined() {
            "trunc".to_string()
        } else {
            native_try!(temporal_option_string(vm, mode_raw))
        };
        let smallest_raw = native_try!(temporal_option_value(vm, options, options_value, "smallestUnit"));
        let smallest = if smallest_raw.is_undefined() {
            "nanosecond".to_string()
        } else {
            native_try!(temporal_option_string(vm, smallest_raw))
        };
        (largest, increment, mode, smallest)
    };

    let Some((smallest_index, smallest_ns, increment_limit)) = instant_difference_unit(&smallest_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid smallest unit"));
    };
    let largest_index = match largest_value {
        Some(value) => match instant_difference_unit(&value) {
            Some((index, _, _)) => index,
            None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid largest unit")),
        },
        None => smallest_index.min(2),
    };
    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
    };
    if !increment_value.is_finite() {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment_value.trunc();
    if !(1.0..=1_000_000_000.0).contains(&increment) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment as i128;
    if increment >= increment_limit || increment_limit % increment != 0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    if largest_index > smallest_index {
        return NativeResult::Err(crate::error::create_range_error(vm, "smallest unit exceeds largest unit"));
    }

    let delta = if since { epoch_ns - other_ns } else { other_ns - epoch_ns };
    let Some(quantum_ns) = smallest_ns.checked_mul(increment) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    };
    let Some(rounded_ns) = round_instant_difference(delta, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant difference is out of range"));
    };
    let Some(values) = balance_instant_difference(rounded_ns, largest_index) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant difference is out of range"));
    };
    make_duration(vm, values)
}

/// `Temporal.Instant.prototype.until(other, options)`：返回从 receiver 到 other 的精确时长。
pub fn instant_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_difference(vm, args, false)
}

/// `Temporal.Instant.prototype.since(other, options)`：返回从 other 到 receiver 的精确时长。
pub fn instant_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_difference(vm, args, true)
}

pub(crate) fn instant_time_zone_offset(value: &str) -> Option<i32> {
    canonical_time_zone(value).map(|(_, offset)| offset)
}

fn format_instant_iso(
    epoch_ns: i128, offset_minutes: Option<i32>, include_seconds: bool, fractional_digits: Option<usize>,
) -> Option<String> {
    const DAY_NS: i128 = 86_400_000_000_000;
    let offset_ns = i128::from(offset_minutes.unwrap_or(0)).checked_mul(60_000_000_000)?;
    let local_ns = epoch_ns.checked_add(offset_ns)?;
    let days = local_ns.div_euclid(DAY_NS);
    let mut time_ns = local_ns.rem_euclid(DAY_NS);
    let hour = time_ns / 3_600_000_000_000;
    time_ns %= 3_600_000_000_000;
    let minute = time_ns / 60_000_000_000;
    time_ns %= 60_000_000_000;
    let second = time_ns / 1_000_000_000;
    let subsecond = time_ns % 1_000_000_000;
    let (year, month, day) = civil_from_days(days);

    let mut output = format!("{}-{month:02}-{day:02}T{hour:02}:{minute:02}", format_iso_year(year));
    if include_seconds {
        output.push_str(&format!(":{second:02}"));
        match fractional_digits {
            Some(0) => {}
            Some(digits) => {
                let fraction = format!("{subsecond:09}");
                output.push('.');
                output.push_str(&fraction[..digits]);
            }
            None if subsecond != 0 => {
                let fraction = format!("{subsecond:09}").trim_end_matches('0').to_string();
                output.push('.');
                output.push_str(&fraction);
            }
            None => {}
        }
    }

    if let Some(offset) = offset_minutes {
        let sign = if offset < 0 { '-' } else { '+' };
        let magnitude = offset.abs();
        output.push_str(&format!("{sign}{:02}:{:02}", magnitude / 60, magnitude % 60));
    } else {
        output.push('Z');
    }
    Some(output)
}

fn instant_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };
    match format_instant_iso(epoch_ns, None, true, None) {
        Some(output) => NativeResult::Ok(vm.new_string_owned(output)),
        None => NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    }
}

/// `Temporal.Instant.prototype.toString(options)`：按精度、舍入模式和时区输出 ISO 8601。
pub fn instant_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let epoch_ns = match get_instant_epoch_ns(obj) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    };
    let options_value = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (fractional_input, mode_value, smallest_value, time_zone_raw) = if options_value.is_undefined() {
        (FractionalSecondDigitsInput::Auto, "trunc".to_string(), None, JsValue::undefined())
    } else {
        if !options_value.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let fractional_raw = native_try!(temporal_option_value(vm, options, options_value, "fractionalSecondDigits"));
        let fractional = if fractional_raw.is_undefined() {
            FractionalSecondDigitsInput::Auto
        } else if fractional_raw.is_int() || fractional_raw.is_double() {
            FractionalSecondDigitsInput::Number(to_number(fractional_raw))
        } else {
            FractionalSecondDigitsInput::String(native_try!(temporal_option_string(vm, fractional_raw)))
        };
        let mode_raw = native_try!(temporal_option_value(vm, options, options_value, "roundingMode"));
        let mode = if mode_raw.is_undefined() {
            "trunc".to_string()
        } else {
            native_try!(temporal_option_string(vm, mode_raw))
        };
        let smallest_raw = native_try!(temporal_option_value(vm, options, options_value, "smallestUnit"));
        let smallest = if smallest_raw.is_undefined() {
            None
        } else {
            Some(native_try!(temporal_option_string(vm, smallest_raw)))
        };
        let time_zone = native_try!(temporal_option_value(vm, options, options_value, "timeZone"));
        (fractional, mode, smallest, time_zone)
    };

    let fractional_digits = native_try!(parse_fractional_second_digits(vm, fractional_input));
    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
    };
    let (quantum_ns, include_seconds, output_digits) = match smallest_value.as_deref() {
        Some("minute" | "minutes") => (60_000_000_000, false, Some(0)),
        Some("second" | "seconds") => (1_000_000_000, true, Some(0)),
        Some("millisecond" | "milliseconds") => (1_000_000, true, Some(3)),
        Some("microsecond" | "microseconds") => (1_000, true, Some(6)),
        Some("nanosecond" | "nanoseconds") => (1, true, Some(9)),
        Some(_) => return NativeResult::Err(crate::error::create_range_error(vm, "invalid smallest unit")),
        None => match fractional_digits {
            Some(digits) => (10_i128.pow((9 - digits) as u32), true, Some(digits)),
            None => (1, true, None),
        },
    };
    let offset_minutes = if time_zone_raw.is_undefined() {
        None
    } else {
        if !time_zone_raw.is_string() {
            return NativeResult::Err(crate::error::create_type_error(vm, "invalid time zone"));
        }
        let time_zone = to_string(time_zone_raw);
        match instant_time_zone_offset(&time_zone) {
            Some(offset) => Some(offset),
            None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid time zone")),
        }
    };
    let Some(rounded_ns) = round_instant_ns(epoch_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    };
    if rounded_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    match format_instant_iso(rounded_ns, offset_minutes, include_seconds, output_digits) {
        Some(output) => NativeResult::Ok(vm.new_string_owned(output)),
        None => NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    }
}

/// `Temporal.Instant.prototype.toJSON()`：输出默认 UTC ISO 字符串，忽略参数。
pub fn instant_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_default_string(vm, args)
}

/// `Temporal.Instant.prototype.toLocaleString()`：当前使用稳定的 UTC ISO 表示。
pub fn instant_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_default_string(vm, args)
}

/// `Temporal.Instant.prototype.toZonedDateTimeISO(timeZone)`：以同一纪元纳秒创建 ISO 日历 ZonedDateTime。
pub fn instant_to_zoned_date_time_iso<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "timeZone is required"));
    }
    let raw = vm.reg(args[1]);
    if !raw.is_string() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid time zone"));
    }
    let input = to_string(raw);
    let Some((time_zone_id, _)) = canonical_time_zone(&input) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time zone"));
    };
    make_zoned_date_time(vm, epoch_ns, &time_zone_id, "iso8601")
}

/// `Temporal.Instant.prototype.valueOf()`：Temporal 对象禁止转原始值。
pub fn instant_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.Instant has no valueOf"))
}
