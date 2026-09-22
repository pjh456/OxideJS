//! Temporal.PlainTime：ISO 时间解析、构造、getter、with/until/since 与加减舍入。

use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::common::native_try;
use super::{
    difference_core, duration_component_integer, duration_like_values, ensure_plain_time, get_double_prop,
    initialize_temporal_receiver, instant_rounding_mode, instant_string_without_annotations, is_ctor_call,
    make_plain_time, parse_any_offset_minutes, parse_difference_settings, parse_fractional_second_digits,
    parse_iso_date, receiver_obj, reject_partial_object_with_calendar_or_time_zone, round_instant_ns,
    temporal_number_component, temporal_option_number, temporal_option_string, temporal_option_value,
    temporal_overflow, valid_offset_fraction, zoned_date_time_plain_parts, FractionalSecondDigitsInput,
};

/// 时间串的日期歧义判定：形如 YYYY-MM / MMDD / YYYYMM / MM-DD 且作为日期合法 → 歧义。
///
/// # 边界与前提
/// - 2 月按闰年处理（0229 判歧义、0230 不判）。
/// - 月/日非法（13、00、2 月 30 日等）不算歧义，按时间解析。
fn is_ambiguous_date_string(s: &str) -> bool {
    let bytes = s.as_bytes();
    let all_digits = |range: std::ops::Range<usize>| {
        bytes.get(range.clone()).is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    let two_digits = |at: usize| -> Option<i32> {
        if at + 1 >= bytes.len() {
            None
        } else {
            Some((bytes[at] - b'0') as i32 * 10 + (bytes[at + 1] - b'0') as i32)
        }
    };
    let day_in_month = |month: i32, day: i32| -> bool {
        // 2 月按闰年（29 天）判定歧义。
        let max = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => 29,
            _ => return false,
        };
        day >= 1 && day <= max
    };
    match s.len() {
        4 if all_digits(0..4) => match (two_digits(0), two_digits(2)) {
            (Some(month), Some(day)) => day_in_month(month, day),
            _ => false,
        },
        6 if all_digits(0..6) => match two_digits(4) {
            Some(m) => (1..=12).contains(&m),
            None => false,
        },
        5 if bytes[2] == b'-' && all_digits(0..2) && all_digits(3..5) => match (two_digits(0), two_digits(3)) {
            (Some(month), Some(day)) => day_in_month(month, day),
            _ => false,
        },
        7 if bytes[4] == b'-' && all_digits(0..4) && all_digits(5..7) => match two_digits(5) {
            Some(m) => (1..=12).contains(&m),
            None => false,
        },
        _ => false,
    }
}

/// 解析时间主体（时[:分[:秒[.小数]]] 或 HHMM / HHMMSS），闰秒按前一秒。
fn parse_plain_clock(clock: &str) -> Option<f64> {
    let (clock, fraction) = match clock.find(['.', ',']) {
        Some(index) => (&clock[..index], Some(&clock[index + 1..])),
        None => (clock, None),
    };
    let subsecond = match fraction {
        Some(digits) => {
            if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let mut padded = digits.to_string();
            padded.extend(std::iter::repeat('0').take(9 - digits.len()));
            padded.parse::<u32>().ok()?
        }
        None => 0,
    };
    let (hour, minute, second) = if clock.contains(':') {
        let fields: Vec<&str> = clock.split(':').collect();
        if fields.is_empty()
            || fields.len() > 3
            || fields
                .iter()
                .any(|field| field.len() != 2 || !field.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
        if fraction.is_some() && fields.len() < 3 {
            return None;
        }
        (
            fields[0].parse::<u32>().ok()?,
            fields.get(1).map_or(Ok(0), |field| field.parse()).ok()?,
            fields.get(2).map_or(Ok(0), |field| field.parse()).ok()?,
        )
    } else {
        if !clock.bytes().all(|byte| byte.is_ascii_digit())
            || clock.len() % 2 != 0
            || clock.len() > 6
            || clock.is_empty()
        {
            return None;
        }
        if fraction.is_some() && clock.len() < 6 {
            return None;
        }
        let read2 = |at: usize| (clock.as_bytes()[at] - b'0') as u32 * 10 + (clock.as_bytes()[at + 1] - b'0') as u32;
        (
            read2(0),
            if clock.len() >= 4 { read2(2) } else { 0 },
            if clock.len() >= 6 { read2(4) } else { 0 },
        )
    };
    let second = if second == 60 { 59 } else { second };
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(
        hour as f64 * 3_600_000_000_000.0
            + minute as f64 * 60_000_000_000.0
            + second as f64 * 1_000_000_000.0
            + subsecond as f64,
    )
}

/// ParseTemporalTimeString：时间串 → 当日纳秒（offset/注解被忽略、Z 拒绝）。
///
/// # 边界与前提
/// - Z/z designator → None；纯日期串 → None（不隐式午夜）。
/// - 日期+时间（"1976-11-18T12:34..."）→ 取 T 后时间部分；空格分隔需带日期部分。
/// - 无 T 的数字/连字符形式先做日期歧义判定：日期合法 → None（须 T 前缀）；
///   日期非法 → 按时间解析（HHMM-UU 形式，offset 被忽略）。
/// - 小数秒 ≤9 位；offset 小数秒 ≤9 位；闰秒按前一秒。
pub(crate) fn parse_plain_time_string(input: &str) -> Option<f64> {
    let text = input.trim();
    if text.contains('\u{2212}') {
        return None;
    }
    let body = instant_string_without_annotations(text)?;
    if body.contains(['Z', 'z']) {
        return None;
    }
    let sep = body.find(['T', 't', ' ']);
    let (date_part, time_part, has_t) = match sep {
        Some(0) => ("", &body[1..], matches!(body.as_bytes().first(), Some(b'T' | b't'))),
        Some(index) => (&body[..index], &body[index + 1..], matches!(body.as_bytes()[index], b'T' | b't')),
        None => ("", body, false),
    };
    if !has_t && sep == Some(0) {
        // 前导空格不能替代 T 前缀（无日期部分的时间串）。
        return None;
    }
    if !date_part.is_empty() {
        // 完整日期部分必须可解析（负零年等由 parse_iso_date 拒绝）。
        if parse_iso_date(date_part).is_err() {
            return None;
        }
        return parse_plain_time_spec(time_part);
    }
    if !has_t && !time_part.contains(':') && is_ambiguous_date_string(time_part) {
        return None;
    }
    parse_plain_time_spec(time_part)
}

/// 时间主体 + 尾部 offset（剥离并忽略）：offset 语法非法返回 None。
fn parse_plain_time_spec(s: &str) -> Option<f64> {
    let offset_start = s
        .char_indices()
        .rev()
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index));
    let (clock, offset) = match offset_start {
        Some(index) => (&s[..index], &s[index..]),
        None => (s, ""),
    };
    if !offset.is_empty() && (parse_any_offset_minutes(offset).is_none() || !valid_offset_fraction(offset)) {
        return None;
    }
    parse_plain_clock(clock)
}

/// ToTemporalTime：PlainTime 对象 / ZDT / PlainDateTime / 字符串 / property bag → 当日纳秒。
///
/// # 步骤
/// 1. undefined → 0（午夜）；字符串走 parse_plain_time_string（失败 RangeError）。
/// 2. PlainTime 读槽 0；ZDT 用其自身时区取本地时间；PlainDateTime 读时间槽。
/// 3. 其余对象按 ToTemporalTimeRecord 读时间字段（无字段 TypeError）。
///
/// # 边界与前提
/// - 非字符串原始值（number/bigint/null/boolean）→ TypeError；Symbol → TypeError。
/// - bag 字段缺省 0（完整模式），越界值按 constrain 钳制（second=60 → 59）。
pub(crate) fn plain_time_like_ns<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    if value.is_undefined() {
        return Ok(0.0);
    }
    if value.is_string() {
        return parse_plain_time_string(&to_string(value))
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid ISO 8601 time"));
    }
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_plain_time_obj() {
                return Ok(get_double_prop(obj, 0));
            }
            if obj.is_zoned_date_time_obj() {
                return zoned_date_time_plain_parts(vm, obj).map(|(_, _, _, time_ns)| time_ns);
            }
            if obj.is_plain_date_time_obj() {
                return Ok(get_double_prop(obj, 3));
            }
            return plain_time_bag_ns(vm, value, obj);
        }
    }
    Err(crate::error::create_type_error(vm, "cannot convert to PlainTime"))
}

/// ToTemporalTimeRecord（完整模式）：按字母序读时间字段，缺省 0，无字段 TypeError。
fn plain_time_bag_ns<H: VmHost>(vm: &mut H, value: JsValue, obj: &JsObject) -> Result<f64, JsValue> {
    let hour_raw = temporal_option_value(vm, obj, value, "hour")?;
    let microsecond_raw = temporal_option_value(vm, obj, value, "microsecond")?;
    let millisecond_raw = temporal_option_value(vm, obj, value, "millisecond")?;
    let minute_raw = temporal_option_value(vm, obj, value, "minute")?;
    let nanosecond_raw = temporal_option_value(vm, obj, value, "nanosecond")?;
    let second_raw = temporal_option_value(vm, obj, value, "second")?;
    if [hour_raw, microsecond_raw, millisecond_raw, minute_raw, nanosecond_raw, second_raw]
        .iter()
        .all(|raw| raw.is_undefined())
    {
        return Err(crate::error::create_type_error(vm, "no time units present"));
    }
    let mut convert = |raw: JsValue| -> Result<f64, JsValue> {
        if raw.is_undefined() {
            Ok(0.0)
        } else {
            let number = temporal_option_number(vm, raw)?;
            if !number.is_finite() {
                return Err(crate::error::create_range_error(vm, "invalid time component"));
            }
            Ok(number.trunc())
        }
    };
    let hour = convert(hour_raw)?;
    let microsecond = convert(microsecond_raw)?;
    let millisecond = convert(millisecond_raw)?;
    let minute = convert(minute_raw)?;
    let nanosecond = convert(nanosecond_raw)?;
    let second = convert(second_raw)?;
    // RegulateTime（constrain）：越界钳制（second=60 → 59）。
    let hour = hour.clamp(0.0, 23.0);
    let minute = minute.clamp(0.0, 59.0);
    let second = second.clamp(0.0, 59.0);
    let millisecond = millisecond.clamp(0.0, 999.0);
    let microsecond = microsecond.clamp(0.0, 999.0);
    let nanosecond = nanosecond.clamp(0.0, 999.0);
    Ok(hour * 3_600_000_000_000.0
        + minute * 60_000_000_000.0
        + second * 1_000_000_000.0
        + millisecond * 1_000_000.0
        + microsecond * 1_000.0
        + nanosecond)
}

pub(crate) fn valid_plain_time(hour: u32, minute: u32, second: u32, ms: u32, us: u32, ns: u32) -> bool {
    hour <= 23 && minute <= 59 && second <= 59 && ms <= 999 && us <= 999 && ns <= 999
}

/// `Temporal.PlainTime` 构造器：`new PlainTime(h, m, s, ms, us, ns)`，缺省为 0。
pub fn plain_time_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().plain_time_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.PlainTime cannot be invoked without 'new'",
        ));
    }
    // 时间分量：缺参或显式 undefined 取默认 0（undefined 是合法分量，与 PD
    // 缺参抛错相反）；其余经 number-only 路径（Symbol/BigInt → TypeError，
    // NaN/±Inf/不可解析串 → RangeError）。
    let mut get = |i: usize| -> Result<f64, JsValue> {
        let raw = if args.len() > i { vm.reg(args[i]) } else { JsValue::undefined() };
        if raw.is_undefined() {
            return Ok(0.0);
        }
        temporal_number_component(vm, raw)
    };
    // 负值在 u32 转换前检查：截断后为负的整分量（含负分数整数化为负）须先抛错，
    // 不能让 f64→u32 饱和把其静默归 0 而逃过范围检查。
    let hour_value = native_try!(get(1));
    let minute_value = native_try!(get(2));
    let second_value = native_try!(get(3));
    let ms_value = native_try!(get(4));
    let us_value = native_try!(get(5));
    let ns_value = native_try!(get(6));
    if [hour_value, minute_value, second_value, ms_value, us_value, ns_value]
        .iter()
        .any(|value| *value < 0.0)
    {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time component"));
    }
    // f64 截断后可能超出 u32 范围，先做有界转换。
    let mut to_u32 = |v: f64| -> Result<u32, JsValue> {
        if v >= 0.0 && v <= u32::MAX as f64 {
            Ok(v as u32)
        } else {
            Err(crate::error::create_range_error(vm, "invalid time component"))
        }
    };
    let hour = native_try!(to_u32(hour_value));
    let minute = native_try!(to_u32(minute_value));
    let second = native_try!(to_u32(second_value));
    let ms = native_try!(to_u32(ms_value));
    let us = native_try!(to_u32(us_value));
    let ns = native_try!(to_u32(ns_value));
    if !valid_plain_time(hour, minute, second, ms, us, ns) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time component"));
    }
    let total_ns = hour as f64 * 3.6e12
        + minute as f64 * 6e10
        + second as f64 * 1e9
        + ms as f64 * 1e6
        + us as f64 * 1e3
        + ns as f64;
    initialize_temporal_receiver(vm, args, JsObject::OBJ_TYPE_PLAIN_TIME, [JsValue::float(total_ns)])
}

/// 拆解午夜后纳秒为各分量。
/// 带符号分解午夜后纳秒为时/分/秒/毫秒/微秒/纳秒（各分量向零截断，保持同号）。
pub(crate) fn plain_time_components_signed(total_ns: i128) -> [i128; 6] {
    const SCALES: [i128; 5] = [3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000];
    let mut values = [0_i128; 6];
    let mut remainder = total_ns;
    for (index, scale) in SCALES.iter().enumerate() {
        values[index] = remainder / scale;
        remainder %= scale;
    }
    values[5] = remainder;
    values
}

pub(crate) fn plain_time_components(total_ns: f64) -> (u32, u32, u32, u32, u32, u32) {
    let total = total_ns as u64;
    let ns = total % 1_000;
    let us = total / 1_000 % 1_000;
    let ms = total / 1_000_000 % 1_000;
    let second = total / 1_000_000_000 % 60;
    let minute = total / 60_000_000_000 % 60;
    let hour = total / 3_600_000_000_000 % 24;
    (hour as u32, minute as u32, second as u32, ms as u32, us as u32, ns as u32)
}

fn plain_time_get<H: VmHost>(vm: &mut H, args: &[u8], select: fn(u32, u32, u32, u32, u32, u32) -> u32) -> NativeResult {
    let ptr = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(e) => return NativeResult::Err(e),
    };
    let obj = unsafe { &*ptr };
    if let Err(e) = ensure_plain_time(vm, obj) {
        return NativeResult::Err(e);
    }
    let (h, m, s, ms, us, ns) = plain_time_components(get_double_prop(obj, 0));
    NativeResult::Ok(JsValue::float(select(h, m, s, ms, us, ns) as f64))
}

/// `Temporal.PlainTime.prototype.hour` getter。
pub fn plain_time_hour<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_get(vm, args, |h, _, _, _, _, _| h)
}

/// `Temporal.PlainTime.prototype.minute` getter。
pub fn plain_time_minute<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_get(vm, args, |_, m, _, _, _, _| m)
}

/// `Temporal.PlainTime.prototype.second` getter。
pub fn plain_time_second<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_get(vm, args, |_, _, s, _, _, _| s)
}

/// `Temporal.PlainTime.prototype.millisecond` getter。
pub fn plain_time_millisecond<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_get(vm, args, |_, _, _, ms, _, _| ms)
}

/// `Temporal.PlainTime.prototype.microsecond` getter。
pub fn plain_time_microsecond<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_get(vm, args, |_, _, _, _, us, _| us)
}

/// `Temporal.PlainTime.prototype.nanosecond` getter。
pub fn plain_time_nanosecond<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_get(vm, args, |_, _, _, _, _, ns| ns)
}

/// `Temporal.PlainTime.prototype.with(temporalTimeLike [, options])`：替换
/// receiver 的指定时间分量，返回新 PlainTime。
///
/// # 步骤
/// 1. branding + ToTemporalTimeLike（非对象 / Temporal 实例 / calendar、
///    timeZone 有定义 → TypeError；字段按字典序读 hour → microsecond →
///    millisecond → minute → nanosecond → second，非数字 → TypeError，
///    NaN/±Inf → RangeError）。
/// 2. 分量全 undefined → TypeError。
/// 3. options → overflow（缺省 constrain）。
/// 4. RegulateTime：constrain 钳制；reject 先校验范围再截断（负 / 超界分量
///    立即抛 RangeError）。
/// 5. 合并分量算午夜后纳秒，构造新 PlainTime。
///
/// # 边界与前提
/// - 字段读取先于选项解析：即使选项本身类型不合法，也先读完 partial 字段并让字段的
///   类型错误先抛出，与 ZonedDateTime 的 with 一致。
/// - receiver 分量取自午夜后纳秒槽；partial 中 undefined 的分量保留 receiver 值。
pub fn plain_time_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_time(vm, obj));
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    native_try!(reject_partial_object_with_calendar_or_time_zone(vm, value));

    // 字段读取按字典序，undefined 不覆盖；number-only 路径（Symbol/BigInt →
    // TypeError，NaN/±Inf → RangeError）。
    let bag = unsafe { &*value.as_js_object_ptr() };
    let read = |vm: &mut H, name: &str| -> Result<Option<f64>, JsValue> {
        let raw = temporal_option_value(vm, bag, value, name)?;
        if raw.is_undefined() {
            Ok(None)
        } else {
            Ok(Some(temporal_number_component(vm, raw)?))
        }
    };
    let hour = native_try!(read(vm, "hour"));
    let microsecond = native_try!(read(vm, "microsecond"));
    let millisecond = native_try!(read(vm, "millisecond"));
    let minute = native_try!(read(vm, "minute"));
    let nanosecond = native_try!(read(vm, "nanosecond"));
    let second = native_try!(read(vm, "second"));
    if [hour, microsecond, millisecond, minute, nanosecond, second]
        .iter()
        .all(|c| c.is_none())
    {
        return NativeResult::Err(crate::error::create_type_error(vm, "no properties present"));
    }

    // options：字段读取完成后解析 overflow。
    let constrain = native_try!(temporal_overflow(vm, args));

    // RegulateTime：partial 有定义的分量覆盖 receiver 分量。
    let (recv_hour, recv_minute, recv_second, recv_ms, recv_us, recv_ns) =
        plain_time_components(get_double_prop(obj, 0));
    let merged = [
        hour.unwrap_or(recv_hour as f64),
        minute.unwrap_or(recv_minute as f64),
        second.unwrap_or(recv_second as f64),
        millisecond.unwrap_or(recv_ms as f64),
        microsecond.unwrap_or(recv_us as f64),
        nanosecond.unwrap_or(recv_ns as f64),
    ];
    let values = if constrain {
        [
            merged[0].clamp(0.0, 23.0),
            merged[1].clamp(0.0, 59.0),
            merged[2].clamp(0.0, 59.0),
            merged[3].clamp(0.0, 999.0),
            merged[4].clamp(0.0, 999.0),
            merged[5].clamp(0.0, 999.0),
        ]
    } else {
        // reject 语义须先做范围校验再截断：负分量或超上限分量立即抛错，
        // 不允许 f64→u32 饱和把负值静默归 0 而逃过范围检查。
        let limits = [23.0, 59.0, 59.0, 999.0, 999.0, 999.0];
        if merged.iter().zip(limits.iter()).any(|(v, limit)| *v < 0.0 || *v > *limit) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid time component"));
        }
        merged
    };
    let total_ns = values[0] * 3_600_000_000_000.0
        + values[1] * 60_000_000_000.0
        + values[2] * 1_000_000_000.0
        + values[3] * 1_000_000.0
        + values[4] * 1_000.0
        + values[5];
    make_plain_time(vm, total_ns)
}

/// 按是否含秒与小数位格式化当日纳秒为 `HH:MM:SS[.frac]`。
///
/// # 边界与前提
/// - `include_seconds=false` 时忽略小数位，仅输出 `HH:MM`。
/// - `Some(0)` 不输出小数；`Some(digits)` 固定补足位数；`None` 按实际非零亚秒去尾零。
pub(crate) fn format_plain_time_iso(time_ns: i128, include_seconds: bool, fractional_digits: Option<usize>) -> String {
    let hour = time_ns / 3_600_000_000_000;
    let minute = time_ns / 60_000_000_000 % 60;
    let second = time_ns / 1_000_000_000 % 60;
    let subsecond = time_ns % 1_000_000_000;
    let mut output = format!("{hour:02}:{minute:02}");
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
                output.push('.');
                output.push_str(format!("{subsecond:09}").trim_end_matches('0'));
            }
            None => {}
        }
    }
    output
}

/// `Temporal.PlainTime.prototype.toString(options)`：按精度、舍入模式与最小单位输出 ISO 8601 时间。
pub fn plain_time_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_time(vm, obj));
    let total_ns = get_double_prop(obj, 0) as i128;
    let options_value = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (fractional_input, mode_value, smallest_value) = if options_value.is_undefined() {
        (FractionalSecondDigitsInput::Auto, "trunc".to_string(), None)
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
        (fractional, mode, smallest)
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
    // 舍入后可能跨到次日（rounding-cross-midnight），取模保持 0-24 域。
    const DAY_NS: i128 = 86_400_000_000_000;
    let Some(rounded_ns) = round_instant_ns(total_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time"));
    };
    let time_ns = rounded_ns.rem_euclid(DAY_NS);
    let output = format_plain_time_iso(time_ns, include_seconds, output_digits);
    NativeResult::Ok(vm.new_string_owned(output))
}

/// `Temporal.PlainTime.prototype.toJSON()`：输出默认 ISO 时间字符串，忽略参数。
pub fn plain_time_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_default_string(vm, args)
}

/// `Temporal.PlainTime.prototype.toLocaleString()`：当前使用稳定的默认 ISO 时间表示。
pub fn plain_time_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_default_string(vm, args)
}

/// 输出无 options 的默认 PlainTime 串（全秒 + 非零亚秒）。
fn plain_time_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_time(vm, obj));
    let total_ns = get_double_prop(obj, 0) as i128;
    NativeResult::Ok(vm.new_string_owned(format_plain_time_iso(total_ns, true, None)))
}

/// `Temporal.PlainTime.prototype.valueOf()`：Temporal 对象禁止转原始值。
pub fn plain_time_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.PlainTime has no valueOf"))
}

/// `Temporal.PlainTime.from(item, options)`：从字符串、PlainTime/ZDT/PlainDateTime 实例或 property bag 构造。
pub fn plain_time_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if value.is_string() {
        // 规范顺序：先 ParseTemporalTimeString，再 ToTemporalOverflow(options)。
        let total_ns = match parse_plain_time_string(&to_string(value)) {
            Some(ns) => ns,
            None => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO 8601 time"));
            }
        };
        native_try!(temporal_overflow(vm, args));
        make_plain_time(vm, total_ns)
    } else {
        // 对象分支：实例复制 / property bag 统一走 plain_time_like_ns（含缺省 0 与约束钳制）。
        native_try!(temporal_overflow(vm, args));
        let total_ns = native_try!(plain_time_like_ns(vm, value));
        make_plain_time(vm, total_ns)
    }
}

/// `Temporal.PlainTime.prototype.equals(other)`：各分量全等返回 true，否则 false。
///
/// # 边界与前提
/// - other 经 `plain_time_like_ns` 归一（实例/字符串/property bag），无法转换抛 TypeError/RangeError。
pub fn plain_time_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(error) => return NativeResult::Err(error),
    };
    let obj = unsafe { &*ptr };
    if let Err(error) = ensure_plain_time(vm, obj) {
        return NativeResult::Err(error);
    }
    let receiver_ns = get_double_prop(obj, 0);
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let other_ns = match plain_time_like_ns(vm, other_val) {
        Ok(ns) => ns,
        Err(error) => return NativeResult::Err(error),
    };
    NativeResult::Ok(JsValue::bool(receiver_ns as i128 == other_ns as i128))
}

/// `Temporal.PlainTime.compare(one, two)`：静态比较，按午夜后纳秒返回 -1、0 或 1。
pub fn plain_time_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let one = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let two = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let one_ns = match plain_time_like_ns(vm, one) {
        Ok(ns) => ns,
        Err(error) => return NativeResult::Err(error),
    };
    let two_ns = match plain_time_like_ns(vm, two) {
        Ok(ns) => ns,
        Err(error) => return NativeResult::Err(error),
    };
    let result = match (one_ns as i128).cmp(&(two_ns as i128)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    NativeResult::Ok(JsValue::int(result))
}

// ───────────────────── PlainTime until / since ─────────────────────

/// until/since 核心：receiver 与 other 归一为午夜后纳秒，经 `difference_core` 计算差值。
///
/// # 步骤
/// 1. branding receiver 取槽 0 ns，other 经 `plain_time_like_ns` 归一。
/// 2. `parse_difference_settings` 解析 options（default_largest=4=hour）。
/// 3. 日期固定为 epoch（0,0,0），调 `difference_core` 输出 Duration。
///
/// # 边界与前提
/// - since 复用 same 语义并整体取反（difference_core 内部处理），不调换两端。
fn plain_time_difference<H: VmHost>(vm: &mut H, args: &[u8], since: bool) -> NativeResult {
    let ptr = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(error) => return NativeResult::Err(error),
    };
    let obj = unsafe { &*ptr };
    if let Err(error) = ensure_plain_time(vm, obj) {
        return NativeResult::Err(error);
    }
    let receiver_ns = get_double_prop(obj, 0);
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let other_ns = match plain_time_like_ns(vm, other_val) {
        Ok(ns) => ns,
        Err(error) => return NativeResult::Err(error),
    };
    let options_value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let settings = match parse_difference_settings(vm, options_value, false, 4) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    difference_core(vm, (0, 0, 0), receiver_ns as i128, (0, 0, 0), other_ns as i128, settings, since, 9)
}

/// `Temporal.PlainTime.prototype.until(other, options)`。
pub fn plain_time_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_difference(vm, args, false)
}

/// `Temporal.PlainTime.prototype.since(other, options)`。
pub fn plain_time_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_difference(vm, args, true)
}

// ───────────────────── PlainTime add / subtract ─────────────────────

/// add/subtract 核心：receiver 午夜后纳秒叠加时长的"时间域"增量。
///
/// # 步骤
/// 1. branding receiver 取槽 0 ns。
/// 2. `duration_like_values` 归一 duration，`temporal_overflow` 解析 options（读序：duration → options）。
/// 3. 仅取 hours 起的时间字段换算纳秒增量（日期字段 days 及以上规范忽略）。
/// 4. 按 sign 叠加后 `rem_euclid(DAY_NS)` 保持 0-24 域 → `make_plain_time`。
///
/// # 边界与前提
/// - duration 全 0 → 值不变的新对象（blank-duration 语义）。
/// - duration 含日期字段（y/m/w/d）不报错，规范对 PlainTime 直接忽略。
fn plain_time_apply_duration<H: VmHost>(vm: &mut H, args: &[u8], sign: i64) -> NativeResult {
    let ptr = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(error) => return NativeResult::Err(error),
    };
    let obj = unsafe { &*ptr };
    if let Err(error) = ensure_plain_time(vm, obj) {
        return NativeResult::Err(error);
    }
    let time_ns = get_double_prop(obj, 0) as i128;
    let duration_like = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = match duration_like_values(vm, duration_like) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
    if let Err(error) = temporal_overflow(vm, args) {
        return NativeResult::Err(error);
    }
    const DAY_NS: i128 = 86_400_000_000_000;
    // 时间字段（hours 起）单独换算纳秒；日期字段 days 及以上对 PlainTime 无意义直接忽略。
    let [_, _, _, _, h, min, s, ms, us, ns] = values;
    let time_delta = duration_component_integer(h).unwrap_or(0) * 3_600_000_000_000
        + duration_component_integer(min).unwrap_or(0) * 60_000_000_000
        + duration_component_integer(s).unwrap_or(0) * 1_000_000_000
        + duration_component_integer(ms).unwrap_or(0) * 1_000_000
        + duration_component_integer(us).unwrap_or(0) * 1_000
        + duration_component_integer(ns).unwrap_or(0);
    // 时间溢出跨午夜：rem_euclid 保持 0-24 域。
    let new_time_ns = (time_ns + time_delta * sign as i128).rem_euclid(DAY_NS);
    make_plain_time(vm, new_time_ns as f64)
}

/// `Temporal.PlainTime.prototype.add(durationLike, options)`。
pub fn plain_time_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_apply_duration(vm, args, 1)
}

/// `Temporal.PlainTime.prototype.subtract(durationLike, options)`。
pub fn plain_time_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_apply_duration(vm, args, -1)
}

// ───────────────────── PlainTime round ─────────────────────

/// PlainTime round 的最小单位表：小时..纳秒（不含 day）。
/// 返回 (单位纳秒, 最大增量)。最大增量 = MaximumTemporalDurationRoundingIncrement：
/// hour→24、minute→60、second→60、millisecond→1000、microsecond→1000、nanosecond→1000。
pub(crate) fn plain_time_round_unit(value: &str) -> Option<(i128, i128)> {
    match value {
        "hour" | "hours" => Some((3_600_000_000_000, 24)),
        "minute" | "minutes" => Some((60_000_000_000, 60)),
        "second" | "seconds" => Some((1_000_000_000, 60)),
        "millisecond" | "milliseconds" => Some((1_000_000, 1_000)),
        "microsecond" | "microseconds" => Some((1_000, 1_000)),
        "nanosecond" | "nanoseconds" => Some((1, 1_000)),
        _ => None,
    }
}

/// `Temporal.PlainTime.prototype.round(roundTo)`：按最小单位、增量和模式在"当日"域内舍入。
///
/// # 步骤
/// 1. roundTo 解析：undefined → TypeError；字符串 → {smallestUnit: 串}；对象 → 按
///    roundingIncrement、roundingMode、smallestUnit 的固定读取顺序依次取值。
/// 2. branding receiver 取槽 0 ns。
/// 3. 单位表查 smallestUnit（hour..nanosecond；None → RangeError "invalid smallest unit"）。
/// 4. roundingIncrement 校验：1..=1e9 且 **真因子**（increment < max_increment 且
///    max_increment % increment == 0，否则 RangeError）。
/// 5. mode = instant_rounding_mode（None → RangeError）；缺省 "halfExpand"。
/// 6. round_instant_ns(time_ns, unit_ns*increment, mode)，结果 rem_euclid(DAY_NS) 保持 0-24 域。
///
/// # 边界与前提
/// - 舍入跨午夜（如 23:59:59.9 舍入到秒）→ 取模回 00:00，符合 rounding-cross-midnight 语义。
pub fn plain_time_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let round_to = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
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

    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_time(vm, obj));
    let time_ns = get_double_prop(obj, 0) as i128;

    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
    };
    let Some(unit_value) = unit_value else {
        return NativeResult::Err(crate::error::create_range_error(vm, "smallestUnit is required"));
    };
    let Some((unit_ns, max_increment)) = plain_time_round_unit(&unit_value) else {
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
    // 真因子校验：增量须严格小于最大增量且能整除（MaximumTemporalDurationRoundingIncrement
    // 语义；instant_round 只查 % 的缺陷不在此沿用）。
    if increment >= max_increment || max_increment % increment != 0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "rounding increment must divide a day"));
    }
    let Some(quantum_ns) = unit_ns.checked_mul(increment) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    };
    let Some(rounded_ns) = round_instant_ns(time_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time"));
    };
    const DAY_NS: i128 = 86_400_000_000_000;
    let time_ns = rounded_ns.rem_euclid(DAY_NS);
    make_plain_time(vm, time_ns as f64)
}
