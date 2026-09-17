//! Temporal.Duration：ISO 字符串与 property bag 解析、构造、getter、total/round、
//! 加减比较与格式化。

use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::common::native_try;
use super::common::{
    canonical_time_zone, civil_from_days, days_from_civil, ensure_duration, get_double_prop, get_instant_epoch_ns,
    initialize_temporal_receiver, is_ctor_call, native_engine_error, plain_date_time_object_parts, receiver_obj,
    temporal_option_number, temporal_option_string, temporal_option_value, zoned_date_time_plain_parts,
};
use super::difference::{
    add_date_duration, add_days_iso, compare_iso_date, date_duration_sign, date_until_iso, nudge_iso_difference,
    nudge_window, plain_date_time_unit_index, round_instant_difference, DifferenceSettings, DAY_NS, MAX_ISO_DAY,
};
use super::instant::{instant_rounding_mode, instant_time_zone_offset, InstantRoundingMode};
use super::zoned_date_time::{parse_any_offset_minutes, valid_offset_fraction, zoned_date_time_string_parts};
use super::{make_duration, parse_plain_date_time_string};

fn parse_duration_string(input: &str) -> Option<[f64; 10]> {
    let bytes = input.as_bytes();
    let mut cursor = 0usize;
    let negative = match bytes.first().copied() {
        Some(b'-') => {
            cursor += 1;
            true
        }
        Some(b'+') => {
            cursor += 1;
            false
        }
        _ => false,
    };
    if bytes.get(cursor).copied().map(|byte| byte.to_ascii_uppercase()) != Some(b'P') {
        return None;
    }
    cursor += 1;

    let mut values = [0.0; 10];
    let mut in_time = false;
    let mut saw_unit = false;
    let mut saw_time_unit = false;
    let mut last_order = 0usize;
    let mut fraction_seen = false;
    while cursor < bytes.len() {
        if bytes[cursor].eq_ignore_ascii_case(&b'T') {
            if in_time {
                return None;
            }
            in_time = true;
            cursor += 1;
            continue;
        }

        let number_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == number_start {
            return None;
        }
        let whole = input[number_start..cursor].parse::<f64>().ok()?;
        if !whole.is_finite() {
            return None;
        }

        let mut fraction = None;
        if matches!(bytes.get(cursor), Some(b'.' | b',')) {
            cursor += 1;
            let fraction_start = cursor;
            while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                cursor += 1;
            }
            let digits = cursor - fraction_start;
            if digits == 0 || digits > 9 {
                return None;
            }
            let numerator = input[fraction_start..cursor].parse::<u64>().ok()?;
            fraction = Some((numerator, 10_u64.pow(digits as u32)));
        }

        let unit = bytes.get(cursor)?.to_ascii_uppercase();
        cursor += 1;
        let (index, order, unit_nanos) = match (in_time, unit) {
            (false, b'Y') => (0, 1, None),
            (false, b'M') => (1, 2, None),
            (false, b'W') => (2, 3, None),
            (false, b'D') => (3, 4, None),
            (true, b'H') => (4, 5, Some(3_600_000_000_000_u64)),
            (true, b'M') => (5, 6, Some(60_000_000_000_u64)),
            (true, b'S') => (6, 7, Some(1_000_000_000_u64)),
            _ => return None,
        };
        if order <= last_order || fraction_seen {
            return None;
        }
        last_order = order;
        values[index] = whole;
        saw_unit = true;
        saw_time_unit |= in_time;

        if let Some((numerator, scale)) = fraction {
            let unit_nanos = unit_nanos?;
            let mut nanos = (numerator as u128 * unit_nanos as u128 / scale as u128) as u64;
            if index <= 4 {
                values[5] += (nanos / 60_000_000_000) as f64;
                nanos %= 60_000_000_000;
            }
            if index <= 5 {
                values[6] += (nanos / 1_000_000_000) as f64;
                nanos %= 1_000_000_000;
            }
            values[7] += (nanos / 1_000_000) as f64;
            nanos %= 1_000_000;
            values[8] += (nanos / 1_000) as f64;
            values[9] += (nanos % 1_000) as f64;
            fraction_seen = true;
        }
    }
    if !saw_unit || (in_time && !saw_time_unit) {
        return None;
    }
    if negative {
        for value in &mut values {
            if *value != 0.0 {
                *value = -*value;
            }
        }
    }
    Some(values)
}

const DURATION_FIELD_ORDER: [(&str, usize); 10] = [
    ("days", 3),
    ("hours", 4),
    ("microseconds", 8),
    ("milliseconds", 7),
    ("minutes", 5),
    ("months", 1),
    ("nanoseconds", 9),
    ("seconds", 6),
    ("weeks", 2),
    ("years", 0),
];

fn duration_field_number<H: VmHost>(
    vm: &mut H, obj: &JsObject, receiver: JsValue, name: &str,
) -> Result<Option<f64>, JsValue> {
    let key_val = vm.new_string(name);
    let si = vm.property_key_si(key_val);
    let raw = match vm.ordinary_get(obj, si, receiver) {
        Ok(value) => value,
        Err(error) => return Err(native_engine_error(vm, &error)),
    };
    if raw.is_undefined() {
        return Ok(None);
    }
    let primitive = match oxide_runtime_api::to_primitive(raw, oxide_runtime_api::ToPrimitiveHint::Number, vm) {
        Ok(value) => value,
        Err(error) => return Err(native_engine_error(vm, &error)),
    };
    if primitive.is_symbol() || primitive.is_bigint() {
        return Err(crate::error::create_type_error(vm, "cannot convert duration field to number"));
    }
    Ok(Some(to_number(primitive)))
}

fn duration_partial_values<H: VmHost>(vm: &mut H, val: JsValue) -> Result<[Option<f64>; 10], JsValue> {
    if !val.is_object() {
        return Err(crate::error::create_type_error(vm, "duration-like value must be an object"));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "duration-like value must be an object"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_duration_obj() {
        return Ok(std::array::from_fn(|index| Some(get_double_prop(obj, index))));
    }

    let mut values = [None; 10];
    for (name, index) in DURATION_FIELD_ORDER {
        values[index] = duration_field_number(vm, obj, val, name)?;
    }
    if values.iter().all(Option::is_none) {
        return Err(crate::error::create_type_error(vm, "duration-like object has no duration fields"));
    }
    Ok(values)
}

pub(crate) fn duration_component_integer(value: f64) -> Option<i128> {
    if !value.is_finite() || value.fract() != 0.0 || value.abs() >= i128::MAX as f64 {
        return None;
    }
    Some(value as i128)
}

pub(crate) fn duration_time_nanoseconds(values: &[f64; 10]) -> Option<i128> {
    const SCALES: [i128; 7] =
        [86_400_000_000_000, 3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    let mut total = 0_i128;
    for (value, scale) in values[3..].iter().zip(SCALES) {
        total = total.checked_add(duration_component_integer(*value)?.checked_mul(scale)?)?;
    }
    Some(total)
}

fn validate_duration_values<H: VmHost>(vm: &mut H, values: &[f64; 10]) -> Result<(), JsValue> {
    let mut sign = 0_i8;
    for value in values {
        if duration_component_integer(*value).is_none() {
            return Err(crate::error::create_range_error(vm, "invalid duration"));
        }
        if *value != 0.0 {
            let current = if value.is_sign_negative() { -1 } else { 1 };
            if sign != 0 && sign != current {
                return Err(crate::error::create_range_error(vm, "duration fields must have the same sign"));
            }
            sign = current;
        }
    }
    if values[..3].iter().any(|value| value.abs() > u32::MAX as f64) {
        return Err(crate::error::create_range_error(vm, "duration date field is out of range"));
    }
    const MAX_TIME_NANOSECONDS: i128 = (1_i128 << 53) * 1_000_000_000;
    let total = duration_time_nanoseconds(values)
        .ok_or_else(|| crate::error::create_range_error(vm, "duration time fields are out of range"))?;
    if total.abs() >= MAX_TIME_NANOSECONDS {
        return Err(crate::error::create_range_error(vm, "duration time fields are out of range"));
    }
    Ok(())
}

pub(crate) fn duration_like_values<H: VmHost>(vm: &mut H, val: JsValue) -> Result<[f64; 10], JsValue> {
    if val.is_string() {
        let values = parse_duration_string(&to_string(val))
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid duration string"))?;
        validate_duration_values(vm, &values)?;
        return Ok(values);
    }
    let partial = duration_partial_values(vm, val)?;
    let values = std::array::from_fn(|index| partial[index].unwrap_or(0.0));
    validate_duration_values(vm, &values)?;
    Ok(values)
}

/// `Temporal.Duration` 构造器，保存十个整数时长分量。
pub fn duration_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().duration_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.Duration cannot be invoked without 'new'",
        ));
    }
    let mut values = [0.0; 10];
    for (index, value) in values.iter_mut().enumerate() {
        if args.len() <= index + 1 {
            continue;
        }
        let number = to_number(vm.reg(args[index + 1]));
        if !number.is_finite() || number.fract() != 0.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid duration"));
        }
        *value = number;
    }
    native_try!(validate_duration_values(vm, &values));
    initialize_temporal_receiver(vm, args, JsObject::OBJ_TYPE_DURATION, values.map(JsValue::float))
}

/// `Temporal.Duration.from(value)`，复制 Duration 或读取同名字段。
pub fn duration_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = native_try!(duration_like_values(vm, val));
    make_duration(vm, values)
}

fn duration_values(obj: &JsObject) -> [f64; 10] {
    std::array::from_fn(|index| get_double_prop(obj, index))
}

/// 归一 options.relativeTo 为相对日期 `(y, m, d)`。
///
/// # 步骤
/// 1. string → parse_plain_date_time_string，失败回退 parse_plain_date_string（PDT 优先、PD 回退）；失败 RangeError。
/// 2. PlainDateTime / PlainDate 对象 → 直读 prop 0/1/2。
/// 3. ZonedDateTime → zoned_date_time_plain_parts 取本地 (y,m,d)。
/// 4. Instant → epoch 纳秒按 UTC 分解（div_euclid(DAY_NS) → civil_from_days）。
/// 5. 其他对象按 property bag 解析（ToTemporalDateTime，constrain）。
/// 6. undefined → None；其余原始值 → TypeError。
///
/// # 边界与前提
/// - 调用方须已把 raw 从 options 取出（读序由调用方保证）。
/// - 本函数只取日期分量；时间分量由调用方的 duration 分量另行加。
fn duration_relative_to_date<H: VmHost>(
    vm: &mut H, relative_raw: JsValue,
) -> Result<Option<(i128, i128, i128)>, JsValue> {
    if relative_raw.is_undefined() {
        return Ok(None);
    }
    if relative_raw.is_string() {
        let text = to_string(relative_raw);
        // 先按 PlainDateTime/PlainDate 解析（纯日期、日期时间、带 offset/注解的 plain 串）。
        if let Ok((year, month, day, _time_ns)) = parse_plain_date_time_string(&text) {
            return Ok(Some((i128::from(year), i128::from(month), i128::from(day))));
        }
        // ZonedDateTime-like（含 Z 或时区注解）：按 instant 解析 + 注解时区反推墙钟日期。
        let (epoch_ns, time_zone_id, _calendar) = zoned_date_time_string_parts(vm, &text, "reject")?;
        let offset_minutes = instant_time_zone_offset(&time_zone_id).unwrap_or(0);
        let wall_ns = epoch_ns + i128::from(offset_minutes) * 60_000_000_000;
        let (year, month, day) = civil_from_days(wall_ns.div_euclid(DAY_NS));
        return Ok(Some((year, month, day)));
    }
    if !relative_raw.is_object() {
        return Err(crate::error::create_type_error(vm, "invalid relativeTo"));
    }
    let ptr = relative_raw.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "invalid relativeTo"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_plain_date_time_obj() || obj.is_plain_date_obj() {
        return Ok(Some((
            i128::from(get_double_prop(obj, 0) as i32),
            i128::from(get_double_prop(obj, 1) as u32),
            i128::from(get_double_prop(obj, 2) as u32),
        )));
    }
    if obj.is_zoned_date_time_obj() {
        let (year, month, day, _time_ns) = zoned_date_time_plain_parts(vm, obj)?;
        return Ok(Some((i128::from(year), i128::from(month), i128::from(day))));
    }
    if obj.is_instant_obj() {
        let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
            return Err(crate::error::create_range_error(vm, "invalid Instant"));
        };
        let (year, month, day) = civil_from_days(epoch_ns.div_euclid(DAY_NS));
        return Ok(Some((year, month, day)));
    }
    // property bag：按 ToRelativeTemporalObject 依次校验 calendar/timeZone/offset 再读字段。
    let calendar_raw = temporal_option_value(vm, obj, relative_raw, "calendar")?;
    if !calendar_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "invalid relativeTo"));
    }
    let time_zone_raw = temporal_option_value(vm, obj, relative_raw, "timeZone")?;
    if !time_zone_raw.is_undefined() {
        // 非字符串（对象/符号/null/数字）→ TypeError；字符串非合法时区 → RangeError。
        if !time_zone_raw.is_string() {
            return Err(crate::error::create_type_error(vm, "invalid time zone"));
        }
        if canonical_time_zone(&to_string(time_zone_raw)).is_none() {
            return Err(crate::error::create_range_error(vm, "invalid time zone"));
        }
    }
    let offset_raw = temporal_option_value(vm, obj, relative_raw, "offset")?;
    if !offset_raw.is_undefined() {
        // 非字符串 → TypeError；字符串格式非法（含亚秒偏移）→ RangeError。
        if !offset_raw.is_string() {
            return Err(crate::error::create_type_error(vm, "invalid offset"));
        }
        let offset_input = to_string(offset_raw);
        if parse_any_offset_minutes(&offset_input).is_none() || !valid_offset_fraction(&offset_input) {
            return Err(crate::error::create_range_error(vm, "invalid offset"));
        }
    }
    let (year, month, day, _time_ns, _calendar) = plain_date_time_object_parts(vm, relative_raw, obj, true, false)?;
    Ok(Some((i128::from(year), i128::from(month), i128::from(day))))
}

fn format_duration_number(value: f64) -> String {
    format!("{}", value as i64)
}

fn format_duration_seconds(seconds: f64, milliseconds: f64, microseconds: f64, nanoseconds: f64) -> Option<String> {
    let total = duration_component_integer(seconds)?
        .checked_mul(1_000_000_000)?
        .checked_add(duration_component_integer(milliseconds)?.checked_mul(1_000_000)?)?
        .checked_add(duration_component_integer(microseconds)?.checked_mul(1_000)?)?
        .checked_add(duration_component_integer(nanoseconds)?)?;
    if total == 0 {
        return None;
    }
    let negative = total < 0;
    let magnitude = total.abs();
    let whole = magnitude / 1_000_000_000;
    let fraction = magnitude % 1_000_000_000;
    let mut result = format!("{}", whole);
    if fraction != 0 {
        let mut digits = format!("{fraction:09}");
        while digits.ends_with('0') {
            digits.pop();
        }
        result.push('.');
        result.push_str(&digits);
    }
    if negative {
        result.insert(0, '-');
    }
    Some(result)
}

/// `Temporal.Duration.prototype.with(partial)`，以给定字段替换当前时长分量。
pub fn duration_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let partial_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let partial = native_try!(duration_partial_values(vm, partial_value));
    let mut values = duration_values(obj);
    for (index, value) in partial.into_iter().enumerate() {
        if let Some(value) = value {
            values[index] = value;
        }
    }
    native_try!(validate_duration_values(vm, &values));
    make_duration(vm, values)
}

/// `Temporal.Duration.prototype.total(totalOf)`：按单位汇总时长。
/// 日历单位（year/month/week）需要 relativeTo（PlainDateTime/PlainDate）。
pub fn duration_total<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let total_of = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if total_of.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "options argument is required"));
    }
    let (unit_raw, relative_raw) = if total_of.is_string() {
        (to_string(total_of), JsValue::undefined())
    } else {
        if !total_of.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = total_of.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let relative_raw = match temporal_option_value(vm, options, total_of, "relativeTo") {
            Ok(raw) => raw,
            Err(error) => return NativeResult::Err(error),
        };
        let unit_raw = match temporal_option_value(vm, options, total_of, "unit") {
            Ok(raw) => match temporal_option_string(vm, raw) {
                Ok(value) => value,
                Err(error) => return NativeResult::Err(error),
            },
            Err(error) => return NativeResult::Err(error),
        };
        (unit_raw, relative_raw)
    };
    let unit_index = match plain_date_time_unit_index(&unit_raw) {
        Some(index) => index,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid unit")),
    };
    let values = duration_values(obj);
    // 含日历单位（year/month/week）时，days 及以上的 total 必须走 relativeTo 日历路径。
    let has_calendar_units = values[0] != 0.0 || values[1] != 0.0 || values[2] != 0.0;

    // relativeTo：统一经 duration_relative_to_date 归一（支持 string/PD/PDT/ZDT/Instant/bag）。
    let relative_date = match duration_relative_to_date(vm, relative_raw) {
        Ok(relative) => relative,
        Err(error) => return NativeResult::Err(error),
    };

    const UNIT_NS: [i128; 6] = [3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    if unit_index <= 2 || (unit_index == 3 && has_calendar_units) {
        // 日历单位：需要 relativeTo，用纪元纳秒窗口计算分数总量。
        let Some(rel_date) = relative_date else {
            return NativeResult::Err(crate::error::create_range_error(
                vm,
                &format!("a starting point is required for {} total", unit_raw),
            ));
        };
        let Some(time_ns_base) = duration_time_nanoseconds(&values) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        // days 并入时间（24 小时/天），整日回填到日期分量。
        // duration_time_nanoseconds 已包含 days 分量（24 小时/天），直接取整日。
        let time_ns_total = time_ns_base;
        let delta_days = time_ns_total / DAY_NS;
        let target_time = time_ns_total % DAY_NS;
        let mut date_parts = [0.0; 10];
        date_parts[0] = values[0];
        date_parts[1] = values[1];
        date_parts[2] = values[2];
        date_parts[3] = delta_days as f64;
        let target_date = add_date_duration(rel_date, &date_parts);

        // DifferenceISODateTime：date1=relativeTo（午夜）、date2=target。
        let date1 = rel_date;
        let mut date2 = target_date;
        let mut time_ns = target_time;
        let time_sign = if time_ns > 0 {
            1_i128
        } else if time_ns < 0 {
            -1_i128
        } else {
            0
        };
        let date_sign = compare_iso_date(date1, date2);
        if date_sign != 0 && date_sign == time_sign {
            date2 = add_days_iso(date2, time_sign);
            time_ns -= time_sign * DAY_NS;
        }
        let date_values = date_until_iso(date1, date2, unit_index);
        let sign = {
            let ds = date_duration_sign(&date_values);
            if ds != 0 {
                ds
            } else if time_ns > 0 {
                1
            } else if time_ns < 0 {
                -1
            } else {
                1
            }
        };
        let origin_epoch = days_from_civil(date1.0, date1.1, date1.2) * DAY_NS;
        let dest_epoch = days_from_civil(target_date.0, target_date.1, target_date.2) * DAY_NS + target_time;
        let (r1, _, start_dur, end_dur) = nudge_window(sign, &date_values, date1, 1, unit_index, false);
        let epoch_of = |dur: &[f64; 10]| -> Option<i128> {
            if date_duration_sign(dur) == 0 {
                return Some(origin_epoch);
            }
            let date = add_date_duration(date1, dur);
            let days = days_from_civil(date.0, date.1, date.2);
            if days.abs() > MAX_ISO_DAY {
                return None;
            }
            Some(days * DAY_NS)
        };
        let Some(start_epoch) = epoch_of(&start_dur) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        let Some(end_epoch) = epoch_of(&end_dur) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        let numerator = dest_epoch - start_epoch;
        let denominator = end_epoch - start_epoch;
        if denominator == 0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        }
        let total = (denominator as f64 * r1 as f64 + numerator as f64 * sign as f64) / denominator as f64;
        return NativeResult::Ok(JsValue::float(total));
    }

    // 均匀长度单位（day..nanosecond）：直接按纳秒汇总。
    let Some(time_ns_base) = duration_time_nanoseconds(&values) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
    };
    // duration_time_nanoseconds 已包含 days 分量。
    let total_ns = time_ns_base;
    let scale = if unit_index == 3 { DAY_NS } else { UNIT_NS[unit_index - 4] };
    NativeResult::Ok(JsValue::float(total_ns as f64 / scale as f64))
}

/// `Temporal.Duration.prototype.toString()` 的 ISO 8601 表示。
pub fn duration_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let mut values = duration_values(obj);
    let negative = values.iter().any(|value| *value < 0.0);
    for value in &mut values {
        *value = value.abs();
    }
    let mut output = String::from(if negative { "-P" } else { "P" });
    let date_units = [(0, "Y"), (1, "M"), (2, "W"), (3, "D")];
    let mut has_date = false;
    for (index, suffix) in date_units {
        if values[index] != 0.0 {
            has_date = true;
            output.push_str(&format_duration_number(values[index]));
            output.push_str(suffix);
        }
    }
    let time_units = [(4, "H"), (5, "M")];
    let has_time = values[4..].iter().any(|value| *value != 0.0);
    if has_time || !has_date {
        output.push('T');
        for (index, suffix) in time_units {
            if values[index] != 0.0 {
                output.push_str(&format_duration_number(values[index]));
                output.push_str(suffix);
            }
        }
        if let Some(seconds) = format_duration_seconds(values[6], values[7], values[8], values[9]) {
            output.push_str(&seconds);
            output.push('S');
        } else if output.ends_with('T') {
            output.push_str("0S");
        }
    }
    NativeResult::Ok(vm.new_string_owned(output))
}

/// `Temporal.Duration.prototype.valueOf()` 始终拒绝隐式数值转换。
pub fn duration_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.Duration has no valueOf"))
}

/// `Temporal.Duration.prototype.abs()`，返回所有分量的绝对值副本。
pub fn duration_abs<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let mut values = duration_values(obj);
    for value in &mut values {
        *value = value.abs();
    }
    make_duration(vm, values)
}

/// `Temporal.Duration.prototype.negated()`，返回非零分量取反的副本。
pub fn duration_negated<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let mut values = duration_values(obj);
    for value in &mut values {
        if *value != 0.0 {
            *value = -*value;
        }
    }
    make_duration(vm, values)
}

/// `Temporal.Duration.prototype.add(other)`：分量相加并按规范平衡时间单位；
/// 任一侧含日历单位（years/months/weeks）时抛 RangeError（无 relativeTo 支持）。
pub fn duration_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let other_val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other = native_try!(duration_like_values(vm, other_val));
    let receiver = duration_values(obj);
    let values = native_try!(add_duration_values(vm, &receiver, &other));
    make_duration(vm, values)
}

/// 两个纯时间 duration（无 year/month/week）按分量精确求和并按最大非零单位平衡。
///
/// # 步骤
/// 1. 最大单位取两侧最大的非零时间单位（days=3 最大，ns=9 最小）。
/// 2. 分量在 i128 上按纳秒精确求和（f64 分量是精确整数，数学值不受 2^53 截断）。
/// 3. 从总纳秒向下按同号截断分解，进位到最大单位为止。
/// 4. 平衡后存储时间分量（含 days，𝔽 舍入）的纳秒加权和校验 2^53 秒上限。
///
/// # 边界与前提
/// - 任一侧含日历单位抛 RangeError（本批无 relativeTo 支持）。
/// - 全零与 nanosecond 分量和为 0 时直接返回零时长。
fn add_duration_values<H: VmHost>(vm: &mut H, receiver: &[f64; 10], other: &[f64; 10]) -> Result<[f64; 10], JsValue> {
    if receiver[..3].iter().any(|value| *value != 0.0) || other[..3].iter().any(|value| *value != 0.0) {
        return Err(crate::error::create_range_error(vm, "cannot add durations with calendar units"));
    }
    // 最大单位取 receiver 与参数中最大的非零时间单位（days=3 最大，ns=9 最小）。
    let mut largest = 9_usize;
    for index in 3..10 {
        if receiver[index] != 0.0 || other[index] != 0.0 {
            largest = index;
            break;
        }
    }
    if largest == 9 && receiver[9] + other[9] == 0.0 {
        return Ok([0.0; 10]);
    }
    // 分量按精确整数求和（f64 分量是精确整数；超过 2^53 的和需在 i128 上保持精确，规范按数学值计算）。
    let mut total_ns = 0_i128;
    const SUM_SCALES: [i128; 7] =
        [86_400_000_000_000, 3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    for (index, scale) in (3..10).zip(SUM_SCALES) {
        let Some(a) = duration_component_integer(receiver[index]) else {
            return Err(crate::error::create_range_error(vm, "invalid duration"));
        };
        let Some(b) = duration_component_integer(other[index]) else {
            return Err(crate::error::create_range_error(vm, "invalid duration"));
        };
        let Some(component) = a.checked_add(b).and_then(|value| value.checked_mul(scale)) else {
            return Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        let Some(updated) = total_ns.checked_add(component) else {
            return Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        total_ns = updated;
    }
    // 从总纳秒向下按截断除法分解，进位到 largest 为止（largest 单位可无界）。
    let mut values = [0.0; 10];
    let mut rem = total_ns;
    let mut unit = 9_usize;
    loop {
        if unit == largest {
            values[unit] = rem as f64;
            break;
        }
        let base = match unit {
            7..=9 => 1_000,
            6 | 5 => 60,
            _ => 24,
        };
        values[unit] = (rem % base) as f64;
        rem /= base;
        unit -= 1;
    }
    // 范围校验：判据作用在存储分量（平衡后 𝔽 舍入结果）的纳秒加权和上，而非输入
    // 精确和——内部槽位按 float64 可表示整数存储，转换可有损，舍入后分量可能越界；
    // 加权和同时覆盖逐分量检查的盲区（总和落在 [2^53 s, 2^53 s + 86400 s) 而各分量
    // 均低于上限的缝隙带）。
    const MAX_TIME_NANOSECONDS: i128 = (1_i128 << 53) * 1_000_000_000;
    let Some(stored_ns) = duration_time_nanoseconds(&values) else {
        return Err(crate::error::create_range_error(vm, "invalid duration"));
    };
    if stored_ns.abs() >= MAX_TIME_NANOSECONDS {
        return Err(crate::error::create_range_error(vm, "duration time fields are out of range"));
    }
    Ok(values)
}

/// `Temporal.Duration.prototype.subtract(other)`：等于 add(other.negated())。
///
/// # 边界与前提
/// - 继承 add 的无日历单位限制：任一侧含 year/month/week 抛 RangeError。
/// - 参数归一与 add 一致（字符串/对象/负值均支持）。
pub fn duration_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let other_val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other = native_try!(duration_like_values(vm, other_val));
    let receiver = duration_values(obj);
    let negated = std::array::from_fn(|index| if other[index] != 0.0 { -other[index] } else { 0.0 });
    let values = native_try!(add_duration_values(vm, &receiver, &negated));
    make_duration(vm, values)
}

/// `Temporal.Duration.prototype.equals(other)`：逐分量比较两个时长。
///
/// # 边界与前提
/// - receiver 须为 Duration（branding TypeError）。
/// - 参数经 duration_like_values 归一（无效字符串/混合符号抛 RangeError）。
/// - 全分量相等返回 true；仅数值相等比较，不涉及日历语义。
pub fn duration_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let other_val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other = native_try!(duration_like_values(vm, other_val));
    let receiver = duration_values(obj);
    let equal = (0..10).all(|index| receiver[index] == other[index]);
    NativeResult::Ok(JsValue::bool(equal))
}

/// `Temporal.Duration.prototype.sign`：首个非零分量的符号（-1/0/+1）。
pub fn duration_sign<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    for value in duration_values(obj) {
        if value != 0.0 {
            return NativeResult::Ok(JsValue::int(if value.is_sign_negative() { -1 } else { 1 }));
        }
    }
    NativeResult::Ok(JsValue::int(0))
}

/// `Temporal.Duration.prototype.blank`：所有分量是否全零。
pub fn duration_blank<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    NativeResult::Ok(JsValue::bool(duration_values(obj).iter().all(|value| *value == 0.0)))
}

/// `Temporal.Duration.prototype.toJSON()`：输出默认 ISO 字符串，忽略参数。
pub fn duration_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let receiver = args.first().copied().unwrap_or(0);
    duration_to_string(vm, &[receiver])
}

/// `Temporal.Duration.prototype.toLocaleString()`：默认 locale 下返回 ISO 字符串（无 Intl 依赖）。
pub fn duration_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let receiver = args.first().copied().unwrap_or(0);
    duration_to_string(vm, &[receiver])
}

/// 把 duration 应用相对点，返回目标 `(date, time_ns)`。
///
/// # 步骤
/// 1. days 及时间分量按 24h/day 汇总为纳秒，跨天部分拆成 delta_days 与一天内 target_time。
/// 2. 日历分量（year/month/week）与 delta_days 经 add_date_duration 加到相对点。
///
/// # 边界与前提
/// - 时间分量超出 i128 纳秒表示范围时抛 RangeError。
/// - 返回的 time_ns 带符号（负 duration 时可为负）。
fn duration_apply_to_relative<H: VmHost>(
    vm: &mut H, rel: (i128, i128, i128), values: &[f64; 10],
) -> Result<((i128, i128, i128), i128), JsValue> {
    let Some(time_ns_total) = duration_time_nanoseconds(values) else {
        return Err(crate::error::create_range_error(vm, "duration is out of range"));
    };
    let delta_days = time_ns_total / DAY_NS;
    let target_time = time_ns_total % DAY_NS;
    let mut date_parts = [0.0; 10];
    date_parts[0] = values[0];
    date_parts[1] = values[1];
    date_parts[2] = values[2];
    date_parts[3] = delta_days as f64;
    let end = add_date_duration(rel, &date_parts);
    // 目标日期须落在 ISO 日期范围内（约 ±10^8 天），否则抛 RangeError。
    if days_from_civil(end.0, end.1, end.2).abs() > MAX_ISO_DAY {
        return Err(crate::error::create_range_error(vm, "relativeTo plus duration is out of range"));
    }
    Ok((end, target_time))
}

/// `Temporal.Duration.compare(one, two, options)`：按相对点比较两个时长。
///
/// # 步骤
/// 1. one/two 各自 duration_like_values 归一（读序 one → two → options）。
/// 2. options.relativeTo 经 duration_relative_to_date 归一（含 ZDT/Instant/bag 分支）。
/// 3. 无 relativeTo 且无日历单位：按 duration_time_nanoseconds 直接比大小（fast path）。
/// 4. 无 relativeTo 但含日历单位：RangeError（calendar-possibly-required 语义）。
/// 5. 有 relativeTo：每个 duration 应用相对点得 (date, time)，先比日期再比时间（字典序）。
///
/// # 边界与前提
/// - 含日历单位必须提供 relativeTo，否则 RangeError。
/// - 时间分量按 24h/day 折算；比较的是应用后目标时刻，非分量数值。
pub fn duration_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let one_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let two_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let one = native_try!(duration_like_values(vm, one_val));
    let two = native_try!(duration_like_values(vm, two_val));
    // 全分量相等直接返回 0（含日历单位时也无需 relativeTo，对齐 instances-identical）。
    if one == two {
        return NativeResult::Ok(JsValue::int(0));
    }
    let options_value = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };
    let relative_raw = if options_value.is_undefined() {
        JsValue::undefined()
    } else {
        if !options_value.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        match temporal_option_value(vm, options, options_value, "relativeTo") {
            Ok(raw) => raw,
            Err(error) => return NativeResult::Err(error),
        }
    };
    let relative = native_try!(duration_relative_to_date(vm, relative_raw));

    let ordering = match relative {
        None => {
            // 无 relativeTo：含日历单位抛 RangeError，否则按归一纳秒比大小。
            if one[..3].iter().any(|value| *value != 0.0) || two[..3].iter().any(|value| *value != 0.0) {
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "relativeTo is required for calendar units",
                ));
            }
            let one_ns = native_try!(duration_time_nanoseconds(&one)
                .ok_or_else(|| { crate::error::create_range_error(vm, "duration is out of range") }));
            let two_ns = native_try!(duration_time_nanoseconds(&two)
                .ok_or_else(|| { crate::error::create_range_error(vm, "duration is out of range") }));
            one_ns.cmp(&two_ns)
        }
        Some(rel) => {
            // 有 relativeTo：各 duration 应用后先比日期再比时间。
            let (date1, time1) = native_try!(duration_apply_to_relative(vm, rel, &one));
            let (date2, time2) = native_try!(duration_apply_to_relative(vm, rel, &two));
            let date_cmp = compare_iso_date(date1, date2);
            if date_cmp != 0 {
                if date_cmp > 0 {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                }
            } else {
                time1.cmp(&time2)
            }
        }
    };
    NativeResult::Ok(JsValue::int(match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }))
}
/// `Temporal.Duration.prototype.round(roundTo)`：按最小单位舍入并按最大单位平衡。
///
/// # 步骤
/// 1. 解析 roundTo（字符串简写或对象）→ largestUnit/smallestUnit/roundingIncrement/roundingMode/relativeTo。
/// 2. relativeTo 经 duration_relative_to_date 归一；仅日历单位路径需要它。
/// 3. 纯时间路径（无 year/month/week 且 largest/smallest ≥ day）按 24h/day 汇总舍入。
/// 4. 日历单位路径把 duration 应用 relativeTo 后走 nudge_iso_difference 平衡舍入。
///
/// # 边界与前提
/// - 含日历单位（year/month/week 非零）或目标单位为日历单位时 relativeTo 缺失抛 RangeError。
/// - 时间分量按 24 小时/天折算，与 polyfill 的 24h-day 语义一致。
pub fn duration_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let round_to = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    if round_to.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "options parameter is required"));
    }

    let values = duration_values(obj);
    // 现有最大单位：首个非零分量；全零时视为 nanosecond。
    let mut existing_largest = 9_usize;
    for (index, value) in values.iter().enumerate() {
        if *value != 0.0 {
            existing_largest = index;
            break;
        }
    }

    // roundTo 字符串 => { smallestUnit: 字符串 }；否则必须为对象。
    let (largest_raw, relative_raw, increment_raw, mode_raw, smallest_raw) = if round_to.is_string() {
        (
            JsValue::undefined(),
            JsValue::undefined(),
            JsValue::undefined(),
            JsValue::undefined(),
            round_to,
        )
    } else {
        if !round_to.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "roundTo must be an object"));
        }
        let options_ptr = round_to.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "roundTo must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let largest_raw = native_try!(temporal_option_value(vm, options, round_to, "largestUnit"));
        let relative_raw = native_try!(temporal_option_value(vm, options, round_to, "relativeTo"));
        let increment_raw = native_try!(temporal_option_value(vm, options, round_to, "roundingIncrement"));
        let mode_raw = native_try!(temporal_option_value(vm, options, round_to, "roundingMode"));
        let smallest_raw = native_try!(temporal_option_value(vm, options, round_to, "smallestUnit"));
        (largest_raw, relative_raw, increment_raw, mode_raw, smallest_raw)
    };

    // largestUnit：允许 "auto"；缺失/undefined 视为未提供。
    let largest_provided = !largest_raw.is_undefined();
    let largest_index = if largest_raw.is_undefined() {
        None
    } else {
        let unit = native_try!(temporal_option_string(vm, largest_raw));
        if unit == "auto" {
            None
        } else {
            match plain_date_time_unit_index(&unit) {
                Some(index) => Some(index),
                None => {
                    return NativeResult::Err(crate::error::create_range_error(vm, "invalid largestUnit"));
                }
            }
        }
    };

    // roundingIncrement：ToIntegerOrInfinity 舍入后需在 [1, 10^9] 内。
    let increment_value = if increment_raw.is_undefined() {
        1.0
    } else {
        native_try!(temporal_option_number(vm, increment_raw))
    };
    let increment = increment_value.trunc();
    if !increment.is_finite() || !(1.0..=1_000_000_000.0).contains(&increment) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment as i128;

    // roundingMode：缺失时 halfExpand。
    let mode = if mode_raw.is_undefined() {
        InstantRoundingMode::HalfExpand
    } else {
        let mode_string = native_try!(temporal_option_string(vm, mode_raw));
        match instant_rounding_mode(&mode_string) {
            Some(mode) => mode,
            None => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
            }
        }
    };

    // smallestUnit：缺失时 nanosecond。
    let smallest_provided = !smallest_raw.is_undefined();
    let smallest_index = if smallest_raw.is_undefined() {
        9
    } else {
        let unit = native_try!(temporal_option_string(vm, smallest_raw));
        match plain_date_time_unit_index(&unit) {
            Some(index) => index,
            None => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid smallestUnit"));
            }
        }
    };

    // 默认最大单位：现有最大单位与最小单位中较大（索引较小）的那个。
    let default_largest = if existing_largest < smallest_index {
        existing_largest
    } else {
        smallest_index
    };
    let largest = largest_index.unwrap_or(default_largest);

    // 至少一个单位需要显式提供；largest 不能小于 smallest。
    if !smallest_provided && !largest_provided {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "at least one of smallestUnit or largestUnit is required",
        ));
    }
    if smallest_index < largest {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "largestUnit cannot be smaller than smallestUnit",
        ));
    }

    // 舍入增量上限：单位越小可除以上一级；日历单位（year/month/week/day）无整除约束。
    const MAX_INCREMENT: [i128; 10] = [0, 0, 0, 0, 24, 60, 60, 1000, 1000, 1000];
    let max_increment = MAX_INCREMENT[smallest_index];
    if max_increment != 0 && (increment >= max_increment || max_increment % increment != 0) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    // 日历单位的 increment 要求 largestUnit 与 smallestUnit 相同（防止窗口舍入歧义）。
    if increment > 1 && smallest_index == 3 && largest != smallest_index {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "rounding increments of calendar units require largestUnit to equal smallestUnit",
        ));
    }

    // relativeTo 仅在日历路径需要时读取（解析失败直接抛错）。
    let relative_date = match duration_relative_to_date(vm, relative_raw) {
        Ok(relative) => relative,
        Err(error) => return NativeResult::Err(error),
    };

    // 日历单位存在或目标为日历单位时走 relativeTo 路径，否则走纯时间路径。
    let needs_relative = values[..3].iter().any(|value| *value != 0.0) || largest < 3 || smallest_index < 3;
    if !needs_relative {
        // 纯时间路径：按 24 小时/天将 days..nanoseconds 汇总为纳秒，按 smallest 舍入后平衡到 largest。
        let Some(time_ns) = duration_time_nanoseconds(&values) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        const UNIT_NS: [i128; 7] =
            [86_400_000_000_000, 3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
        let quantum = if smallest_index == 3 {
            increment * UNIT_NS[0]
        } else {
            increment * UNIT_NS[smallest_index - 3]
        };
        let Some(rounded) = round_instant_difference(time_ns, quantum, mode) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        let mut result = [0.0; 10];
        if smallest_index == 3 {
            result[3] = (rounded / UNIT_NS[0]) as f64;
        } else {
            // 与 duration_add 相同的同号模分解：从 ns 向 largest 进位。
            let mut rem = rounded;
            let mut unit = 9_usize;
            loop {
                if unit == largest {
                    result[unit] = rem as f64;
                    break;
                }
                let base = match unit {
                    7..=9 => 1_000,
                    6 | 5 => 60,
                    _ => 24,
                };
                result[unit] = (rem % base) as f64;
                rem /= base;
                unit -= 1;
            }
        }
        // 范围校验：对每个时间分量按纳秒刻度检查是否达到 2^53 秒上限。
        const MAX_TIME_NANOSECONDS: f64 = (1_i128 << 53) as f64 * 1_000_000_000.0;
        const UNIT_SCALES: [f64; 7] = [
            86_400_000_000_000.0,
            3_600_000_000_000.0,
            60_000_000_000.0,
            1_000_000_000.0,
            1_000_000.0,
            1_000.0,
            1.0,
        ];
        for (index, scale) in (3..10).zip(UNIT_SCALES) {
            if result[index] != 0.0 && result[index].abs() * scale >= MAX_TIME_NANOSECONDS {
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "duration time fields are out of range",
                ));
            }
        }
        return make_duration(vm, result);
    }

    // 日历路径：需要 relativeTo，把 duration 应用后按 nudge_iso_difference 舍入平衡。
    let Some(rel) = relative_date else {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "a starting point is required for rounding calendar units",
        ));
    };
    let Some(time_ns_total) = duration_time_nanoseconds(&values) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
    };
    let delta_days = time_ns_total / DAY_NS;
    let target_time = time_ns_total % DAY_NS;
    let mut date_parts = [0.0; 10];
    date_parts[0] = values[0];
    date_parts[1] = values[1];
    date_parts[2] = values[2];
    date_parts[3] = delta_days as f64;
    let end = add_date_duration(rel, &date_parts);
    let settings = DifferenceSettings {
        largest_index: largest,
        smallest_index,
        increment,
        mode,
    };
    let result = match nudge_iso_difference(vm, rel, 0, end, target_time, settings, false, 9) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
    make_duration(vm, result)
}

macro_rules! duration_getter {
    ($name:ident, $index:expr) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let ptr = native_try!(receiver_obj(vm, args));
            let obj = unsafe { &*ptr };
            native_try!(ensure_duration(vm, obj));
            NativeResult::Ok(JsValue::float(get_double_prop(obj, $index)))
        }
    };
}

duration_getter!(duration_years, 0);
duration_getter!(duration_months, 1);
duration_getter!(duration_weeks, 2);
duration_getter!(duration_days, 3);
duration_getter!(duration_hours, 4);
duration_getter!(duration_minutes, 5);
duration_getter!(duration_seconds, 6);
duration_getter!(duration_milliseconds, 7);
duration_getter!(duration_microseconds, 8);
duration_getter!(duration_nanoseconds, 9);
