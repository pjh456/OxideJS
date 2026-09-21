//! Temporal.PlainDateTime：ISO 日期时间解析、构造、getter、日历属性与加减/差值。

use chrono::{Datelike, NaiveDate};
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::common::native_try;
use super::{
    civil_from_days, days_from_civil, days_in_month, days_in_month_iso, difference_core, duration_component_integer,
    duration_like_values, ensure_plain_date_time, format_iso_year, get_calendar_id, get_double_prop,
    initialize_temporal_receiver, instant_rounding_mode, is_ctor_call, is_leap_year_iso, make_plain_date,
    make_plain_date_time, make_plain_time, parse_difference_settings, parse_fractional_second_digits,
    parse_temporal_string_impl, plain_date_time_object_parts, plain_time_components, receiver_obj, round_instant_ns,
    temporal_calendar_id_strict, temporal_option_number, temporal_option_string, temporal_option_value,
    temporal_overflow, valid_iso_date, valid_plain_date_time_range, valid_plain_time, zoned_date_time_plain_parts,
    FractionalSecondDigitsInput,
};
// ───────────────────── PlainDateTime 基础方法 ─────────────────────

/// `Temporal.PlainDateTime` 构造器：保存 ISO 日期与午夜后纳秒。
pub fn plain_date_time_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().plain_date_time_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.PlainDateTime cannot be invoked without 'new'",
        ));
    }

    let mut component = |index: usize, default: f64| -> Result<f64, JsValue> {
        if args.len() > index {
            let raw = vm.reg(args[index]);
            // 时间分量（缺省臂为有限值）：显式 undefined 与缺参同义，取缺省值；
            // 日期分量无缺省臂（缺省值为 NaN），undefined 仍经 NaN 统一抛
            // RangeError。
            if raw.is_undefined() && default.is_finite() {
                return Ok(default);
            }
            temporal_option_number(vm, raw).map(f64::trunc)
        } else {
            Ok(default)
        }
    };
    let year_value = native_try!(component(1, f64::NAN));
    let month_value = native_try!(component(2, f64::NAN));
    let day_value = native_try!(component(3, f64::NAN));
    let hour_value = native_try!(component(4, 0.0));
    let minute_value = native_try!(component(5, 0.0));
    let second_value = native_try!(component(6, 0.0));
    let millisecond_value = native_try!(component(7, 0.0));
    let microsecond_value = native_try!(component(8, 0.0));
    let nanosecond_value = native_try!(component(9, 0.0));
    let components = [
        year_value,
        month_value,
        day_value,
        hour_value,
        minute_value,
        second_value,
        millisecond_value,
        microsecond_value,
        nanosecond_value,
    ];
    if components.iter().any(|value| !value.is_finite()) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time component"));
    }

    // f64 截断后可能超出目标整型范围，先做有界转换。
    let year = if year_value >= i32::MIN as f64 && year_value <= i32::MAX as f64 {
        year_value as i32
    } else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid year"));
    };
    let mut to_u32 = |v: f64| -> Result<u32, JsValue> {
        if v >= 0.0 && v <= u32::MAX as f64 {
            Ok(v as u32)
        } else {
            Err(crate::error::create_range_error(vm, "invalid date-time component"))
        }
    };
    let month = native_try!(to_u32(month_value));
    let day = native_try!(to_u32(day_value));
    let hour = native_try!(to_u32(hour_value));
    let minute = native_try!(to_u32(minute_value));
    let second = native_try!(to_u32(second_value));
    let millisecond = native_try!(to_u32(millisecond_value));
    let microsecond = native_try!(to_u32(microsecond_value));
    let nanosecond = native_try!(to_u32(nanosecond_value));
    let non_negative = components[1..].iter().all(|value| *value >= 0.0);
    if !non_negative
        || !valid_iso_date(year, month, day)
        || !valid_plain_time(hour, minute, second, millisecond, microsecond, nanosecond)
    {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time component"));
    }

    let calendar = if args.len() > 10 && !vm.reg(args[10]).is_undefined() {
        native_try!(temporal_calendar_id_strict(vm, vm.reg(args[10]))).unwrap_or_else(|| "iso8601".to_string())
    } else {
        "iso8601".to_string()
    };

    let total_ns = hour as f64 * 3_600_000_000_000.0
        + minute as f64 * 60_000_000_000.0
        + second as f64 * 1_000_000_000.0
        + millisecond as f64 * 1_000_000.0
        + microsecond as f64 * 1_000.0
        + nanosecond as f64;
    if !valid_plain_date_time_range(year, month, day, total_ns) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time component"));
    }
    let calendar_value = vm.new_string(&calendar);
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_PLAIN_DATE_TIME,
        [
            JsValue::float(year as f64),
            JsValue::float(month as f64),
            JsValue::float(day as f64),
            JsValue::float(total_ns),
            calendar_value,
        ],
    )
}

pub(crate) fn parse_plain_date_time_string(input: &str) -> Result<(i32, u32, u32, f64), String> {
    parse_temporal_string_impl(input, true)
}

fn plain_date_time_like_parts<H: VmHost>(
    vm: &mut H, value: JsValue, constrain: bool,
) -> Result<(i32, u32, u32, f64, Option<String>), JsValue> {
    if value.is_string() {
        return parse_plain_date_time_string(&to_string(value))
            .map(|(year, month, day, total_ns)| (year, month, day, total_ns, None))
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 date-time"));
    }
    if !value.is_object() {
        return Err(crate::error::create_type_error(vm, "cannot convert to PlainDateTime"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "cannot convert to PlainDateTime"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_plain_date_time_obj() {
        return Ok((
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
            get_double_prop(obj, 3),
            Some(get_calendar_id(obj, 4)),
        ));
    }
    if obj.is_plain_date_obj() {
        return Ok((
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
            0.0,
            Some(get_calendar_id(obj, 3)),
        ));
    }
    if obj.is_zoned_date_time_obj() {
        return zoned_date_time_plain_parts(vm, obj)
            .map(|(year, month, day, total_ns)| (year, month, day, total_ns, Some(get_calendar_id(obj, 2))));
    }
    plain_date_time_object_parts(vm, value, obj, constrain, false)
}

/// `Temporal.PlainDateTime.from(item)`：从实例、ISO 字符串或字段对象创建副本。
pub fn plain_date_time_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if value.is_string() {
        // 规范顺序：先 ParseTemporalDateTimeString，再 ToTemporalOverflow(options)。
        let (year, month, day, total_ns) = match parse_plain_date_time_string(&to_string(value)) {
            Ok(parts) => parts,
            Err(_) => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO 8601 date-time"));
            }
        };
        native_try!(temporal_overflow(vm, args));
        make_plain_date_time(vm, year, month, day, total_ns, "iso8601")
    } else {
        let constrain = native_try!(temporal_overflow(vm, args));
        let (year, month, day, total_ns, calendar) = native_try!(plain_date_time_like_parts(vm, value, constrain));
        make_plain_date_time(vm, year, month, day, total_ns, calendar.as_deref().unwrap_or("iso8601"))
    }
}

/// `Temporal.PlainDateTime.compare(one, two)`：按 ISO 日期时间字段做字典序比较。
pub fn plain_date_time_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let one = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let two = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let one = native_try!(plain_date_time_like_parts(vm, one, true));
    let two = native_try!(plain_date_time_like_parts(vm, two, true));
    let ordering = (one.0, one.1, one.2, one.3 as u64).cmp(&(two.0, two.1, two.2, two.3 as u64));
    NativeResult::Ok(JsValue::int(match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }))
}

/// `Temporal.PlainDateTime.prototype.equals(other)`：比较日期时间分量是否相等。
pub fn plain_date_time_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (y, m, d, ns) = match plain_date_time_parts(vm, args) {
        Ok(parts) => parts,
        Err(e) => return NativeResult::Err(e),
    };
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let other = match plain_date_time_like_parts(vm, other_val, true) {
        Ok(parts) => parts,
        Err(e) => return NativeResult::Err(e),
    };
    let equal = other.0 == y && other.1 == m && other.2 == d && other.3 as u64 == ns as u64;
    NativeResult::Ok(JsValue::bool(equal))
}

fn plain_date_time_parts<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(i32, u32, u32, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_date_time(vm, obj)?;
    Ok((
        get_double_prop(obj, 0) as i32,
        get_double_prop(obj, 1) as u32,
        get_double_prop(obj, 2) as u32,
        get_double_prop(obj, 3),
    ))
}

macro_rules! plain_date_time_date_getter {
    ($name:ident, $index:tt) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let parts = native_try!(plain_date_time_parts(vm, args));
            NativeResult::Ok(JsValue::float(parts.$index as f64))
        }
    };
}

plain_date_time_date_getter!(plain_date_time_year, 0);
plain_date_time_date_getter!(plain_date_time_month, 1);
plain_date_time_date_getter!(plain_date_time_day, 2);

fn plain_date_time_naive<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<NaiveDate, JsValue> {
    let (year, month, day, _) = plain_date_time_parts(vm, args)?;
    NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| crate::error::create_range_error(vm, "invalid date"))
}

/// `Temporal.PlainDateTime.prototype.dayOfWeek` getter。
pub fn plain_date_time_day_of_week<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = native_try!(plain_date_time_naive(vm, args));
    NativeResult::Ok(JsValue::float((date.weekday().num_days_from_monday() + 1) as f64))
}

/// `Temporal.PlainDateTime.prototype.dayOfYear` getter。
pub fn plain_date_time_day_of_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = native_try!(plain_date_time_naive(vm, args));
    NativeResult::Ok(JsValue::float(date.ordinal() as f64))
}

/// `Temporal.PlainDateTime.prototype.daysInMonth` getter。
pub fn plain_date_time_days_in_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = native_try!(plain_date_time_naive(vm, args));
    NativeResult::Ok(JsValue::float(days_in_month_iso(date.year(), date.month()) as f64))
}

/// `Temporal.PlainDateTime.prototype.daysInWeek` getter。
pub fn plain_date_time_days_in_week<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_time_parts(vm, args));
    NativeResult::Ok(JsValue::float(7.0))
}

/// `Temporal.PlainDateTime.prototype.daysInYear` getter。
pub fn plain_date_time_days_in_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = native_try!(plain_date_time_naive(vm, args));
    NativeResult::Ok(JsValue::float(if is_leap_year_iso(date.year()) { 366.0 } else { 365.0 }))
}

/// `Temporal.PlainDateTime.prototype.monthsInYear` getter。
pub fn plain_date_time_months_in_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_time_parts(vm, args));
    NativeResult::Ok(JsValue::float(12.0))
}

/// `Temporal.PlainDateTime.prototype.inLeapYear` getter。
pub fn plain_date_time_in_leap_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = native_try!(plain_date_time_naive(vm, args));
    NativeResult::Ok(JsValue::bool(is_leap_year_iso(date.year())))
}

/// `Temporal.PlainDateTime.prototype.weekOfYear` getter。
pub fn plain_date_time_week_of_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = native_try!(plain_date_time_naive(vm, args));
    NativeResult::Ok(JsValue::float(date.iso_week().week() as f64))
}

/// `Temporal.PlainDateTime.prototype.yearOfWeek` getter。
pub fn plain_date_time_year_of_week<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = native_try!(plain_date_time_naive(vm, args));
    NativeResult::Ok(JsValue::float(date.iso_week().year() as f64))
}

/// `Temporal.PlainDateTime.prototype.monthCode` getter。
pub fn plain_date_time_month_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, month, _, _) = native_try!(plain_date_time_parts(vm, args));
    NativeResult::Ok(vm.new_string(&format!("M{month:02}")))
}

/// ISO 日历没有可观察的 era 字段。
pub fn plain_date_time_era<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_time_parts(vm, args));
    NativeResult::Ok(JsValue::undefined())
}

/// ISO 日历没有可观察的 eraYear 字段。
pub fn plain_date_time_era_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_time_parts(vm, args));
    NativeResult::Ok(JsValue::undefined())
}

fn plain_date_time_time_get<H: VmHost>(
    vm: &mut H, args: &[u8], select: fn(u32, u32, u32, u32, u32, u32) -> u32,
) -> NativeResult {
    let (_, _, _, total_ns) = native_try!(plain_date_time_parts(vm, args));
    let (hour, minute, second, millisecond, microsecond, nanosecond) = plain_time_components(total_ns);
    NativeResult::Ok(JsValue::float(select(hour, minute, second, millisecond, microsecond, nanosecond) as f64))
}

/// `Temporal.PlainDateTime.prototype.hour` getter。
pub fn plain_date_time_hour<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_time_get(vm, args, |hour, _, _, _, _, _| hour)
}

/// `Temporal.PlainDateTime.prototype.minute` getter。
pub fn plain_date_time_minute<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_time_get(vm, args, |_, minute, _, _, _, _| minute)
}

/// `Temporal.PlainDateTime.prototype.second` getter。
pub fn plain_date_time_second<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_time_get(vm, args, |_, _, second, _, _, _| second)
}

/// `Temporal.PlainDateTime.prototype.millisecond` getter。
pub fn plain_date_time_millisecond<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_time_get(vm, args, |_, _, _, millisecond, _, _| millisecond)
}

/// `Temporal.PlainDateTime.prototype.microsecond` getter。
pub fn plain_date_time_microsecond<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_time_get(vm, args, |_, _, _, _, microsecond, _| microsecond)
}

/// `Temporal.PlainDateTime.prototype.nanosecond` getter。
pub fn plain_date_time_nanosecond<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_time_get(vm, args, |_, _, _, _, _, nanosecond| nanosecond)
}

/// `Temporal.PlainDateTime.prototype.calendarId`：读日历槽，兜底 ISO 8601。
pub fn plain_date_time_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date_time(vm, obj));
    NativeResult::Ok(vm.new_string(&get_calendar_id(obj, 4)))
}

/// 返回仅保留日期分量的新 `Temporal.PlainDate`，日历从 receiver 槽继承。
pub fn plain_date_time_to_plain_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    let (year, month, day, _) = native_try!(plain_date_time_parts(vm, args));
    make_plain_date(vm, year, month, day, &get_calendar_id(obj, 4))
}

/// 返回仅保留时间分量的新 `Temporal.PlainTime`。
pub fn plain_date_time_to_plain_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, _, _, total_ns) = native_try!(plain_date_time_parts(vm, args));
    make_plain_time(vm, total_ns)
}

/// Temporal 对象禁止隐式转换为原始值。
pub fn plain_date_time_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.PlainDateTime has no valueOf"))
}

/// 格式化 PlainDateTime 本地 ISO 日期时间（无偏移），可裁秒与小数位并追加日历注解。
fn format_plain_date_time_iso(
    year: i32, month: u32, day: u32, time_ns: i128, include_seconds: bool, fractional_digits: Option<usize>,
    calendar_name: &str,
) -> String {
    let hour = time_ns / 3_600_000_000_000;
    let minute = time_ns / 60_000_000_000 % 60;
    let second = time_ns / 1_000_000_000 % 60;
    let subsecond = time_ns % 1_000_000_000;
    let mut output = format!("{}-{month:02}-{day:02}T{hour:02}:{minute:02}", format_iso_year(i128::from(year)));
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
    match calendar_name {
        "always" => output.push_str("[u-ca=iso8601]"),
        "critical" => output.push_str("[!u-ca=iso8601]"),
        _ => {}
    }
    output
}

fn plain_date_time_iso_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, month, day, total_ns) = native_try!(plain_date_time_parts(vm, args));
    let output = format_plain_date_time_iso(year, month, day, total_ns as i128, true, None, "never");
    NativeResult::Ok(vm.new_string_owned(output))
}

/// `Temporal.PlainDateTime.prototype.toString(options)`：按精度、舍入模式与日历显示输出 ISO 8601。
pub fn plain_date_time_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, month, day, total_ns) = native_try!(plain_date_time_parts(vm, args));
    let options_value = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (calendar_name, fractional_input, mode_value, smallest_value) = if options_value.is_undefined() {
        ("auto".to_string(), FractionalSecondDigitsInput::Auto, "trunc".to_string(), None)
    } else {
        if !options_value.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let calendar_raw = native_try!(temporal_option_value(vm, options, options_value, "calendarName"));
        let calendar_name = if calendar_raw.is_undefined() {
            "auto".to_string()
        } else {
            native_try!(temporal_option_string(vm, calendar_raw))
        };
        let fractional_raw = native_try!(temporal_option_value(vm, options, options_value, "fractionalSecondDigits"));
        let fractional_input = if fractional_raw.is_undefined() {
            FractionalSecondDigitsInput::Auto
        } else if fractional_raw.is_int() || fractional_raw.is_double() {
            FractionalSecondDigitsInput::Number(to_number(fractional_raw))
        } else {
            FractionalSecondDigitsInput::String(native_try!(temporal_option_string(vm, fractional_raw)))
        };
        let mode_raw = native_try!(temporal_option_value(vm, options, options_value, "roundingMode"));
        let mode_value = if mode_raw.is_undefined() {
            "trunc".to_string()
        } else {
            native_try!(temporal_option_string(vm, mode_raw))
        };
        let smallest_raw = native_try!(temporal_option_value(vm, options, options_value, "smallestUnit"));
        let smallest_value = if smallest_raw.is_undefined() {
            None
        } else {
            Some(native_try!(temporal_option_string(vm, smallest_raw)))
        };
        (calendar_name, fractional_input, mode_value, smallest_value)
    };

    match calendar_name.as_str() {
        "auto" | "always" | "never" | "critical" => {}
        _ => return NativeResult::Err(crate::error::create_range_error(vm, "invalid calendarName")),
    }
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
    const DAY_NS: i128 = 86_400_000_000_000;
    let Some(rounded_ns) = round_instant_ns(total_ns as i128, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time"));
    };
    let extra_days = rounded_ns / DAY_NS;
    let time_ns = rounded_ns % DAY_NS;
    let total_days = days_from_civil(i128::from(year), i128::from(month), i128::from(day)) + extra_days;
    let (year, month, day) = civil_from_days(total_days);
    if !valid_plain_date_time_range(year as i32, month as u32, day as u32, time_ns as f64) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time"));
    }
    let output = format_plain_date_time_iso(
        year as i32,
        month as u32,
        day as u32,
        time_ns,
        include_seconds,
        output_digits,
        &calendar_name,
    );
    NativeResult::Ok(vm.new_string_owned(output))
}

/// 按 duration-like 分量对 PlainDateTime 做加减：先平衡时间（溢出为天），
/// 再按年/月/周/日推进日期，最后校验范围。
fn plain_date_time_apply_duration<H: VmHost>(vm: &mut H, args: &[u8], sign: i64) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    let (year, month, day, time_ns) = match plain_date_time_parts(vm, args) {
        Ok(parts) => parts,
        Err(error) => return NativeResult::Err(error),
    };
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = match duration_like_values(vm, val) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
    let constrain = match temporal_overflow(vm, args) {
        Ok(constrain) => constrain,
        Err(error) => return NativeResult::Err(error),
    };
    const DAY_NS: i128 = 86_400_000_000_000;
    // 时间字段（hours 起）单独换算纳秒；days 由日期部分处理，避免重复计入。
    let [_, _, _, _, h, min, s, ms, us, ns] = values;
    let time_delta = duration_component_integer(h).unwrap_or(0) * 3_600_000_000_000
        + duration_component_integer(min).unwrap_or(0) * 60_000_000_000
        + duration_component_integer(s).unwrap_or(0) * 1_000_000_000
        + duration_component_integer(ms).unwrap_or(0) * 1_000_000
        + duration_component_integer(us).unwrap_or(0) * 1_000
        + duration_component_integer(ns).unwrap_or(0);
    let total_ns = time_ns as i128 + time_delta * sign as i128;
    let extra_days = total_ns.div_euclid(DAY_NS);
    let new_time_ns = total_ns.rem_euclid(DAY_NS);

    let [y, m, w, d, ..] = values;
    let months =
        (duration_component_integer(y).unwrap_or(0) * 12 + duration_component_integer(m).unwrap_or(0)) * sign as i128;
    let total_month = i128::from(year) * 12 + i128::from(month) - 1 + months;
    let ny = total_month.div_euclid(12);
    let nm = total_month.rem_euclid(12) + 1;
    let Some(max_day) = days_in_month(ny, nm) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date"));
    };
    let day = i128::from(day);
    let new_day = if day > max_day {
        if !constrain {
            return NativeResult::Err(crate::error::create_range_error(vm, "day out of range"));
        }
        max_day
    } else {
        day
    };
    let day_delta = duration_component_integer(w).unwrap_or(0) * 7 + duration_component_integer(d).unwrap_or(0);
    let total_days = days_from_civil(ny, nm, new_day) + extra_days + day_delta * sign as i128;
    let (yy, mm, dd) = civil_from_days(total_days);
    if !valid_plain_date_time_range(yy as i32, mm as u32, dd as u32, new_time_ns as f64) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time"));
    }
    make_plain_date_time(vm, yy as i32, mm as u32, dd as u32, new_time_ns as f64, &get_calendar_id(obj, 4))
}

/// 计算差值并舍入。until 返回 other 减 receiver，since 返回反向。
fn plain_date_time_difference<H: VmHost>(vm: &mut H, args: &[u8], since: bool) -> NativeResult {
    let (sy, sm, sd, st) = match plain_date_time_parts(vm, args) {
        Ok(parts) => parts,
        Err(error) => return NativeResult::Err(error),
    };
    let other = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (oy, om, od, ot, _) = match plain_date_time_like_parts(vm, other, true) {
        Ok(parts) => parts,
        Err(error) => return NativeResult::Err(error),
    };
    let options_value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let settings = match parse_difference_settings(vm, options_value, false, 3) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    difference_core(
        vm,
        (i128::from(sy), i128::from(sm), i128::from(sd)),
        st as i128,
        (i128::from(oy), i128::from(om), i128::from(od)),
        ot as i128,
        settings,
        since,
        9,
    )
}

/// `Temporal.PlainDateTime.prototype.until(other, options)`。
pub fn plain_date_time_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_difference(vm, args, false)
}

/// `Temporal.PlainDateTime.prototype.since(other, options)`。
pub fn plain_date_time_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_difference(vm, args, true)
}

/// `Temporal.PlainDateTime.prototype.add(durationLike, options)`。
pub fn plain_date_time_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_apply_duration(vm, args, 1)
}

/// `Temporal.PlainDateTime.prototype.subtract(durationLike, options)`。
pub fn plain_date_time_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_apply_duration(vm, args, -1)
}

/// `Temporal.PlainDateTime.prototype.toJSON()`：输出默认 ISO 日期时间。
pub fn plain_date_time_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_date_time_iso_string(vm, args)
}
