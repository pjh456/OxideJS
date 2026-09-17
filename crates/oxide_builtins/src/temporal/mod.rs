use chrono::{Datelike, NaiveDate};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

mod common;
mod difference;
mod duration;
mod instant;
mod plain_date;
mod plain_time;
mod zoned_date_time;

use common::*;
use difference::*;
pub use duration::*;
pub use instant::*;
pub use plain_date::*;
pub use plain_time::*;
pub use zoned_date_time::*;

fn make_plain_date<H: VmHost>(vm: &mut H, year: i32, month: u32, day: u32, calendar: &str) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_date_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_DATE;
    obj.set_prop_at(0, JsValue::float(year as f64));
    obj.set_prop_at(1, JsValue::float(month as f64));
    obj.set_prop_at(2, JsValue::float(day as f64));
    obj.set_prop_at(3, vm.new_string(calendar));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn make_plain_time<H: VmHost>(vm: &mut H, total_ns: f64) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_time_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_TIME;
    obj.set_prop_at(0, JsValue::float(total_ns));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn make_plain_date_time<H: VmHost>(
    vm: &mut H, year: i32, month: u32, day: u32, total_ns: f64, calendar: &str,
) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_date_time_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_DATE_TIME;
    obj.set_prop_at(0, JsValue::float(year as f64));
    obj.set_prop_at(1, JsValue::float(month as f64));
    obj.set_prop_at(2, JsValue::float(day as f64));
    obj.set_prop_at(3, JsValue::float(total_ns));
    obj.set_prop_at(4, vm.new_string(calendar));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn make_duration<H: VmHost>(vm: &mut H, values: [f64; 10]) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().duration_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_DURATION;
    for (index, value) in values.into_iter().enumerate() {
        obj.set_prop_at(index, JsValue::float(value));
    }
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

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

    let year = year_value as i32;
    let month = month_value as u32;
    let day = day_value as u32;
    let hour = hour_value as u32;
    let minute = minute_value as u32;
    let second = second_value as u32;
    let millisecond = millisecond_value as u32;
    let microsecond = microsecond_value as u32;
    let nanosecond = nanosecond_value as u32;
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

fn parse_plain_date_time_string(input: &str) -> Result<(i32, u32, u32, f64), String> {
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

// Temporal.PlainMonthDay / Temporal.PlainYearMonth 对象地基：
// 数字分量转换、槽对象构造、构造器与 getter/toString/toJSON。

/// 构造 PlainMonthDay 实例对象（槽 0-3 = 月/日/参考年/日历 ID）。
/// from/toPlainDate 等返回新对象的成员使用；构造器走 receiver 初始化路径。
fn make_plain_month_day<H: VmHost>(vm: &mut H, month: u32, day: u32, ref_year: i32, calendar: &str) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_month_day_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_MONTH_DAY;
    obj.set_prop_at(0, JsValue::float(month as f64));
    obj.set_prop_at(1, JsValue::float(day as f64));
    obj.set_prop_at(2, JsValue::float(ref_year as f64));
    obj.set_prop_at(3, vm.new_string(calendar));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

/// 构造 PlainYearMonth 实例对象（槽 0-3 = 年/月/参考日/日历 ID）。
/// from 等返回新对象的成员使用；构造器走 receiver 初始化路径。
fn make_plain_year_month<H: VmHost>(vm: &mut H, year: i32, month: u32, ref_day: u32, calendar: &str) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_year_month_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_YEAR_MONTH;
    obj.set_prop_at(0, JsValue::float(year as f64));
    obj.set_prop_at(1, JsValue::float(month as f64));
    obj.set_prop_at(2, JsValue::float(ref_day as f64));
    obj.set_prop_at(3, vm.new_string(calendar));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn ensure_plain_month_day<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_month_day_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

fn ensure_plain_year_month<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_year_month_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

/// 读 PlainMonthDay 的 月/日/参考年 三元组（槽 0-2），含 receiver 校验。
fn plain_month_day_mdy<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(f64, f64, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_month_day(vm, obj)?;
    Ok((get_double_prop(obj, 0), get_double_prop(obj, 1), get_double_prop(obj, 2)))
}

/// 读 PlainYearMonth 的 年/月/参考日 三元组（槽 0-2），含 receiver 校验。
fn plain_year_month_ymd<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(f64, f64, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_year_month(vm, obj)?;
    Ok((get_double_prop(obj, 0), get_double_prop(obj, 1), get_double_prop(obj, 2)))
}

/// 构造器日历参数解析（ToTemporalCalendarSlotValue，缺省 "iso8601"）：
/// undefined → 缺省；字符串 → 严格白名单（非法 RangeError）；Temporal 日期实例 →
/// 日历槽直读（不触发属性 getter）；函数对象 → 缺省（无日历行为）；
/// 其他对象与原始值 → TypeError。
fn temporal_constructor_calendar_id<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
    if value.is_undefined() {
        return Ok("iso8601".to_string());
    }
    if value.is_string() {
        return temporal_calendar_id_strict(vm, value)
            .map(|calendar| calendar.unwrap_or_else(|| "iso8601".to_string()));
    }
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_function() {
                return Ok("iso8601".to_string());
            }
            let slot = if obj.is_plain_date_obj() {
                3
            } else if obj.is_plain_date_time_obj() {
                4
            } else if obj.is_zoned_date_time_obj() {
                2
            } else if obj.is_plain_month_day_obj() || obj.is_plain_year_month_obj() {
                3
            } else {
                return Err(crate::error::create_type_error(vm, "invalid calendar"));
            };
            return Ok(get_calendar_id(obj, slot));
        }
    }
    Err(crate::error::create_type_error(vm, "invalid calendar"))
}

/// `Temporal.PlainMonthDay` 构造器：`new PlainMonthDay(month, day[, calendar[, referenceISOYear]])`。
pub fn plain_month_day_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().plain_month_day_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.PlainMonthDay cannot be invoked without 'new'",
        ));
    }
    // 转换序：month → day → calendar → referenceISOYear（后两者有缺省值）。
    let month = native_try!(temporal_number_component(
        vm,
        if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() },
    ));
    let day = native_try!(temporal_number_component(
        vm,
        if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() },
    ));
    let calendar_raw = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };
    let calendar = native_try!(temporal_constructor_calendar_id(vm, calendar_raw));
    let ref_year = if args.len() > 4 && !vm.reg(args[4]).is_undefined() {
        native_try!(temporal_number_component(vm, vm.reg(args[4]))) as i32
    } else {
        1972
    };
    if !valid_iso_date(ref_year, month as u32, day as u32) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    // 表示范围（ISODateWithinLimits）：-271821-04-19 … +275760-09-13。
    let day_count = days_from_civil(i128::from(ref_year), i128::from(month as i32), i128::from(day as i32));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    let calendar_value = vm.new_string(&calendar);
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_PLAIN_MONTH_DAY,
        [
            JsValue::float(month),
            JsValue::float(day),
            JsValue::float(ref_year as f64),
            calendar_value,
        ],
    )
}

/// `Temporal.PlainYearMonth` 构造器：`new PlainYearMonth(year, month[, calendar[, referenceISODay]])`。
pub fn plain_year_month_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().plain_year_month_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.PlainYearMonth cannot be invoked without 'new'",
        ));
    }
    // 转换序：year → month → calendar → referenceISODay（后两者有缺省值）。
    let year = native_try!(temporal_number_component(
        vm,
        if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() },
    )) as i32;
    let month = native_try!(temporal_number_component(
        vm,
        if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() },
    ));
    let calendar_raw = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };
    let calendar = native_try!(temporal_constructor_calendar_id(vm, calendar_raw));
    let ref_day = if args.len() > 4 && !vm.reg(args[4]).is_undefined() {
        native_try!(temporal_number_component(vm, vm.reg(args[4])))
    } else {
        1.0
    };
    let month_i = month as i32;
    if !(1..=12).contains(&month_i) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    let ref_day_i = ref_day as i32;
    if ref_day_i < 1 || ref_day_i > days_in_month_iso(year, month_i as u32) as i32 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    if !iso_year_month_within_limits(year, month_i as u32) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    let calendar_value = vm.new_string(&calendar);
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_PLAIN_YEAR_MONTH,
        [
            JsValue::float(year as f64),
            JsValue::float(month),
            JsValue::float(ref_day),
            calendar_value,
        ],
    )
}

/// `Temporal.PlainMonthDay.prototype.day` getter（槽 1）。
pub fn plain_month_day_day<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, day, _) = native_try!(plain_month_day_mdy(vm, args));
    NativeResult::Ok(JsValue::float(day))
}

/// `Temporal.PlainMonthDay.prototype.monthCode` getter：`M01`..`M12`。
pub fn plain_month_day_month_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (month, _, _) = native_try!(plain_month_day_mdy(vm, args));
    NativeResult::Ok(vm.new_string(&format!("M{:02}", month as u32)))
}

/// `Temporal.PlainMonthDay.prototype.calendarId` getter：读日历槽（槽 3）。
pub fn plain_month_day_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    NativeResult::Ok(vm.new_string(&get_calendar_id(obj, 3)))
}

/// `Temporal.PlainYearMonth.prototype.year` getter（槽 0）。
pub fn plain_year_month_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, _, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(year))
}

/// `Temporal.PlainYearMonth.prototype.month` getter（槽 1）。
pub fn plain_year_month_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, month, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(month))
}

/// `Temporal.PlainYearMonth.prototype.monthCode` getter：`M01`..`M12`。
pub fn plain_year_month_month_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, month, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(vm.new_string(&format!("M{:02}", month as u32)))
}

/// `Temporal.PlainYearMonth.prototype.calendarId` getter：读日历槽（槽 3）。
pub fn plain_year_month_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    NativeResult::Ok(vm.new_string(&get_calendar_id(obj, 3)))
}

/// `Temporal.PlainYearMonth.prototype.daysInMonth` getter：按年月的 ISO 月长。
pub fn plain_year_month_days_in_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, month, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(days_in_month_iso(year as i32, month as u32) as f64))
}

/// `Temporal.PlainYearMonth.prototype.daysInYear` getter：366（闰年）或 365。
pub fn plain_year_month_days_in_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, _, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(if is_leap_year_iso(year as i32) { 366.0 } else { 365.0 }))
}

/// `Temporal.PlainYearMonth.prototype.monthsInYear` getter：恒 12（ISO 日历）。
pub fn plain_year_month_months_in_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(12.0))
}

/// `Temporal.PlainYearMonth.prototype.inLeapYear` getter。
pub fn plain_year_month_in_leap_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, _, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::bool(is_leap_year_iso(year as i32)))
}

/// `Temporal.PlainYearMonth.prototype.era` getter：ISO 日历无纪元，恒 undefined。
pub fn plain_year_month_era<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::undefined())
}

/// `Temporal.PlainYearMonth.prototype.eraYear` getter：ISO 日历无纪元，恒 undefined。
pub fn plain_year_month_era_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::undefined())
}

/// PlainMonthDay ISO 串：always/critical 或非 iso8601 日历补参考年与日历注解；
/// iso8601 日历的 auto/never 形保持裸 `MM-DD`。
fn plain_month_day_iso_string(obj: &JsObject, show: ShowCalendar) -> String {
    let month = get_double_prop(obj, 0) as i32;
    let day = get_double_prop(obj, 1) as i32;
    let calendar = get_calendar_id(obj, 3);
    let with_year = matches!(show, ShowCalendar::Always | ShowCalendar::Critical) || calendar != "iso8601";
    if !with_year {
        return format!("{month:02}-{day:02}");
    }
    let ref_year = get_double_prop(obj, 2) as i32;
    let annotation = calendar_annotation(&calendar, show);
    format!("{}-{month:02}-{day:02}{annotation}", format_iso_year(ref_year as i128))
}

/// PlainMonthDay 默认形串（auto 形），含 receiver 校验，不读 options。
fn plain_month_day_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    NativeResult::Ok(vm.new_string_owned(plain_month_day_iso_string(obj, ShowCalendar::Auto)))
}

/// `Temporal.PlainMonthDay.prototype.toString([options])`：
/// 默认 `MM-DD`；非 iso8601 日历补参考年与 `[u-ca=…]` 注解，
/// always/critical 另按注解表补 `[u-ca=…]`/`[!u-ca=…]`。
pub fn plain_month_day_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let show = native_try!(temporal_to_show_calendar(vm, args));
    NativeResult::Ok(vm.new_string_owned(plain_month_day_iso_string(obj, show)))
}

/// `Temporal.PlainMonthDay.prototype.toJSON()`：默认形串，忽略参数。
pub fn plain_month_day_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_month_day_default_string(vm, args)
}

/// `Temporal.PlainMonthDay.prototype.toLocaleString()`：locale/options 无引擎行为，
/// 恒输出默认形串。
pub fn plain_month_day_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_month_day_default_string(vm, args)
}

/// `Temporal.PlainMonthDay.prototype.valueOf()`：Temporal 对象无值表示，恒 TypeError。
pub fn plain_month_day_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.PlainMonthDay has no valueOf"))
}

/// PlainYearMonth ISO 串：always/critical 或非 iso8601 日历补参考日与日历注解；
/// iso8601 日历的 auto/never 形保持裸 `±YYYY-MM`。
fn plain_year_month_iso_string(obj: &JsObject, show: ShowCalendar) -> String {
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as i32;
    let calendar = get_calendar_id(obj, 3);
    let with_day = matches!(show, ShowCalendar::Always | ShowCalendar::Critical) || calendar != "iso8601";
    if !with_day {
        return format!("{}-{month:02}", format_iso_year(year as i128));
    }
    let ref_day = get_double_prop(obj, 2) as i32;
    let annotation = calendar_annotation(&calendar, show);
    format!("{}-{month:02}-{ref_day:02}{annotation}", format_iso_year(year as i128))
}

/// PlainYearMonth 默认形串（auto 形），含 receiver 校验，不读 options。
fn plain_year_month_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    NativeResult::Ok(vm.new_string_owned(plain_year_month_iso_string(obj, ShowCalendar::Auto)))
}

/// `Temporal.PlainYearMonth.prototype.toString([options])`：
/// 默认 `±YYYY-MM`；非 iso8601 日历补参考日与 `[u-ca=…]` 注解，
/// always/critical 另按注解表补 `[u-ca=…]`/`[!u-ca=…]`。
pub fn plain_year_month_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let show = native_try!(temporal_to_show_calendar(vm, args));
    NativeResult::Ok(vm.new_string_owned(plain_year_month_iso_string(obj, show)))
}

/// `Temporal.PlainYearMonth.prototype.toJSON()`：默认形串，忽略参数。
pub fn plain_year_month_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_default_string(vm, args)
}

/// `Temporal.PlainYearMonth.prototype.toLocaleString()`：locale/options 无引擎行为，
/// 恒输出默认形串。
pub fn plain_year_month_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_default_string(vm, args)
}

/// `Temporal.PlainYearMonth.prototype.valueOf()`：Temporal 对象无值表示，恒 TypeError。
pub fn plain_year_month_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.PlainYearMonth has no valueOf"))
}

// ==================== PlainMonthDay.from ====================

/// PlainMonthDay 串注解校验：与 validate_temporal_annotation_suffix 同构，
/// 但 u-ca 值只接受 iso8601（大小写不敏感），其余内置日历 ID 拒绝。
fn validate_month_day_annotation_suffix(mut suffix: &str) -> Result<(), String> {
    let mut calendar_seen = false;
    let mut saw_critical_calendar = false;
    let mut time_zone_seen = false;
    while !suffix.is_empty() {
        if !suffix.starts_with('[') {
            return Err("invalid annotation suffix".into());
        }
        let Some(end) = suffix.find(']') else {
            return Err("unterminated annotation".into());
        };
        if end == 1 {
            return Err("empty annotation".into());
        }
        let annotation = &suffix[1..end];
        let (critical, body) = annotation.strip_prefix('!').map_or((false, annotation), |body| (true, body));
        if let Some((key, value)) = body.split_once('=') {
            if key.bytes().any(|byte| byte.is_ascii_uppercase()) {
                return Err("annotation keys must be lowercase".into());
            }
            if key == "u-ca" {
                if calendar_seen {
                    if critical || saw_critical_calendar {
                        return Err("invalid calendar annotation".into());
                    }
                } else if !value.eq_ignore_ascii_case("iso8601") {
                    return Err("invalid calendar annotation".into());
                } else {
                    calendar_seen = true;
                    saw_critical_calendar = critical;
                }
            } else if critical {
                return Err("unknown critical annotation".into());
            }
        } else if time_zone_seen {
            return Err("invalid time-zone annotation".into());
        } else {
            time_zone_seen = true;
        }
        suffix = &suffix[end + 1..];
    }
    Ok(())
}

/// PlainMonthDay 串的时间部分校验（值丢弃）：偏移剥离须恰好耗尽剩余文本；
/// 接受 T/t/空格分隔、小时可缺分钟秒、秒可 60（leap second）、小数秒 ≤9 位；
/// 禁止小数时/分、UTC 设计符在调用侧先行拒绝。
fn validate_month_day_time_suffix(input: &str) -> Result<(), String> {
    let time_end = input
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index))
        .unwrap_or(input.len());
    if time_end < input.len() && !strip_iso_offset(&input[time_end..])?.is_empty() {
        return Err("invalid ISO offset".into());
    }
    let time = &input[..time_end];
    if time.is_empty() {
        return Err("missing ISO time".into());
    }
    let (clock, fraction) = match time.find(['.', ',']) {
        Some(index) => (&time[..index], Some(&time[index + 1..])),
        None => (time, None),
    };
    let fields = if clock.contains(':') {
        clock.split(':').collect::<Vec<_>>()
    } else {
        if clock.len() % 2 != 0 || clock.len() > 6 {
            return Err("invalid ISO time".into());
        }
        clock
            .as_bytes()
            .chunks(2)
            .map(std::str::from_utf8)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "invalid ISO time".to_string())?
    };
    if fields.is_empty() || fields.len() > 3 || fields.iter().any(|field| field.len() != 2) {
        return Err("invalid ISO time".into());
    }
    if fraction.is_some() && fields.len() < 3 {
        return Err("fractional hours and minutes are not valid".into());
    }
    let parse_field = |index: usize| -> Result<u32, String> {
        fields
            .get(index)
            .map_or(Ok(0), |field| field.parse().map_err(|_| "invalid ISO time".to_string()))
    };
    let hour = parse_field(0)?;
    let minute = parse_field(1)?;
    let mut second = parse_field(2)?;
    if second == 60 {
        second = 59;
    }
    let subsecond = match fraction {
        Some(digits) if !digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|byte| byte.is_ascii_digit()) => {
            let mut padded = digits.to_owned();
            padded.extend(std::iter::repeat('0').take(9 - digits.len()));
            padded.parse::<u32>().map_err(|_| "invalid ISO fraction".to_string())?
        }
        Some(_) => return Err("invalid ISO fraction".into()),
        None => 0,
    };
    if !valid_plain_time(hour, minute, second, subsecond / 1_000_000, subsecond / 1_000 % 1_000, subsecond % 1_000) {
        return Err("invalid ISO time".into());
    }
    Ok(())
}

/// "MM-DD" 或 "MMDD" 形态的月日解析。
fn parse_month_day_md(input: &str) -> Result<(u32, u32), String> {
    let bytes = input.as_bytes();
    let (m_s, d_s) = if bytes.len() == 4 && bytes.iter().all(|b| b.is_ascii_digit()) {
        (&input[..2], &input[2..])
    } else if bytes.len() == 5
        && bytes[2] == b'-'
        && bytes[..2].iter().all(|b| b.is_ascii_digit())
        && bytes[3..].iter().all(|b| b.is_ascii_digit())
    {
        (&input[..2], &input[3..])
    } else {
        return Err("invalid ISO month-day".into());
    };
    let month: u32 = m_s.parse().map_err(|_| "invalid ISO month".to_string())?;
    let day: u32 = d_s.parse().map_err(|_| "invalid ISO day".to_string())?;
    Ok((month, day))
}

/// PlainMonthDay 串的日期部分：MM-DD / MMDD / --MM-DD / --MMDD / 完整日期（扩展或紧凑）。
/// 年份仅用于负零年拒绝（-000000 → 错），无范围检查；返回 (month, day)。
fn parse_month_day_date_part(input: &str) -> Result<(u32, u32), String> {
    if input.is_empty() {
        return Err("invalid ISO date".into());
    }
    if let Some(rest) = input.strip_prefix("--") {
        let (month, day) = parse_month_day_md(rest)?;
        return finish_month_day_range(month, day);
    }
    let bytes = input.as_bytes();
    let signed = matches!(bytes[0], b'-' | b'+');
    let negative = bytes[0] == b'-';
    let mut i = usize::from(signed);
    let y_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let digits = &input[y_start..i];
    if i < bytes.len() && bytes[i] == b'-' {
        // 扩展形：年份 + -MM-DD；无符号两位引导是 MM-DD 形态。
        if digits.len() == 2 && !signed {
            let (month, day) = parse_month_day_md(input)?;
            return finish_month_day_range(month, day);
        }
        let expected = usize::from(signed) * 6 + 4 * (1 - usize::from(signed));
        if digits.len() != expected {
            return Err("invalid ISO year".into());
        }
        i += 1;
        let month = read_iso2(input, &mut i, bytes)?;
        if i >= bytes.len() || bytes[i] != b'-' {
            return Err("invalid ISO date".into());
        }
        i += 1;
        let day = read_iso2(input, &mut i, bytes)?;
        if i != bytes.len() {
            return Err("invalid trailing content".into());
        }
        if negative && digits == "000000" {
            return Err("invalid ISO negative zero year".into());
        }
        return finish_month_day_range(month, day);
    }
    if i != bytes.len() {
        return Err("invalid trailing content".into());
    }
    // 紧凑纯数字：4 位 = MMDD（无符号）；否则 (4-6 位年) + 4 位月日。
    let len = digits.len();
    if !(len == 4 && !signed || len == 10 && signed || (!signed && (8..=10).contains(&len))) {
        return Err("invalid ISO date".into());
    }
    let tail = &input[y_start..];
    let y_len = tail.len() - 4;
    let (month, day) = parse_month_day_md(&tail[y_len..])?;
    if y_len == 6 && negative && &tail[..6] == "000000" {
        return Err("invalid ISO negative zero year".into());
    }
    finish_month_day_range(month, day)
}

/// 月日取值边界：月 1..=12、日 1..=31（串路径无参考年，不做月长检查）。
fn finish_month_day_range(month: u32, day: u32) -> Result<(u32, u32), String> {
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return Err("invalid ISO date".into());
    }
    Ok((month, day))
}

/// 解析 PlainMonthDay ISO 串（"MM-DD"/"MMDD"/"--MM-DD"/完整日期/完整 datetime）。
/// 注解与时间部分按规则校验；参考年恒 1972（串中年份丢弃）。
fn parse_month_day_string(input: &str) -> Result<(u32, u32), String> {
    let trimmed = input.trim();
    if trimmed.contains('\u{2212}') {
        return Err("variant minus sign is not valid for PlainMonthDay".into());
    }
    let text = trimmed.to_owned();
    let annotation_start = text.find('[').unwrap_or(text.len());
    validate_month_day_annotation_suffix(&text[annotation_start..])?;
    let text = &text[..annotation_start];
    if text.contains('Z') || text.contains('z') {
        return Err("UTC designator is not valid for PlainMonthDay".into());
    }
    let separator = text.find(['T', 't', ' ']);
    let (date_part, time_part) = match separator {
        Some(index) => (&text[..index], &text[index + 1..]),
        None => (text, ""),
    };
    if !time_part.is_empty() {
        validate_month_day_time_suffix(time_part)?;
    }
    parse_month_day_date_part(date_part)
}

/// 年月串边界：月 1..=12；日（若有）1..=31 且过语法级月日静态检查
/// （2 月 30/31、30 天月份 31 拒；无参考年，2 月 29 不拒）。
fn finish_year_month_range(year: i32, month: u32, day: Option<u32>) -> Result<(i32, u32, Option<u32>), String> {
    if !(1..=12).contains(&month) {
        return Err("invalid ISO month".into());
    }
    if let Some(day) = day {
        if !(1..=31).contains(&day) || (month == 2 && day > 29) || (matches!(month, 4 | 6 | 9 | 11) && day == 31) {
            return Err("invalid ISO date".into());
        }
    }
    Ok((year, month, day))
}

/// PlainYearMonth 串的日期部分：扩展形（无符号 4 位年或带符号 6 位年）+ -MM[-DD]，
/// 紧凑形（无符号 6/8 位、带符号 8/10 位纯数字）；日部分可缺（年-月形）；
/// 负零年（-000000）拒；返回 (year, month, day)。
fn parse_year_month_date_part(input: &str) -> Result<(i32, u32, Option<u32>), String> {
    if input.is_empty() {
        return Err("invalid ISO date".into());
    }
    let bytes = input.as_bytes();
    let signed = matches!(bytes[0], b'-' | b'+');
    let negative = bytes[0] == b'-';
    let mut i = usize::from(signed);
    let y_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let digits = &input[y_start..i];
    if negative && digits == "000000" {
        return Err("invalid ISO negative zero year".into());
    }
    if i < bytes.len() && bytes[i] == b'-' {
        // 扩展形：无符号 4 位年或带符号 6 位年，随后 -MM[-DD]。
        let expected = usize::from(signed) * 6 + 4 * (1 - usize::from(signed));
        if digits.len() != expected {
            return Err("invalid ISO year".into());
        }
        let year = digits.parse::<i32>().map_err(|_| "invalid ISO year".to_string())?;
        let year = if negative { -year } else { year };
        i += 1;
        let month = read_iso2(input, &mut i, bytes)?;
        if i >= bytes.len() {
            return finish_year_month_range(year, month, None);
        }
        if bytes[i] != b'-' {
            return Err("invalid trailing content".into());
        }
        i += 1;
        let day = read_iso2(input, &mut i, bytes)?;
        if i != bytes.len() {
            return Err("invalid trailing content".into());
        }
        return finish_year_month_range(year, month, Some(day));
    }
    if i != bytes.len() {
        return Err("invalid trailing content".into());
    }
    // 紧凑纯数字：无符号 6（年+月）/ 8（年+月+日）；带符号 8 / 10（6 位年起步）。
    let len = digits.len();
    if !(matches!(len, 6 | 8) && !signed || matches!(len, 8 | 10) && signed) {
        return Err("invalid ISO date".into());
    }
    let month_start = if signed { 6 } else { 4 };
    let year = digits[..month_start]
        .parse::<i32>()
        .map_err(|_| "invalid ISO year".to_string())?;
    let year = if negative { -year } else { year };
    let month = digits[month_start..month_start + 2]
        .parse::<u32>()
        .map_err(|_| "invalid ISO month".to_string())?;
    let day = if digits.len() > month_start + 2 {
        Some(
            digits[month_start + 2..]
                .parse::<u32>()
                .map_err(|_| "invalid ISO day".to_string())?,
        )
    } else {
        None
    };
    finish_year_month_range(year, month, day)
}

/// 解析 PlainYearMonth ISO 串（年-月 / 年-月-日 / 完整 datetime 形态）。
/// 返回 (year, month)：日部分仅参与语法级校验，参考日恒 1（不入结果）。
fn parse_year_month_string(input: &str) -> Result<(i32, u32), String> {
    let trimmed = input.trim();
    if trimmed.contains('\u{2212}') {
        return Err("variant minus sign is not valid for PlainYearMonth".into());
    }
    let text = trimmed.to_owned();
    let annotation_start = text.find('[').unwrap_or(text.len());
    validate_month_day_annotation_suffix(&text[annotation_start..])?;
    let text = &text[..annotation_start];
    if text.contains('Z') || text.contains('z') {
        return Err("UTC designator is not valid for PlainYearMonth".into());
    }
    let separator = text.find(['T', 't', ' ']);
    let (date_part, time_part) = match separator {
        Some(index) => (&text[..index], &text[index + 1..]),
        None => (text, ""),
    };
    if !time_part.is_empty() {
        validate_month_day_time_suffix(time_part)?;
    }
    let (year, month, _) = parse_year_month_date_part(date_part)?;
    Ok((year, month))
}

/// `Temporal.PlainMonthDay.from(item[, options])`：ISO 串 / PMD 实例 / PD 实例 / 字段对象。
/// 参考年恒 1972（串中年份与 bag year 均不入结果，year 仅供 overflow 月长判断）；
/// options.overflow 在全部字段 Get 之后读取。
pub fn plain_month_day_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if value.is_string() {
        // 规范顺序：先解析（错误即抛，不触 options），再读 overflow（结果不受其影响）。
        let (month, day) = native_try!(parse_month_day_string(&to_string(value))
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 month-day string")));
        native_try!(temporal_overflow(vm, args));
        return make_plain_month_day(vm, month, day, 1972, "iso8601");
    }
    if !value.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "from() argument must be a string or an object"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "from() argument must be a string or an object"));
    }
    let obj = unsafe { &*ptr };
    // 实例快路径：槽直读，不触发属性。
    if obj.is_plain_month_day_obj() {
        // 实例复制：槽直读（refYear/日历保留），overflow 读取但结果不受其影响。
        native_try!(temporal_overflow(vm, args));
        let calendar = get_calendar_id(obj, 3);
        return make_plain_month_day(
            vm,
            get_double_prop(obj, 0) as u32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as i32,
            &calendar,
        );
    }
    if obj.is_plain_date_obj() {
        // PlainDate 实例：年月日槽直读，overflow 按其年应用，参考年 1972。
        let constrain = native_try!(temporal_overflow(vm, args));
        let year = get_double_prop(obj, 0) as i32;
        let month = get_double_prop(obj, 1) as u32;
        let mut day = get_double_prop(obj, 2) as u32;
        let calendar = get_calendar_id(obj, 3);
        if constrain {
            day = day.min(days_in_month_iso(year, month));
        }
        return make_plain_month_day(vm, month, day, 1972, &calendar);
    }
    // 字段对象：Get 序 calendar → day → month → monthCode → year → era → eraYear，
    // 全部读原始值（不转换）；overflow 在全部 Get 之后读取。
    let calendar_raw = native_try!(temporal_option_value(vm, obj, value, "calendar"));
    let calendar = native_try!(month_day_bag_calendar_id(vm, calendar_raw));
    let day_raw = native_try!(temporal_option_value(vm, obj, value, "day"));
    let month_raw = native_try!(temporal_option_value(vm, obj, value, "month"));
    let month_code_raw = native_try!(temporal_option_value(vm, obj, value, "monthCode"));
    let month_code = match month_code_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_option_string(vm, month_code_raw))),
    };
    let year_raw = native_try!(temporal_option_value(vm, obj, value, "year"));
    let era_raw = native_try!(temporal_option_value(vm, obj, value, "era"));
    let era_year_raw = native_try!(temporal_option_value(vm, obj, value, "eraYear"));
    let constrain = native_try!(temporal_overflow(vm, args));
    // era 族：双现 → RangeError；恰一个 → 忽略（ISO 无纪元体系）。
    if !era_raw.is_undefined() && !era_year_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_range_error(vm, "era and eraYear cannot both be present"));
    }
    // 月来源：双给 → RangeError；双缺 → TypeError（先于日存在性）。
    if !month_raw.is_undefined() && month_code.is_some() {
        return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode cannot both be present"));
    }
    if month_raw.is_undefined() && month_code.is_none() {
        return NativeResult::Err(crate::error::create_type_error(vm, "month or monthCode is required"));
    }
    if day_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "day is required"));
    }
    // monthCode 两段校验：第一段语法（"M"+两位数字，可选再 +"L"）先于一切数值转换；
    // 第二段适配（闰月后缀、1..12 范围）在 year 转换之后。
    let (month_code_num, leap_month) = match &month_code {
        Some(text) => {
            let b = text.as_bytes();
            let well_formed = b[0] == b'M'
                && b.len() >= 3
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && (b.len() == 3 || (b.len() == 4 && b[3] == b'L'));
            if !well_formed {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
            }
            (Some(u32::from(b[1] - b'0') * 10 + u32::from(b[2] - b'0')), b.len() == 4)
        }
        None => (None, false),
    };
    // year 转换（TypeError/RangeError）在 monthCode 语法之后。
    let year = match year_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, year_raw)) as i32),
    };
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, month_raw))),
    };
    // monthCode 第二段适配：闰月后缀或月值越界 → RangeError。
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：monthCode 与数值 month 并存须一致（冲突 → RangeError）；
    // 数值 month 负值恒 RangeError，>12 时 constrain 钳 12、reject 抛错。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        let f = month_f.expect("month or monthCode is required checked above");
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
        }
        if f > 12.0 {
            if constrain {
                12
            } else {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
            }
        } else {
            f as u32
        }
    };
    let day_f = native_try!(temporal_number_component(vm, day_raw));
    if day_f < 1.0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
    }
    // overflow 应用：月长按 year（bag 缺省 1972）判断；constrain 钳制，reject 抛错。
    let overflow_year = year.unwrap_or(1972);
    let days = days_in_month_iso(overflow_year, month) as i32;
    let day_i = day_f as i32;
    let day = if day_i > days {
        if constrain {
            days
        } else {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
        }
    } else {
        day_i
    };
    make_plain_month_day(vm, month, day as u32, 1972, &calendar)
}

/// `Temporal.PlainYearMonth.from(item[, options])`：ISO 串 / PYM 实例 / PD 实例 / 字段对象。
/// 串路径按 (年, 月) 查表示范围（日不参与）；实例分支槽直读（PYM 复制保留参考日，
/// PD 参考日恒 1）；bag 路径 year 必填、day 从不读取（参考日恒 1）、
/// monthCode 先 ToPrimitive 再两段校验（语法先于 year 转换、适配在 year 转换后）。
pub fn plain_year_month_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if value.is_string() {
        // 规范顺序：先解析（坏串 RangeError 不触 options），再读 overflow，再查 (年, 月) 范围。
        let (year, month) = native_try!(parse_year_month_string(&to_string(value))
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 year-month string")));
        native_try!(temporal_overflow(vm, args));
        if !iso_year_month_within_limits(year, month) {
            return NativeResult::Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
        }
        return make_plain_year_month(vm, year, month, 1, "iso8601");
    }
    if !value.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "from() argument must be a string or an object"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "from() argument must be a string or an object"));
    }
    let obj = unsafe { &*ptr };
    // 实例快路径：槽直读，不触发属性。
    if obj.is_plain_year_month_obj() {
        // 实例复制：overflow 先读（坏 options 先抛）后复制槽（参考日/日历保留）。
        native_try!(temporal_overflow(vm, args));
        let calendar = get_calendar_id(obj, 3);
        return make_plain_year_month(
            vm,
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
            &calendar,
        );
    }
    if obj.is_plain_date_obj() {
        // PlainDate 实例：年/月槽直读，参考日恒 1（日不读），日历槽直读。
        native_try!(temporal_overflow(vm, args));
        let year = get_double_prop(obj, 0) as i32;
        let month = get_double_prop(obj, 1) as u32;
        let calendar = get_calendar_id(obj, 3);
        return make_plain_year_month(vm, year, month, 1, &calendar);
    }
    // 字段对象：Get 序 calendar → year → month → monthCode → era → eraYear（day 不读）；
    // overflow 在全部 Get 之后读取。
    let calendar_raw = native_try!(temporal_option_value(vm, obj, value, "calendar"));
    let calendar = native_try!(month_day_bag_calendar_id(vm, calendar_raw));
    let year_raw = native_try!(temporal_option_value(vm, obj, value, "year"));
    let month_raw = native_try!(temporal_option_value(vm, obj, value, "month"));
    let month_code_raw = native_try!(temporal_option_value(vm, obj, value, "monthCode"));
    let era_raw = native_try!(temporal_option_value(vm, obj, value, "era"));
    let era_year_raw = native_try!(temporal_option_value(vm, obj, value, "eraYear"));
    let constrain = native_try!(temporal_overflow(vm, args));
    // era 族：双现 → RangeError；恰一个 → 忽略（ISO 无纪元体系）。
    if !era_raw.is_undefined() && !era_year_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_range_error(vm, "era and eraYear cannot both be present"));
    }
    // 存在性：year 必填（先于 monthCode 语法）；month|monthCode 至少其一。
    if year_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "year is required"));
    }
    if month_raw.is_undefined() && month_code_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "month or monthCode is required"));
    }
    // monthCode ToPrimitive 语义：字符串直用；对象（含函数）经 valueOf/toString；
    // 转换结果非字符串（number/bigint/boolean/null/Symbol 等）→ TypeError。
    let month_code = if month_code_raw.is_undefined() {
        None
    } else if month_code_raw.is_string() {
        Some(to_string(month_code_raw))
    } else if month_code_raw.is_object() {
        let primitive =
            match oxide_runtime_api::to_primitive(month_code_raw, oxide_runtime_api::ToPrimitiveHint::String, vm) {
                Ok(primitive) => primitive,
                Err(error) => return NativeResult::Err(native_engine_error(vm, &error)),
            };
        if !primitive.is_string() {
            return NativeResult::Err(crate::error::create_type_error(vm, "invalid monthCode"));
        }
        Some(to_string(primitive))
    } else {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid monthCode"));
    };
    // monthCode 第一段语法（"M"+两位数字，可选再 +"L"）先于一切数值转换。
    let (month_code_num, leap_month) = match &month_code {
        Some(text) => {
            let b = text.as_bytes();
            let well_formed = b[0] == b'M'
                && b.len() >= 3
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && (b.len() == 3 || (b.len() == 4 && b[3] == b'L'));
            if !well_formed {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
            }
            (Some(u32::from(b[1] - b'0') * 10 + u32::from(b[2] - b'0')), b.len() == 4)
        }
        None => (None, false),
    };
    // year 转换（TypeError/RangeError）在 monthCode 语法之后。
    let year = native_try!(temporal_number_component(vm, year_raw)) as i32;
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, month_raw))),
    };
    // monthCode 第二段适配：闰月后缀或月值越界 → RangeError。
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：monthCode 与数值 month 并存须一致（冲突 → RangeError）；
    // 数值 month <1 恒 RangeError，>12 时 constrain 钳 12、reject 抛错。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        let f = month_f.expect("month or monthCode is required checked above");
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
        }
        if f > 12.0 {
            if constrain {
                12
            } else {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
            }
        } else {
            f as u32
        }
    };
    // (年, 月) 表示范围。
    if !iso_year_month_within_limits(year, month) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
    }
    make_plain_year_month(vm, year, month, 1, &calendar)
}

/// ToTemporalYearMonth 四分支共享件（compare 等无 options 参数的成员复用）：
/// 串（解析 + (年, 月) 范围，参考日恒 1）/ PYM 实例（槽直读，参考日保留）/
/// PD 实例（年月槽直读，参考日恒 1）/ 字段 bag（year 必填、day 从不读取、参考日恒 1、
/// monthCode 两段校验同 from）；constrain 决定 month >12 钳制或抛错。
fn year_month_like_parts<H: VmHost>(
    vm: &mut H, value: JsValue, constrain: bool,
) -> Result<(i32, u32, u32, String), JsValue> {
    if value.is_string() {
        let (year, month) = parse_year_month_string(&to_string(value))
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 year-month string"))?;
        if !iso_year_month_within_limits(year, month) {
            return Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
        }
        return Ok((year, month, 1, "iso8601".to_string()));
    }
    if !value.is_object() {
        return Err(crate::error::create_type_error(vm, "argument must be a string or an object"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "argument must be a string or an object"));
    }
    let obj = unsafe { &*ptr };
    // 实例快路径：槽直读，不触发属性。
    if obj.is_plain_year_month_obj() {
        return Ok((
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
            get_calendar_id(obj, 3),
        ));
    }
    if obj.is_plain_date_obj() {
        return Ok((get_double_prop(obj, 0) as i32, get_double_prop(obj, 1) as u32, 1, get_calendar_id(obj, 3)));
    }
    // 字段对象：Get 序 calendar → year → month → monthCode → era → eraYear（day 不读）。
    let calendar_raw = temporal_option_value(vm, obj, value, "calendar")?;
    let calendar = month_day_bag_calendar_id(vm, calendar_raw)?;
    let year_raw = temporal_option_value(vm, obj, value, "year")?;
    let month_raw = temporal_option_value(vm, obj, value, "month")?;
    let month_code_raw = temporal_option_value(vm, obj, value, "monthCode")?;
    let era_raw = temporal_option_value(vm, obj, value, "era")?;
    let era_year_raw = temporal_option_value(vm, obj, value, "eraYear")?;
    // era 族：双现 → RangeError；恰一个 → 忽略（ISO 无纪元体系）。
    if !era_raw.is_undefined() && !era_year_raw.is_undefined() {
        return Err(crate::error::create_range_error(vm, "era and eraYear cannot both be present"));
    }
    // 存在性：year 必填（先于 monthCode 语法）；month|monthCode 至少其一。
    if year_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "year is required"));
    }
    if month_raw.is_undefined() && month_code_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "month or monthCode is required"));
    }
    // monthCode ToPrimitive 语义：字符串直用；对象（含函数）经 valueOf/toString；
    // 转换结果非字符串（number/bigint/boolean/null/Symbol 等）→ TypeError。
    let month_code = if month_code_raw.is_undefined() {
        None
    } else if month_code_raw.is_string() {
        Some(to_string(month_code_raw))
    } else if month_code_raw.is_object() {
        let primitive =
            match oxide_runtime_api::to_primitive(month_code_raw, oxide_runtime_api::ToPrimitiveHint::String, vm) {
                Ok(primitive) => primitive,
                Err(error) => return Err(native_engine_error(vm, &error)),
            };
        if !primitive.is_string() {
            return Err(crate::error::create_type_error(vm, "invalid monthCode"));
        }
        Some(to_string(primitive))
    } else {
        return Err(crate::error::create_type_error(vm, "invalid monthCode"));
    };
    // monthCode 第一段语法（"M"+两位数字，可选再 +"L"）先于一切数值转换。
    let (month_code_num, leap_month) = match &month_code {
        Some(text) => {
            let b = text.as_bytes();
            let well_formed = b[0] == b'M'
                && b.len() >= 3
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && (b.len() == 3 || (b.len() == 4 && b[3] == b'L'));
            if !well_formed {
                return Err(crate::error::create_range_error(vm, "invalid monthCode"));
            }
            (Some(u32::from(b[1] - b'0') * 10 + u32::from(b[2] - b'0')), b.len() == 4)
        }
        None => (None, false),
    };
    // year 转换（TypeError/RangeError）在 monthCode 语法之后。
    let year = temporal_number_component(vm, year_raw)? as i32;
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(temporal_number_component(vm, month_raw)?),
    };
    // monthCode 第二段适配：闰月后缀或月值越界 → RangeError。
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：monthCode 与数值 month 并存须一致（冲突 → RangeError）；
    // 数值 month <1 恒 RangeError，>12 时 constrain 钳 12、reject 抛错。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        let f = month_f.expect("month or monthCode is required checked above");
        if f < 1.0 {
            return Err(crate::error::create_range_error(vm, "invalid month"));
        }
        if f > 12.0 {
            if constrain {
                12
            } else {
                return Err(crate::error::create_range_error(vm, "invalid month"));
            }
        } else {
            f as u32
        }
    };
    // (年, 月) 表示范围。
    if !iso_year_month_within_limits(year, month) {
        return Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
    }
    Ok((year, month, 1, calendar))
}

/// `Temporal.PlainYearMonth.compare(one, two)`：静态比较，返回 -1/0/1。
/// 比较 (年, 月, 参考日) 三元字典序；两参均经 ToTemporalYearMonth。
pub fn plain_year_month_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let a_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let b_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let a = native_try!(year_month_like_parts(vm, a_val, true));
    let b = native_try!(year_month_like_parts(vm, b_val, true));
    let cmp = (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2));
    NativeResult::Ok(JsValue::float(match cmp {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

/// `Temporal.PlainYearMonth.prototype.equals(other)`：比较 (年, 月, 参考日) 三元与日历标识；
/// other 经 ToTemporalYearMonth（串 / 字段对象 / PD 实例 / PYM 实例）转换。
pub fn plain_year_month_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let other = match plain_year_month_from(vm, args) {
        NativeResult::Ok(val) => val,
        NativeResult::Err(err) => return NativeResult::Err(err),
        NativeResult::TailCall { .. } => unreachable!("工厂路径不产生尾调用"),
    };
    let other_ptr = other.as_js_object_ptr();
    if other_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "incompatible receiver"));
    }
    let o = unsafe { &*other_ptr };
    let equal = get_double_prop(obj, 0) == get_double_prop(o, 0)
        && get_double_prop(obj, 1) == get_double_prop(o, 1)
        && get_double_prop(obj, 2) == get_double_prop(o, 2)
        && get_calendar_id(obj, 3) == get_calendar_id(o, 3);
    NativeResult::Ok(JsValue::bool(equal))
}

/// add/subtract 的 (年, 月) 可表示性检查，按日 = 1 计算（ISODateWithinLimits）。
/// 与 ISOYearMonthWithinLimits 的差异仅在 -271821 年：该年 4 月 1 日早于最早
/// 可表示的日期时间，故 4 月及以前越界（5 月起在界内）；+275760 年须 9 月及以前。
fn iso_year_month_day1_within_limits(year: i64, month: i64) -> bool {
    if !(-271_821_i64..=275_760_i64).contains(&year) {
        return false;
    }
    !((year == -271_821 && month < 5) || (year == 275_760 && month > 9))
}

/// `Temporal.PlainYearMonth.prototype.add/subtract(durationLike [, options])` 核心。
///
/// # 步骤
/// 1. receiver branding（TypeError）。
/// 2. ToTemporalDuration（串 / bag / 实例；TypeError、RangeError 各自语义）。
/// 3. 读 overflow（规范顺序先于后续算法校验；ISO 日历下取值不可观测，仅校验）。
/// 4. weeks / days / 时间分量任一非零 → RangeError。
/// 5. receiver (年, 月) 按日 1 检查可表示性 → RangeError。
/// 6. 年、月分量折算绝对月数后相加再平衡（div/rem_euclid）。
/// 7. 结果 (年, 月) 按日 1 检查可表示性 → RangeError。
///
/// # 边界与副作用
/// - 结果参考日恒 1，日历标识取自 receiver 槽 3。
/// - subtract 与 add 共用本函数，sign 参数取反时长分量。
fn plain_year_month_apply_duration<H: VmHost>(vm: &mut H, args: &[u8], sign: i64) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = match duration_like_values(vm, val) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
    let _overflow = native_try!(temporal_overflow(vm, args));
    if values[2..].iter().any(|value| *value != 0.0) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "year-month cannot add weeks, days, or time units",
        ));
    }
    let year = get_double_prop(obj, 0) as i64;
    let month = get_double_prop(obj, 1) as i64;
    if !iso_year_month_day1_within_limits(year, month) {
        return NativeResult::Err(crate::error::create_range_error(vm, "year-month is out of range"));
    }
    let months = (values[0] as i64) * 12 + values[1] as i64;
    let absolute = year * 12 + month - 1 + sign * months;
    let result_year = absolute.div_euclid(12);
    let result_month = absolute.rem_euclid(12) + 1;
    if !iso_year_month_day1_within_limits(result_year, result_month) {
        return NativeResult::Err(crate::error::create_range_error(vm, "year-month is out of range"));
    }
    make_plain_year_month(vm, result_year as i32, result_month as u32, 1, &get_calendar_id(obj, 3))
}

/// `Temporal.PlainYearMonth.prototype.add(durationLike)`。
pub fn plain_year_month_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_apply_duration(vm, args, 1)
}

/// `Temporal.PlainYearMonth.prototype.subtract(durationLike)`。
pub fn plain_year_month_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_apply_duration(vm, args, -1)
}

/// `Temporal.PlainYearMonth.prototype.with(yearMonthLike[, options])`：覆盖 year/month/monthCode；
/// undefined 不覆盖；calendar/timeZone 键拒绝；无识别字段 → TypeError。
///
/// # 步骤
/// 1. receiver branding（TypeError）。
/// 2. reject calendar/timeZone 键与 Temporal 实例（TypeError）。
/// 3. Get 序 month → monthCode → year（字典序）；全 undefined → TypeError。
/// 4. monthCode 第一段语法校验与字段数值转换先于 options 读取。
/// 5. 读 overflow（缺省 constrain）；monthCode 第二段适配（闰月后缀/1..12）与
///    month/monthCode 冲突、>12 钳制在此。
/// 6. 缺省分量取 receiver 槽（年 0 / 月 1）；(年, 月) 过 ISOYearMonthWithinLimits。
///
/// # 边界与副作用
/// - 结果参考日恒 1，日历标识取自 receiver 槽 3。
/// - 年月级范围检查（非按日 1），-271821-04 可经 with 产出。
pub fn plain_year_month_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let like = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    native_try!(reject_partial_object_with_calendar_or_time_zone(vm, like));
    let lptr = like.as_js_object_ptr();
    if lptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let like_obj = unsafe { &*lptr };
    // Get 序（字典序）：month → monthCode → year；无识别字段 → TypeError。
    let month_raw = native_try!(temporal_option_value(vm, like_obj, like, "month"));
    let month_code_raw = native_try!(temporal_option_value(vm, like_obj, like, "monthCode"));
    let month_code = match month_code_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_option_string(vm, month_code_raw))),
    };
    let year_raw = native_try!(temporal_option_value(vm, like_obj, like, "year"));
    if [month_raw, month_code_raw, year_raw].iter().all(|raw| raw.is_undefined()) {
        return NativeResult::Err(crate::error::create_type_error(vm, "no properties present"));
    }
    // monthCode 第一段语法（"M"+两位数字，可选再 +"L"）先于一切数值转换。
    let (month_code_num, leap_month) = match &month_code {
        Some(text) => {
            let b = text.as_bytes();
            let well_formed = b[0] == b'M'
                && b.len() >= 3
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && (b.len() == 3 || (b.len() == 4 && b[3] == b'L'));
            if !well_formed {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
            }
            (Some(u32::from(b[1] - b'0') * 10 + u32::from(b[2] - b'0')), b.len() == 4)
        }
        None => (None, false),
    };
    // 部分字段先于 options 完整转换（RangeError/TypeError 先于 options 形态错误）。
    let year = match year_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, year_raw)) as i32),
    };
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, month_raw))),
    };
    if let Some(f) = month_f {
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
        }
    }
    let constrain = native_try!(temporal_overflow(vm, args));
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：monthCode 与数值 month 并存须一致（冲突 → RangeError）；
    // 数值 month >12 时 constrain 钳 12、reject 抛错；缺省取 receiver 月（槽 1）。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        match month_f {
            Some(f) => {
                if f > 12.0 {
                    if constrain {
                        12
                    } else {
                        return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
                    }
                } else {
                    f as u32
                }
            }
            None => get_double_prop(obj, 1) as u32,
        }
    };
    let year = year.unwrap_or_else(|| get_double_prop(obj, 0) as i32);
    // (年, 月) 可表示范围（年月级，非按日 1）。
    if !iso_year_month_within_limits(year, month) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
    }
    let calendar = get_calendar_id(obj, 3);
    make_plain_year_month(vm, year, month, 1, &calendar)
}

/// `Temporal.PlainYearMonth.prototype.toPlainDate(dayLike)`：dayLike 须为对象，
/// 只 Get `day`（year/month/monthCode/calendar 从不读取）；options.overflow 绝不读取；
/// 合并 (年, 月, 日) 后 constrain 钳 day 至 [1, 月长]，day 级可表示范围检查后
/// 产出 PlainDate（receiver 日历）。
pub fn plain_year_month_to_plain_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let item = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !item.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let iptr = item.as_js_object_ptr();
    if iptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let item_obj = unsafe { &*iptr };
    let day_raw = native_try!(temporal_option_value(vm, item_obj, item, "day"));
    if day_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "day is required"));
    }
    let day = native_try!(temporal_number_component(vm, day_raw));
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as u32;
    // constrain：day 钳到 [1, 月长]。
    let day = (day as i32).clamp(1, days_in_month_iso(year, month) as i32) as u32;
    // day 级表示范围：-100_000_001..=100_000_000（年月级合法而 day 级越界在此抛）。
    let day_count = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
    }
    let calendar = get_calendar_id(obj, 3);
    make_plain_date(vm, year, month, day, &calendar)
}

/// `Temporal.PlainYearMonth.prototype.until(other[, options])`。
pub fn plain_year_month_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_difference(vm, args, false)
}

/// `Temporal.PlainYearMonth.prototype.since(other[, options])`。
pub fn plain_year_month_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_difference(vm, args, true)
}

/// until/since 差值核心（DifferenceTemporalPlainYearMonth）。
///
/// # 步骤
/// 1. receiver branding（TypeError）。
/// 2. other 经 ToTemporalYearMonth（串 / bag / PD / PYM 实例；day 不读）。
/// 3. 日历相等（不等 RangeError；在 options 读取之前）。
/// 4. options 对象 + 差值设置（单位表仅 year/month，smallestUnit 缺省 month，
///    largestUnit 缺省/auto 为 year）。
/// 5. (年, 月, 参考日) 三元全等 → 空 Duration（先于可表示性检查）。
/// 6. 两端按日 1 查可表示性（ISODateWithinLimits，day 级）→ RangeError。
/// 7. 既有差值机件（nudge 窗口端点同样带 day 级范围检查）；since 整体取反。
///
/// # 边界与副作用
/// - 参考日只参与第 5 步的全等判断；差值本身恒按日 1 计算。
/// - 第 2 步的字符串/袋路径为年月级范围检查（自 ToTemporalYearMonth），
///   day 级补查在第 6 步，两步错误类相同（RangeError），仅抛错时点不同。
fn plain_year_month_difference<H: VmHost>(vm: &mut H, args: &[u8], since: bool) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (oy, om, od, ocal) = native_try!(year_month_like_parts(vm, other_val, true));
    // 日历相等（ToTemporalYearMonth 之后、options 读取之前）。
    if get_calendar_id(obj, 3) != ocal {
        return NativeResult::Err(crate::error::create_range_error(vm, "calendars must be equal"));
    }
    let options_value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let settings = match parse_year_month_difference_settings(vm, options_value) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    // (年, 月, 参考日) 三元全等 → 空 Duration（先于可表示性检查）。
    let ry = get_double_prop(obj, 0) as i128;
    let rm = get_double_prop(obj, 1) as i128;
    let rd = get_double_prop(obj, 2) as i128;
    if compare_iso_date((ry, rm, rd), (i128::from(oy), i128::from(om), i128::from(od))) == 0 {
        return make_duration(vm, [0.0; 10]);
    }
    // 两端按日 1 的可表示性检查（ISODateWithinLimits，day 级）。
    if days_from_civil(ry, rm, 1).abs() > MAX_ISO_DAY
        || days_from_civil(i128::from(oy), i128::from(om), 1).abs() > MAX_ISO_DAY
    {
        return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
    }
    difference_core(vm, (ry, rm, 1), 0, (i128::from(oy), i128::from(om), 1), 0, settings, since, 1)
}

/// `Temporal.PlainMonthDay.prototype.equals(other)`：比较 (月, 日, 参考年) 三元与日历标识；
/// other 经 ToTemporalMonthDay（串 / 字段对象 / 实例）转换。
pub fn plain_month_day_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let other = match plain_month_day_from(vm, args) {
        NativeResult::Ok(val) => val,
        NativeResult::Err(err) => return NativeResult::Err(err),
        NativeResult::TailCall { .. } => unreachable!("工厂路径不产生尾调用"),
    };
    let other_ptr = other.as_js_object_ptr();
    if other_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "incompatible receiver"));
    }
    let o = unsafe { &*other_ptr };
    let equal = get_double_prop(obj, 0) == get_double_prop(o, 0)
        && get_double_prop(obj, 1) == get_double_prop(o, 1)
        && get_double_prop(obj, 2) == get_double_prop(o, 2)
        && get_calendar_id(obj, 3) == get_calendar_id(o, 3);
    NativeResult::Ok(JsValue::bool(equal))
}

/// `Temporal.PlainMonthDay.prototype.with(monthDayLike[, options])`：覆盖 day/month/monthCode；
/// year 只参与 overflow 月长判断；undefined 不覆盖；calendar/timeZone 键拒绝；
/// 无识别字段 TypeError；overflow 读自 options（缺省 constrain）。
/// 结果保留 receiver 日历，参考年恒 1972。
pub fn plain_month_day_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let like = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    native_try!(reject_partial_object_with_calendar_or_time_zone(vm, like));
    let lptr = like.as_js_object_ptr();
    if lptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let like_obj = unsafe { &*lptr };
    // Get 序（字典序）：day → month → monthCode → year；无识别字段 → TypeError。
    let day_raw = native_try!(temporal_option_value(vm, like_obj, like, "day"));
    let month_raw = native_try!(temporal_option_value(vm, like_obj, like, "month"));
    let month_code_raw = native_try!(temporal_option_value(vm, like_obj, like, "monthCode"));
    let month_code = match month_code_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_option_string(vm, month_code_raw))),
    };
    let year_raw = native_try!(temporal_option_value(vm, like_obj, like, "year"));
    if [day_raw, month_raw, month_code_raw, year_raw]
        .iter()
        .all(|raw| raw.is_undefined())
    {
        return NativeResult::Err(crate::error::create_type_error(vm, "no properties present"));
    }
    // monthCode 两段校验：语法（"M"+两位数字，可选 +"L"）先于 year 转换；
    // 适配（闰月后缀、1..12 范围）与冲突检查在 options 读取之后。
    let (month_code_num, leap_month) = match &month_code {
        Some(text) => {
            let b = text.as_bytes();
            let well_formed = b[0] == b'M'
                && b.len() >= 3
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && (b.len() == 3 || (b.len() == 4 && b[3] == b'L'));
            if !well_formed {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
            }
            (Some(u32::from(b[1] - b'0') * 10 + u32::from(b[2] - b'0')), b.len() == 4)
        }
        None => (None, false),
    };
    // 部分字段先于 options 完整转换（RangeError/TypeError 先于 options 形态错误）。
    let year = match year_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, year_raw)) as i32),
    };
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, month_raw))),
    };
    let day_f = match day_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, day_raw))),
    };
    if let Some(f) = month_f {
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
        }
    }
    if let Some(f) = day_f {
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
        }
    }
    let constrain = native_try!(temporal_overflow(vm, args));
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：receiver 月（槽 0）为缺省；monthCode 与数值 month 并存须一致；
    // 数值 month 负值恒 RangeError，>12 时 constrain 钳 12、reject 抛错。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        match month_f {
            Some(f) => {
                if f < 1.0 {
                    return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
                }
                if f > 12.0 {
                    if constrain {
                        12
                    } else {
                        return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
                    }
                } else {
                    f as u32
                }
            }
            None => get_double_prop(obj, 0) as u32,
        }
    };
    // 日：receiver 日（槽 1）为缺省（partial 日已先行转换并过 <1 检查）。
    let day_f = day_f.unwrap_or_else(|| get_double_prop(obj, 1));
    // overflow 应用：月长按 bag year（缺省 1972）判断；constrain 钳制，reject 抛错。
    let overflow_year = year.unwrap_or(1972);
    let days = days_in_month_iso(overflow_year, month) as i32;
    let day_i = day_f as i32;
    let day = if day_i > days {
        if constrain {
            days
        } else {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
        }
    } else {
        day_i
    };
    let calendar = get_calendar_id(obj, 3);
    make_plain_month_day(vm, month, day as u32, 1972, &calendar)
}

/// `Temporal.PlainMonthDay.prototype.toPlainDate(yearLike)`：yearLike 为 number 或 {year}；
/// 只 Get item 的 year；overflow 恒 constrain 且不读 options；
/// constrain 钳 day、day 级范围检查后产出 PlainDate（receiver 日历）。
pub fn plain_month_day_to_plain_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let item = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !item.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let iptr = item.as_js_object_ptr();
    if iptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let item_obj = unsafe { &*iptr };
    let year_raw = native_try!(temporal_option_value(vm, item_obj, item, "year"));
    if year_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "year is required"));
    }
    let year = native_try!(temporal_number_component(vm, year_raw)) as i32;
    let month = get_double_prop(obj, 0) as u32;
    // constrain：day 钳到 (year, month) 月长。
    let day = (get_double_prop(obj, 1) as u32).min(days_in_month_iso(year, month));
    // day 级表示范围：-100_000_001..=100_000_000。
    let day_count = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
    }
    let calendar = get_calendar_id(obj, 3);
    make_plain_date(vm, year, month, day, &calendar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_calendar_id_whitelist() {
        // 18 项白名单全部命中且返回规范小写形式。
        for id in CALENDAR_ID_WHITELIST {
            assert_eq!(is_builtin_calendar_id(id), Some(id), "白名单项 {id} 应命中自身");
        }
        // ASCII 大小写折叠：仅首字母大写与全大写变体命中。
        assert_eq!(is_builtin_calendar_id("IsO8601"), Some("iso8601"));
        assert_eq!(is_builtin_calendar_id("HEBREW"), Some("hebrew"));
        assert_eq!(is_builtin_calendar_id("Gregory"), Some("gregory"));
        // 点 I（U+0130）不属于 ASCII 折叠域，必须拒绝，否则误判为 iso8601。
        assert_eq!(is_builtin_calendar_id("\u{0130}SO8601"), None);
        // 白名单外与类日期串拒绝。
        assert_eq!(is_builtin_calendar_id("notacal"), None);
        assert_eq!(is_builtin_calendar_id("1111-11-11"), None);
        assert_eq!(is_builtin_calendar_id("11111111"), None);
    }

    #[test]
    fn strict_calendar_id_parser() {
        // 严格解析：只收白名单 ID，ISO 串 / 未知值 / 空串全拒。
        assert_eq!(parse_temporal_calendar_id_strict("hebrew"), Ok("hebrew".to_string()));
        assert_eq!(parse_temporal_calendar_id_strict("IsO8601"), Ok("iso8601".to_string()));
        assert_eq!(parse_temporal_calendar_id_strict(" gregory "), Ok("gregory".to_string()));
        assert_eq!(
            parse_temporal_calendar_id_strict("1997-12-04[u-ca=iso8601]"),
            Err("invalid calendar".to_string())
        );
        assert_eq!(parse_temporal_calendar_id_strict("11111111"), Err("invalid calendar".to_string()));
        assert_eq!(parse_temporal_calendar_id_strict("\u{0130}SO8601"), Err("invalid calendar".to_string()));
        assert_eq!(parse_temporal_calendar_id_strict(""), Err("invalid calendar".to_string()));
    }

    #[test]
    fn loose_calendar_string_parser() {
        // 宽松解析（property bag 路径）：白名单 ID 返回规范小写，ISO 串路径恒为 iso8601。
        assert_eq!(parse_temporal_calendar_string("gregory"), Ok("gregory".to_string()));
        assert_eq!(parse_temporal_calendar_string("iSo8601"), Ok("iso8601".to_string()));
        assert_eq!(parse_temporal_calendar_string("1997-12-04"), Ok("iso8601".to_string()));
        assert_eq!(parse_temporal_calendar_string("1997-12-04[u-ca=iso8601]"), Ok("iso8601".to_string()));
        assert_eq!(parse_temporal_calendar_string(""), Err("invalid calendar".to_string()));
        // 注解值未过白名单 / 裸未知标识符，两种路径都拒绝。
        assert!(parse_temporal_calendar_string("[u-ca=notacal]").is_err());
        assert!(parse_temporal_calendar_string("notacal").is_err());
    }

    #[test]
    fn annotation_suffix_calendar_whitelist() {
        // 首个 u-ca 注解值过 18 项白名单：白名单内放行（含关键标记），白名单外拒绝。
        assert!(validate_temporal_annotation_suffix("[u-ca=hebrew]").is_ok());
        assert!(validate_temporal_annotation_suffix("[!u-ca=hebrew]").is_ok());
        assert!(validate_temporal_annotation_suffix("[u-ca=iSo8601]").is_ok());
        assert!(validate_temporal_annotation_suffix("[u-ca=notacal]").is_err());
        assert!(validate_temporal_annotation_suffix("[u-ca=1111-11-11]").is_err());
        // 点 I 变体不做 Unicode 折叠，拒绝。
        assert!(validate_temporal_annotation_suffix("[u-ca=\u{0130}SO8601]").is_err());
        // 第二及后续 u-ca 注解被忽略，不参与校验；含关键标记的重复日历报错。
        assert!(validate_temporal_annotation_suffix("[u-ca=iso8601][u-ca=discord]").is_ok());
        assert!(validate_temporal_annotation_suffix("[u-ca=iso8601][!u-ca=iso8601]").is_err());
    }

    #[test]
    fn calendar_id_slot_fallback() {
        // 字符串槽原值返回由 VM 层构造器/getter 测试覆盖；此处验证 undefined 与非 string 槽兜底 iso8601。
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.set_prop_at(3, JsValue::undefined());
        assert_eq!(get_calendar_id(&obj, 3), "iso8601");
        obj.set_prop_at(3, JsValue::int(7));
        assert_eq!(get_calendar_id(&obj, 3), "iso8601");
    }

    #[test]
    fn start_of_day_epoch_ns_instant_range() {
        // 当地午夜回推 epoch 越 Instant 界（±MAX）返回 None，供 startOfDay/hoursInDay RangeError 依据。
        // -100000001 天 + 1h < -MAX → 越界。
        assert_eq!(start_of_day_epoch_ns(-271_821, 4, 19, -60), None);
        // -100000000 天（-271821-04-20）UTC 当地午夜恰为 -MAX，在边界内。
        assert_eq!(start_of_day_epoch_ns(-271_821, 4, 20, 0), Some(-8_640_000_000_000_000_000_000));
        // 同日期 +1h 偏移使当地午夜 -MAX - 1h 越界。
        assert_eq!(start_of_day_epoch_ns(-271_821, 4, 20, 60), None);
        // 明日边界越界：+100000001 天（UTC）。
        assert_eq!(start_of_day_epoch_ns_by_days(100_000_001, 0), None);
        // 正常日期返回当地午夜。
        assert_eq!(start_of_day_epoch_ns(1970, 1, 1, 0), Some(0));
        assert_eq!(start_of_day_epoch_ns(1970, 1, 1, 60), Some(-3_600_000_000_000));
    }

    #[test]
    fn format_time_zone_annotation_normalizes_offset() {
        // 命名区原样；偏移区规范化为 ±HH:MM 带冒号；critical 时 `!` 置于括号内。
        assert_eq!(format_time_zone_annotation("UTC", false), "[UTC]");
        assert_eq!(format_time_zone_annotation("+01:00", false), "[+01:00]");
        assert_eq!(format_time_zone_annotation("+01", false), "[+01:00]");
        assert_eq!(format_time_zone_annotation("-05:00", false), "[-05:00]");
        assert_eq!(format_time_zone_annotation("UTC", true), "[!UTC]");
        assert_eq!(format_time_zone_annotation("+01", true), "[!+01:00]");
    }

    #[test]
    fn format_zoned_date_time_iso_annotations() {
        // 偏移/时区/日历注解各取值组合，验证段序 `{offset}[{tz}][{ca}]` 与 critical 前缀。
        let base = format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "auto", "auto", "auto");
        assert_eq!(base, Some("1970-01-01T01:00:00+01:00[+01:00]".to_string()));
        // offset never 省略偏移段，时区注解仍显示。
        let no_offset = format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "never", "auto", "auto");
        assert_eq!(no_offset, Some("1970-01-01T01:00:00[+01:00]".to_string()));
        // calendarName always 追加日历注解。
        let ca_always = format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "auto", "auto", "always");
        assert_eq!(ca_always, Some("1970-01-01T01:00:00+01:00[+01:00][u-ca=iso8601]".to_string()));
        // timeZoneName/calendarName critical 均 `!` 置于括号内。
        let critical =
            format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "auto", "critical", "critical");
        assert_eq!(critical, Some("1970-01-01T01:00:00+01:00[!+01:00][!u-ca=iso8601]".to_string()));
        // offset critical 偏移段前加 !。
        let offset_critical =
            format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "critical", "auto", "auto");
        assert_eq!(offset_critical, Some("1970-01-01T01:00:00!+01:00[+01:00]".to_string()));
    }

    #[test]
    fn format_zoned_date_time_iso_epoch_rounding_cross_midnight() {
        // 2000-01-01 前 1ns 已舍入到 2000-01-01 整点，8 位小数输出。
        let rounded = round_instant_ns(946_684_799_999_999_999, 100, InstantRoundingMode::HalfExpand).unwrap();
        let output = format_zoned_date_time_iso(rounded, 0, "UTC", "iso8601", true, Some(8), "auto", "auto", "auto");
        assert_eq!(output, Some("2000-01-01T00:00:00.00000000+00:00[UTC]".to_string()));
    }

    #[test]
    fn local_to_epoch_ns_roundtrip() {
        // 与 parse_instant_string 互逆对拍：同一时刻的本地分量 + 偏移换算回 epoch 一致。
        assert_eq!(parse_instant_string("2024-01-01T00:00:00+01:00"), local_to_epoch_ns(2024, 1, 1, 0.0, 60),);
        assert_eq!(
            parse_instant_string("1969-07-16T13:32:01.234567891Z"),
            local_to_epoch_ns(1969, 7, 16, 48_721_234_567_891.0, 0),
        );
        // startOfDay 最小边界：-271821-04-20 加 1h 再回推 1h 偏移回到 -MAX。
        assert_eq!(
            local_to_epoch_ns(-271821, 4, 20, 3_600_000_000_000.0, 60),
            Some(-8_640_000_000_000_000_000_000),
        );
    }

    #[test]
    fn zoned_date_time_string_wall_day_range_boundary() {
        // ZDT 字符串的墙钟日范围（CheckISODaysRange）边界：±10^8 天内合法，
        // 第 ±100000001 天（-271821-04-19 / +275760-09-14）即使 epoch 换算回界内也拒绝。
        assert!(days_from_civil(-271_821, 4, 19).abs() > 100_000_000);
        assert!(days_from_civil(-271_821, 4, 20).abs() <= 100_000_000);
        assert!(days_from_civil(275_760, 9, 14).abs() > 100_000_000);
        assert!(days_from_civil(275_760, 9, 13).abs() <= 100_000_000);
        // 解析路径一致性：边界内字符串可解析，越界墙钟经偏移拉回界内也不放行。
        assert_eq!(
            parse_instant_string("+275760-09-13T01:00+01:00[+01:00]"),
            Some(8_640_000_000_000_000_000_000),
        );
    }

    #[test]
    fn canonical_time_zone_4_digit_offset() {
        // ±HHMM 无冒号形式归一：ID 保留原串，offset 分钟数正确换算。
        assert_eq!(canonical_time_zone("+0000"), Some(("+0000".to_string(), 0)));
        assert_eq!(canonical_time_zone("-0530"), Some(("-0530".to_string(), -330)));
        assert_eq!(canonical_time_zone("+2330"), Some(("+2330".to_string(), 1410)));
        // 非法分钟/小时拒绝。
        assert_eq!(canonical_time_zone("+2400"), None);
        assert_eq!(canonical_time_zone("+0060"), None);
        assert_eq!(canonical_time_zone("+123"), None); // 长度不符
    }

    #[test]
    fn extract_time_zone_annotation_forms() {
        // UTC / critical / 数值偏移各形式提取；u-ca 注解跳过，时区注解在前后均能取到。
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[UTC]"), Some(("UTC".to_string(), false)));
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[!UTC]"), Some(("UTC".to_string(), true)));
        assert_eq!(
            extract_time_zone_annotation("1976-11-18T15:23:30+01:00[+01:00]"),
            Some(("+01:00".to_string(), false))
        );
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[+01]"), Some(("+01".to_string(), false)));
        assert_eq!(
            extract_time_zone_annotation("1976-11-18T15:23:30[+0100]"),
            Some(("+0100".to_string(), false))
        );
        assert_eq!(
            extract_time_zone_annotation("1976-11-18T15:23:30[u-ca=iso8601][UTC]"),
            Some(("UTC".to_string(), false))
        );
        assert_eq!(
            extract_time_zone_annotation("1976-11-18T15:23:30[UTC][u-ca=iso8601]"),
            Some(("UTC".to_string(), false))
        );
        // 无注解 / 非法偏移注解返回 None。
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30Z"), None);
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[+24:00]"), None);
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[]"), None);
    }

    #[test]
    fn parse_plain_time_string_cases() {
        // 明确时间：T 前缀 / 冒号 / 非法日期数字串 / 带 offset 与注解。
        assert_eq!(parse_plain_time_string("T00:30"), Some(1_800_000_000_000.0));
        assert_eq!(parse_plain_time_string("T0030"), Some(1_800_000_000_000.0));
        assert_eq!(parse_plain_time_string("12:34:56.987654321+00:00"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("12:34:56.987654321+00:00[UTC]"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("1976-11-18T12:34:56.987654321+00:00"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("1976-11-18 12:34:56.987654321"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("1314"), Some(47_640_000_000_000.0)); // 13:14
        assert_eq!(parse_plain_time_string("0631"), Some(23_460_000_000_000.0)); // 06:31
        assert_eq!(parse_plain_time_string("2021-13"), Some(73_260_000_000_000.0)); // 20:21 + offset -13 忽略
                                                                                    // 歧义日期 → None（须 T 前缀）。
        assert_eq!(parse_plain_time_string("2019-10-01"), None);
        assert_eq!(parse_plain_time_string("1214"), None); // MMDD 合法
        assert_eq!(parse_plain_time_string("0229"), None); // 闰年 2 月 29 判歧义
        assert_eq!(parse_plain_time_string("1130"), None);
        assert_eq!(parse_plain_time_string("12-14"), None); // MM-DD 合法
        assert_eq!(parse_plain_time_string("202112"), None); // YYYYMM 合法
        assert_eq!(parse_plain_time_string("2021-12"), None); // YYYY-MM 合法
        assert_eq!(parse_plain_time_string("2021-12[-12:00]"), None);
        assert_eq!(parse_plain_time_string("202112[UTC]"), None);
        assert_eq!(parse_plain_time_string("1214[u-ca=iso8601]"), None);
        // T 前缀后歧义消除；空格不能替代 T。
        assert!(parse_plain_time_string("T2021-12").is_some());
        assert_eq!(parse_plain_time_string(" 2021-12"), None);
        // Z designator / 越界 / 多小数 / 负零年。
        assert_eq!(parse_plain_time_string("09:00:00Z"), None);
        assert_eq!(parse_plain_time_string("2019-10-01T09:00:00Z"), None);
        assert_eq!(parse_plain_time_string("24:00"), None);
        assert_eq!(parse_plain_time_string("12:34:56.1234567890"), None);
        assert_eq!(parse_plain_time_string("-000000-12-07T03:24:30"), None);
        // 闰秒按前一秒。
        assert_eq!(parse_plain_time_string("2016-12-31T23:59:60"), Some(86_399_000_000_000.0));
    }

    #[test]
    fn parse_plain_time_string_offsets_and_fractions() {
        // offset 小数秒 ≤9 位合法，>9 位拒绝；逗号小数接受。
        assert_eq!(
            parse_plain_time_string("12:34:56.987654321+00:00:00.000000000"),
            Some(45_296_987_654_321.0)
        );
        assert_eq!(parse_plain_time_string("12:34:56.987654321+00:00:00,0"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("00:00:00.1234567891"), None);
        assert_eq!(parse_plain_time_string("00+00:00:00.1234567891"), None);
        // 日期+offset 但无时间部分 → 拒绝。
        assert_eq!(parse_plain_time_string("2022-09-15Z"), None);
        assert_eq!(parse_plain_time_string("2022-09-15+00:00"), None);
    }

    #[test]
    fn zoned_date_time_round_unit_table() {
        // day 含独立条目，每单位 (ns, 更高一级数量) 与 spec 对齐；day 更高一级数量为 1。
        assert_eq!(zoned_date_time_round_unit("day"), Some((86_400_000_000_000, 1)));
        assert_eq!(zoned_date_time_round_unit("days"), Some((86_400_000_000_000, 1)));
        assert_eq!(zoned_date_time_round_unit("hour"), Some((3_600_000_000_000, 24)));
        assert_eq!(zoned_date_time_round_unit("minute"), Some((60_000_000_000, 60)));
        assert_eq!(zoned_date_time_round_unit("second"), Some((1_000_000_000, 60)));
        assert_eq!(zoned_date_time_round_unit("millisecond"), Some((1_000_000, 1_000)));
        assert_eq!(zoned_date_time_round_unit("microsecond"), Some((1_000, 1_000)));
        assert_eq!(zoned_date_time_round_unit("nanosecond"), Some((1, 1_000)));
        // year/month/week 及拼写错误不在单位表。
        assert_eq!(zoned_date_time_round_unit("years"), None);
        assert_eq!(zoned_date_time_round_unit("months"), None);
        assert_eq!(zoned_date_time_round_unit("weeks"), None);
        assert_eq!(zoned_date_time_round_unit("hourz"), None);
    }

    #[test]
    fn zoned_date_time_round_else_path_epoch() {
        // 217175010123456789n +01:00：本地 time_ns = 55_410_123_456_789。
        // hour/4 quantum 14_400e9 → rounded_time 57_600e9 → local_to_epoch_ns 回推 217177200000000000。
        let quantum = 3_600_000_000_000 * 4;
        let rounded = round_instant_ns(55_410_123_456_789, quantum, InstantRoundingMode::HalfExpand).unwrap();
        assert_eq!(rounded, 57_600_000_000_000);
        // 本地墙钟日回推：offset 60 分，local_to_epoch_ns 得目标 epoch。
        let epoch = local_to_epoch_ns(1976, 11, 18, rounded as f64, 60).unwrap();
        assert_eq!(epoch, 217_177_200_000_000_000);
    }

    #[test]
    fn zoned_date_time_round_day_path_epoch() {
        // 同日本地午夜 startNs（2513 天，offset 60 分），dayProgress 舍到次日 → 217206000000000000。
        let start_ns = start_of_day_epoch_ns(1976, 11, 18, 60).unwrap();
        assert_eq!(start_ns, 217_119_600_000_000_000);
        let day_progress = 217_175_010_123_456_789 - start_ns;
        let rounded = round_instant_ns(day_progress, 86_400_000_000_000, InstantRoundingMode::HalfExpand).unwrap();
        assert_eq!(rounded, 86_400_000_000_000);
        assert_eq!(start_ns + rounded, 217_206_000_000_000_000);
    }

    #[test]
    fn format_plain_time_iso_seconds_and_fractions() {
        // 无小数（亚秒为 0）：仅 HH:MM:SS。
        assert_eq!(format_plain_time_iso(45_296_000_000_000, true, None), "12:34:56");
        // 固定小数位补足 9 位。
        assert_eq!(format_plain_time_iso(45_296_987_654_321, true, Some(9)), "12:34:56.987654321");
        // 固定 3 位截断亚秒。
        assert_eq!(format_plain_time_iso(45_296_987_654_321, true, Some(3)), "12:34:56.987");
        // 固定 0 位不输出小数。
        assert_eq!(format_plain_time_iso(45_296_987_654_321, true, Some(0)), "12:34:56");
        // 自动模式去尾零（.500 归一为 .5，整秒无小数）。
        assert_eq!(format_plain_time_iso(45_296_500_000_000, true, None), "12:34:56.5");
        assert_eq!(format_plain_time_iso(45_296_000_000_000, true, None), "12:34:56");
        // 不含秒：仅 HH:MM。
        assert_eq!(format_plain_time_iso(45_296_000_000_000, false, Some(0)), "12:34");
        assert_eq!(format_plain_time_iso(45_296_987_654_321, false, None), "12:34");
    }

    #[test]
    fn plain_time_rounding_cross_midnight() {
        // 23:59:59.9 以 second 舍入（halfExpand）→ 24:00:00 → 取模回 00:00:00。
        let total_ns = 86_399_900_000_000_i128;
        let quantum = 1_000_000_000_i128;
        let rounded = round_instant_ns(total_ns, quantum, InstantRoundingMode::HalfExpand).unwrap();
        assert_eq!(rounded, 86_400_000_000_000);
        const DAY_NS: i128 = 86_400_000_000_000;
        assert_eq!(rounded.rem_euclid(DAY_NS), 0);
        assert_eq!(format_plain_time_iso(rounded.rem_euclid(DAY_NS), true, Some(0)), "00:00:00");
    }

    #[test]
    fn plain_time_round_increment_real_factor() {
        // 真因子校验：增量须严格小于最大增量且能整除（MaximumTemporalDurationRoundingIncrement）。
        // hour 合法增量：[1,2,3,4,6,8,12]，24（= 最大增量）与 11（不整除）均拒。
        let (_, max_increment) = plain_time_round_unit("hour").unwrap();
        for increment in [1, 2, 3, 4, 6, 8, 12] {
            assert!(increment < max_increment && max_increment % increment == 0, "hour {increment} 应合法");
        }
        for increment in [11, 24] {
            assert!(increment >= max_increment || max_increment % increment != 0, "hour {increment} 应拒绝");
        }
        // minute 合法增量：整除 60 且严格小于 60；60（= 最大增量）与 29（不整除）均拒。
        let (_, max_increment) = plain_time_round_unit("minute").unwrap();
        for increment in [1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30] {
            assert!(increment < max_increment && max_increment % increment == 0, "minute {increment} 应合法");
        }
        for increment in [29, 60] {
            assert!(increment >= max_increment || max_increment % increment != 0, "minute {increment} 应拒绝");
        }
        // millisecond 最大增量 1000：1000 拒，29 拒。
        let (_, max_increment) = plain_time_round_unit("millisecond").unwrap();
        assert!(max_increment == 1000);
        for increment in [29, 1000] {
            assert!(increment >= max_increment || max_increment % increment != 0, "ms {increment} 应拒绝");
        }
    }

    #[test]
    fn plain_time_apply_duration_ignores_date_units() {
        // 时间域加总仅取 hours 起字段；days 及以上对 PlainTime 忽略（与 instant_round 日期单位报错不同）。
        let mut values = [0.0; 10];
        values[3] = 5.0; // days
        values[4] = 2.0; // hours
        values[5] = 30.0; // minutes
        let time_delta = duration_component_integer(values[4]).unwrap() * 3_600_000_000_000
            + duration_component_integer(values[5]).unwrap() * 60_000_000_000;
        assert_eq!(time_delta, 9_000_000_000_000); // 2h30m
                                                   // 忽略 days：time_delta 不含 DAY_NS 分量。
        assert_eq!(time_delta % 86_400_000_000_000, 9_000_000_000_000);
    }
}
