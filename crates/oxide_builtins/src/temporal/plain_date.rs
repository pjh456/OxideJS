//! Temporal.PlainDate：ISO 日期解析与校验、构造、getter、日历属性与加减/差值。

use chrono::{Datelike, Days, NaiveDate};
use oxide_runtime_api::{to_string, NativeResult, ProtoKind, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::common::native_try;
use super::common::{
    civil_from_days, days_from_civil, days_in_month, ensure_plain_date, get_calendar_id, get_double_prop,
    initialize_temporal_receiver, is_ctor_call, parse_plain_date_string, parse_round_options,
    plain_date_time_object_parts, receiver_obj, reject_partial_object_with_calendar_or_time_zone,
    resolve_date_time_parts, temporal_calendar_id, temporal_calendar_id_strict, temporal_number_component,
    temporal_option_string, temporal_option_value, temporal_overflow, zoned_date_time_plain_parts, DateTimeParts,
};
use super::difference::{difference_core, parse_difference_settings, IsoDateTime};
use super::duration::duration_like_values;
use super::{
    make_plain_date, make_plain_date_time, make_plain_month_day, make_plain_year_month, plain_time_like_ns,
    round_instant_ns,
};

/// 校验 ISO 日期分量（month 1-12、day 按月份与闰年，不依赖 chrono 年份范围）。
pub(crate) fn valid_iso_date(year: i32, month: u32, day: u32) -> bool {
    match days_in_month(i128::from(year), i128::from(month)) {
        Some(days) => day != 0 && day as i128 <= days,
        None => false,
    }
}

/// 计算 ISO 日期时间的纪元纳秒；分量须已通过 `valid_iso_date`。
fn iso_date_time_epoch_ns(year: i32, month: u32, day: u32, time_ns: f64) -> Option<i128> {
    let days = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    days.checked_mul(86_400_000_000_000)?.checked_add(time_ns as i128)
}

/// 校验日期时间落在 PlainDateTime 表示范围（约 ±(10^8 + 1) 天，边界互斥）。
pub(crate) fn valid_plain_date_time_range(year: i32, month: u32, day: u32, time_ns: f64) -> bool {
    const MAX_ISO_DAY: i128 = 100_000_000;
    let limit = (MAX_ISO_DAY + 1) * 86_400_000_000_000;
    match iso_date_time_epoch_ns(year, month, day, time_ns) {
        Some(epoch_ns) => epoch_ns > -limit && epoch_ns < limit,
        None => false,
    }
}

/// 读两位数字（basic 格式的月/日等紧凑分量）。
pub(crate) fn read_iso2(t: &str, i: &mut usize, bytes: &[u8]) -> Result<u32, String> {
    if *i + 2 > bytes.len() {
        return Err("truncated ISO component".into());
    }
    let two = &t[*i..*i + 2];
    *i += 2;
    two.parse().map_err(|_| "invalid ISO component".to_string())
}

/// 从 Temporal ISO 字符串中提取日期分量 `(year, month, day)`。
///
/// # 支持
/// - extended `YYYY-MM-DD` 与 basic `YYYYMMDD`，可带 `+`/`-` 符号（扩展年）。
/// - 时间部分（`T`/`t` 起）与 offset（`Z`/`±HH:MM`）与 annotations 允许存在，
///   日期提取时忽略（日历选择由调用方处理）。
///
/// # 边界
/// - 空串、年份不足 4 位、月/日超界、零年（含 `-000000` 减零年）均拒绝。
/// - 日期后的残余内容须以 `T`/`t`、offset、`[` 开头，否则拒绝。
pub(crate) fn parse_iso_date(s: &str) -> Result<(i32, u32, u32), String> {
    let t = s.trim().replace('\u{2212}', "-");
    if t.is_empty() {
        return Err("invalid ISO string".into());
    }
    let bytes = t.as_bytes();
    let mut i = 0usize;
    let signed_year = i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+');
    let negative = if signed_year {
        let neg = bytes[i] == b'-';
        i += 1;
        neg
    } else {
        false
    };
    let y_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let digits = &t[y_start..i];
    // basic 格式（无分隔符，年月日连续）：8-10 位数字，前 4-6 位年，后 4 位月日。
    // 否则纯年（4-6 位），月/日由 `-MM-DD` 接续。
    let (year_digits, month_day) = if digits.len() >= 8 {
        if !(8..=10).contains(&digits.len()) {
            return Err("invalid ISO date".into());
        }
        let y_len = digits.len() - 4;
        if !(4..=6).contains(&y_len) {
            return Err("invalid ISO year".into());
        }
        (&digits[..y_len], Some(&digits[y_len..]))
    } else {
        (digits, None)
    };
    let expected_year_digits = if signed_year { 6 } else { 4 };
    if year_digits.len() != expected_year_digits {
        return Err("invalid ISO year".into());
    }
    let mut year: i64 = year_digits.parse().map_err(|_| "invalid ISO year".to_string())?;
    if negative {
        year = -year;
    }
    let (month, day) = if let Some(md) = month_day {
        if md.len() != 4 {
            return Err("invalid ISO date".into());
        }
        let m: u32 = md[..2].parse().map_err(|_| "invalid ISO month".to_string())?;
        let d: u32 = md[2..].parse().map_err(|_| "invalid ISO day".to_string())?;
        (m, d)
    } else if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
        let m = read_iso2(&t, &mut i, bytes)?;
        if i >= bytes.len() || bytes[i] != b'-' {
            return Err("invalid ISO date".into());
        }
        i += 1;
        let d = read_iso2(&t, &mut i, bytes)?;
        (m, d)
    } else {
        return Err("missing month/day".into());
    };
    let rest = &t[i..];
    if !rest.is_empty() {
        let ok = rest.starts_with(['T', 't', '[']);
        if !ok {
            return Err("invalid trailing content".into());
        }
    }
    if month == 0 || month > 12 || day == 0 {
        return Err("invalid ISO date".into());
    }
    if negative && year == 0 {
        return Err("invalid ISO negative zero year".into());
    }
    Ok((year as i32, month, day))
}

/// `Temporal.PlainDate` 构造器：`new PlainDate(year, month, day)`。
pub fn plain_date_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.builtin_proto(ProtoKind::PlainDateProto);
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.PlainDate cannot be invoked without 'new'",
        ));
    }
    // 分量转换序：year → month → day；缺参归 undefined 经 number-only 路径
    // 统一落 RangeError（日期分量无缺省值，缺参不是合法分量）。
    let year_raw = native_try!(temporal_number_component(
        vm,
        if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() },
    ));
    let month_raw = native_try!(temporal_number_component(
        vm,
        if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() },
    ));
    let day_raw = native_try!(temporal_number_component(
        vm,
        if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() },
    ));
    // f64 截断后可能超出目标整型范围，先做有界转换。
    let year = if year_raw >= i32::MIN as f64 && year_raw <= i32::MAX as f64 {
        year_raw as i32
    } else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid year"));
    };
    let mut to_u32 = |v: f64| -> Result<u32, JsValue> {
        if v >= 0.0 && v <= u32::MAX as f64 {
            Ok(v as u32)
        } else {
            Err(crate::error::create_range_error(vm, "invalid date component"))
        }
    };
    let month = native_try!(to_u32(month_raw));
    let day = native_try!(to_u32(day_raw));
    if !valid_iso_date(year, month, day) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    // 表示范围（ISODateWithinLimits）：-271821-04-19 … +275760-09-13。
    let day_count = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    let calendar = if args.len() > 4 && !vm.reg(args[4]).is_undefined() {
        native_try!(temporal_calendar_id_strict(vm, vm.reg(args[4]))).unwrap_or_else(|| "iso8601".to_string())
    } else {
        "iso8601".to_string()
    };
    let calendar_value = vm.new_string(&calendar);
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_PLAIN_DATE,
        [
            JsValue::float(year as f64),
            JsValue::float(month as f64),
            JsValue::float(day as f64),
            calendar_value,
        ],
    )
}

/// `Temporal.PlainDate.from(value, options)`：接受 ISO 日期字符串或 `{year, month, day}` 对象。
pub fn plain_date_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (year, month, day, calendar) = if val.is_string() {
        // 规范顺序：先 ParseTemporalDateString，再 ToTemporalOverflow(options)。
        let ymd = match parse_plain_date_string(&to_string(val)) {
            Ok(ymd) => ymd,
            Err(_) => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO 8601 date"));
            }
        };
        native_try!(temporal_overflow(vm, args));
        (ymd.0, ymd.1, ymd.2, None)
    } else {
        let constrain = native_try!(temporal_overflow(vm, args));
        match object_date_ymd(vm, val, constrain) {
            Ok(ymd) => ymd,
            Err(error) => return NativeResult::Err(error),
        }
    };
    if !valid_iso_date(year, month, day) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    // 表示范围（ISODateWithinLimits）：-271821-04-19 … +275760-09-13。
    let day_count = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    make_plain_date(vm, year, month, day, calendar.as_deref().unwrap_or("iso8601"))
}

fn plain_date_ymd<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(f64, f64, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_date(vm, obj)?;
    Ok((get_double_prop(obj, 0), get_double_prop(obj, 1), get_double_prop(obj, 2)))
}

/// `Temporal.PlainDate.prototype.year` getter。
pub fn plain_date_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (y, _, _) = native_try!(plain_date_ymd(vm, args));
    NativeResult::Ok(JsValue::float(y))
}

/// `Temporal.PlainDate.prototype.month` getter。
pub fn plain_date_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, m, _) = native_try!(plain_date_ymd(vm, args));
    NativeResult::Ok(JsValue::float(m))
}

/// `Temporal.PlainDate.prototype.day` getter。
pub fn plain_date_day<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, _, d) = native_try!(plain_date_ymd(vm, args));
    NativeResult::Ok(JsValue::float(d))
}

/// `Temporal.PlainDate.prototype.toString()`：输出 `YYYY-MM-DD`。
pub fn plain_date_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date(vm, obj));
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as u32;
    let day = get_double_prop(obj, 2) as u32;
    NativeResult::Ok(vm.new_string(&format!("{year:04}-{month:02}-{day:02}")))
}

// ───────────────────── PlainDate 扩展方法 ─────────────────────

/// 从 PlainDate receiver 读 year/month/day 并构造 chrono NaiveDate（供日期计算）。
fn plain_date_naive<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<NaiveDate, JsValue> {
    let (y, m, d) = plain_date_ymd(vm, args)?;
    NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32)
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid date"))
}

/// 从 PlainDate receiver 读 year/month/day 并按 ISO 范围校验，不依赖 chrono 年份范围。
fn plain_date_ymd_checked<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(i32, u32, u32), JsValue> {
    let (y, m, d) = plain_date_ymd(vm, args)?;
    if y.is_nan() || m.is_nan() || d.is_nan() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let ymd = (y.trunc() as i32, m.trunc() as u32, d.trunc() as u32);
    if !valid_iso_date(ymd.0, ymd.1, ymd.2) {
        return Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    // 表示范围（PlainDate）：-271821-04-19（第 -100,000,001 天）… +275760-09-13（第 +100,000,000 天），
    // 超出 chrono NaiveDate 年份范围（约 ±26 万年）的日期也在此被拒绝而非后续 panic。
    let day_count = days_from_civil(i128::from(ymd.0), i128::from(ymd.1), i128::from(ymd.2));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    Ok(ymd)
}

/// 从对象式日期字段（`{year, month, day}`）读三字段与日历；PlainDate/PlainDateTime 实例
/// 直接读内部槽；缺字段返回 None。
fn object_date_ymd<H: VmHost>(
    vm: &mut H, val: JsValue, constrain: bool,
) -> Result<(i32, u32, u32, Option<String>), JsValue> {
    if !val.is_object() {
        return Err(crate::error::create_type_error(vm, "cannot convert to PlainDate"));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "cannot convert to PlainDate"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_plain_date_time_obj() {
        return Ok((
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
            Some(get_calendar_id(obj, 4)),
        ));
    }
    if obj.is_plain_date_obj() {
        return Ok((
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
            Some(get_calendar_id(obj, 3)),
        ));
    }
    if obj.is_zoned_date_time_obj() {
        let (year, month, day, _) = zoned_date_time_plain_parts(vm, obj)?;
        return Ok((year, month, day, Some(get_calendar_id(obj, 2))));
    }
    let (year, month, day, _, calendar) = plain_date_time_object_parts(vm, val, obj, constrain, true)?;
    Ok((year, month, day, calendar))
}

fn date_like_ymd<H: VmHost>(vm: &mut H, val: JsValue) -> Result<(i32, u32, u32, Option<String>), JsValue> {
    let ymd = if val.is_string() {
        let (y, m, d) = parse_plain_date_string(&to_string(val))
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 date"))?;
        (y, m, d, None)
    } else {
        object_date_ymd(vm, val, true)?
    };
    if !valid_iso_date(ymd.0, ymd.1, ymd.2) {
        return Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    // 表示范围（PlainDate）：-271821-04-19（第 -100,000,001 天）… +275760-09-13（第 +100,000,000 天），
    // 超出 chrono NaiveDate 年份范围（约 ±26 万年）的日期也在此被拒绝而非后续 panic。
    let day_count = days_from_civil(i128::from(ymd.0), i128::from(ymd.1), i128::from(ymd.2));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    Ok(ymd)
}

/// `Temporal.PlainDate.prototype.dayOfWeek` getter：ISO 周几（周一 1 … 周日 7）。
pub fn plain_date_day_of_week<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = match plain_date_naive(vm, args) {
        Ok(d) => d,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::float((date.weekday().num_days_from_monday() + 1) as f64))
}

/// `Temporal.PlainDate.prototype.dayOfYear` getter：年内第几天（1 起）。
pub fn plain_date_day_of_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = match plain_date_naive(vm, args) {
        Ok(d) => d,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::float(date.ordinal() as f64))
}

/// 计算给定年月的天数（含闰年）。
pub(crate) fn days_in_month_iso(year: i32, month: u32) -> u32 {
    if let Some(next) = NaiveDate::from_ymd_opt(year, month + 1, 1) {
        if let Some(prev) = next.checked_sub_days(Days::new(1)) {
            return prev.day();
        }
    }
    // 年越 chrono 表示范围（PMD from 的 bag year 可任意 i32）：直接按 ISO 规则。
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            // 闰年判定须用数学取模（负年），与 chrono 域内结果一致。
            if year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0) {
                29
            } else {
                28
            }
        }
    }
}

/// `Temporal.PlainDate.prototype.daysInMonth` getter。
pub fn plain_date_days_in_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = match plain_date_naive(vm, args) {
        Ok(d) => d,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::float(days_in_month_iso(date.year(), date.month()) as f64))
}

/// `Temporal.PlainDate.prototype.daysInWeek` getter：恒 7。
pub fn plain_date_days_in_week<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_ymd(vm, args));
    NativeResult::Ok(JsValue::float(7.0))
}

pub(crate) fn is_leap_year_iso(year: i32) -> bool {
    NaiveDate::from_ymd_opt(year, 2, 29).is_some()
}

/// `Temporal.PlainDate.prototype.daysInYear` getter：366（闰年）或 365。
pub fn plain_date_days_in_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = match plain_date_naive(vm, args) {
        Ok(d) => d,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::float(if is_leap_year_iso(date.year()) { 366.0 } else { 365.0 }))
}

/// `Temporal.PlainDate.prototype.monthsInYear` getter：恒 12（ISO 日历）。
pub fn plain_date_months_in_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_ymd(vm, args));
    NativeResult::Ok(JsValue::float(12.0))
}

/// `Temporal.PlainDate.prototype.inLeapYear` getter。
pub fn plain_date_in_leap_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = match plain_date_naive(vm, args) {
        Ok(d) => d,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::bool(is_leap_year_iso(date.year())))
}

/// `Temporal.PlainDate.prototype.weekOfYear` getter：ISO 周数。
pub fn plain_date_week_of_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = match plain_date_naive(vm, args) {
        Ok(d) => d,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::float(date.iso_week().week() as f64))
}

/// `Temporal.PlainDate.prototype.yearOfWeek` getter：ISO 周所属年。
pub fn plain_date_year_of_week<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let date = match plain_date_naive(vm, args) {
        Ok(d) => d,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::float(date.iso_week().year() as f64))
}

/// `Temporal.PlainDate.prototype.monthCode` getter：`M01`..`M12`。
pub fn plain_date_month_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, m, _) = match plain_date_ymd(vm, args) {
        Ok(x) => x,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(vm.new_string(&format!("M{m:02}")))
}

/// `Temporal.PlainDate.prototype.era` getter：ISO 日历无纪元，恒 undefined。
pub fn plain_date_era<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_ymd(vm, args));
    NativeResult::Ok(JsValue::undefined())
}

/// `Temporal.PlainDate.prototype.eraYear` getter：ISO 日历无纪元，恒 undefined。
pub fn plain_date_era_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_ymd(vm, args));
    NativeResult::Ok(JsValue::undefined())
}

/// `Temporal.PlainDate.prototype.calendarId`：读日历槽，兜底 ISO 8601。
pub fn plain_date_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date(vm, obj));
    NativeResult::Ok(vm.new_string(&get_calendar_id(obj, 3)))
}

/// `Temporal.PlainDate.prototype.equals(other)`：比较年月日是否相等。
pub fn plain_date_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (y, m, d) = match plain_date_ymd(vm, args) {
        Ok(x) => x,
        Err(e) => return NativeResult::Err(e),
    };
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let other = match date_like_ymd(vm, other_val) {
        Ok(x) => x,
        Err(e) => return NativeResult::Err(e),
    };
    let equal = other.0 as f64 == y && other.1 as f64 == m && other.2 as f64 == d;
    NativeResult::Ok(JsValue::bool(equal))
}

/// `Temporal.PlainDate.prototype.with(temporalDateLike[, options])`：覆盖 year/month/day/monthCode；
/// undefined 字段不覆盖；calendar/timeZone 键与 Temporal 实例、字符串参数均抛 TypeError；
/// 四键全 undefined 抛 TypeError；overflow 读自 options（缺省 constrain）。
/// 结果保留 receiver 日历。
///
/// # 步骤
/// 1. receiver 品牌校验；partial 参数经 reject_partial_object_with_calendar_or_time_zone
///    排除日历/时区载体（其内部按序读 calendar、timeZone 两键）。
/// 2. 按规范读序逐字段读并就地转换：day → month → monthCode → year（数值字段
///    ToNumber，monthCode ToString），转换错误先于 options 校验抛出。
/// 3. month/day 小于 1 抛 RangeError（先于 options 形态错误）。
/// 4. 读 overflow；monthCode 语义校验（L 后缀、超界、与 month 冲突）在 options 之后。
/// 5. 与 receiver 字段合并后经 resolve_date_time_parts 钳制/校验；显式 day count
///    范围检查（ignore_time 路径不查 ISODateWithinLimits）。
pub fn plain_date_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date(vm, obj));
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    native_try!(reject_partial_object_with_calendar_or_time_zone(vm, value));
    let vptr = value.as_js_object_ptr();
    if vptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let vobj = unsafe { &*vptr };
    // 读序由规范固定：day → month → monthCode → year，每字段读后立即转换。
    let day_raw = native_try!(temporal_option_value(vm, vobj, value, "day"));
    let day = if day_raw.is_undefined() {
        None
    } else {
        Some(native_try!(temporal_number_component(vm, day_raw)))
    };
    let month_raw = native_try!(temporal_option_value(vm, vobj, value, "month"));
    let month = if month_raw.is_undefined() {
        None
    } else {
        Some(native_try!(temporal_number_component(vm, month_raw)))
    };
    let month_code_raw = native_try!(temporal_option_value(vm, vobj, value, "monthCode"));
    let month_code = if month_code_raw.is_undefined() {
        None
    } else {
        Some(native_try!(temporal_option_string(vm, month_code_raw)))
    };
    let year_raw = native_try!(temporal_option_value(vm, vobj, value, "year"));
    let year = if year_raw.is_undefined() {
        None
    } else {
        Some(native_try!(temporal_number_component(vm, year_raw)))
    };
    if day.is_none() && month.is_none() && month_code.is_none() && year.is_none() {
        return NativeResult::Err(crate::error::create_type_error(vm, "no properties present"));
    }
    // 月/日小于 1 的 RangeError 先于 options 校验抛出。
    if let Some(month) = month {
        if month < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
        }
    }
    if let Some(day) = day {
        if day < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
        }
    }
    let constrain = native_try!(temporal_overflow(vm, args));
    // 与 receiver 字段合并：bag 字段优先，undefined 字段回落 receiver 槽值。
    // receiver 月只在 bag 未提供 month 与 monthCode 时参与合并，避免与
    // bag 的 monthCode 触发误冲突（冲突检查仅作用于 bag 字段）。
    let month = if month.is_none() && month_code.is_none() {
        Some(get_double_prop(obj, 1))
    } else {
        month
    };
    let (year, month, day, _, _) = native_try!(resolve_date_time_parts(
        vm,
        None,
        DateTimeParts {
            year: year.or(Some(get_double_prop(obj, 0))),
            month,
            month_code,
            day: day.or(Some(get_double_prop(obj, 2))),
            hour: 0.0,
            minute: 0.0,
            second: 0.0,
            millisecond: 0.0,
            microsecond: 0.0,
            nanosecond: 0.0,
        },
        constrain,
        true,
    ));
    // ignore_time 路径不查 ISODateWithinLimits，day count 表示范围须显式检查。
    let day_count = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    make_plain_date(vm, year, month, day, &get_calendar_id(obj, 3))
}

/// `Temporal.PlainDate.prototype.withCalendar(calendar)`：只换日历槽，日期分量不变。
///
/// # 步骤
/// 1. receiver 品牌校验；缺参 / undefined 抛 TypeError。
/// 2. 参数经 temporal_calendar_id（宽松版）：字符串走白名单或 ISO 日期(-时间)串，
///    六类 Temporal 对象直读日历槽（不触发 getter），其他对象 / 原始值抛 TypeError。
/// 3. 读 receiver 槽 0-2 重建，日历槽替换。
///
/// # 边界与前提
/// - 非法日历串 → RangeError；日历 ID 大小写折叠。
pub fn plain_date_with_calendar<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date(vm, obj));
    let calendar_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if calendar_value.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "calendar is required"));
    }
    let calendar = native_try!(temporal_calendar_id(vm, calendar_value)).unwrap_or_else(|| "iso8601".to_string());
    make_plain_date(
        vm,
        get_double_prop(obj, 0) as i32,
        get_double_prop(obj, 1) as u32,
        get_double_prop(obj, 2) as u32,
        &calendar,
    )
}

/// `Temporal.PlainDate.prototype.toPlainDateTime(timeLike)`：日期分量 + 时间槽组合。
///
/// # 步骤
/// 1. receiver 品牌校验；读槽 0-2 与日历槽 3 入本地（plain_time_like_ns 的 bag
///    路径有用户调用窗口，receiver 槽须先于窗口读入）。
/// 2. plain_time_like_ns 取当日纳秒（undefined → 午夜）。
/// 3. valid_plain_date_time_range 严格不等式越界抛 RangeError；make_plain_date_time 重建。
///
/// # 边界与前提
/// - 组合越 PlainDateTime 表示范围（min 午夜抛、min 加 1 纳秒过、max 全满过）→ RangeError。
/// - 日历槽继承 receiver。
pub fn plain_date_to_plain_date_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date(vm, obj));
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as u32;
    let day = get_double_prop(obj, 2) as u32;
    let calendar = get_calendar_id(obj, 3);
    let time_like = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let time_ns = native_try!(plain_time_like_ns(vm, time_like));
    if !valid_plain_date_time_range(year, month, day, time_ns) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time"));
    }
    make_plain_date_time(vm, year, month, day, time_ns, &calendar)
}

/// `Temporal.PlainDate.prototype.toPlainMonthDay()`：纯拷贝月/日，参考年固定 1972。
pub fn plain_date_to_plain_month_day<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date(vm, obj));
    make_plain_month_day(
        vm,
        get_double_prop(obj, 1) as u32,
        get_double_prop(obj, 2) as u32,
        1972,
        &get_calendar_id(obj, 3),
    )
}

/// `Temporal.PlainDate.prototype.toPlainYearMonth()`：纯拷贝年/月，参考日固定 1。
pub fn plain_date_to_plain_year_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date(vm, obj));
    make_plain_year_month(
        vm,
        get_double_prop(obj, 0) as i32,
        get_double_prop(obj, 1) as u32,
        1,
        &get_calendar_id(obj, 3),
    )
}

/// `Temporal.PlainDate.compare(a, b)`：静态比较，返回 -1/0/1。
pub fn plain_date_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let a_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let b_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let a = match date_like_ymd(vm, a_val) {
        Ok(x) => x,
        Err(e) => return NativeResult::Err(e),
    };
    let b = match date_like_ymd(vm, b_val) {
        Ok(x) => x,
        Err(e) => return NativeResult::Err(e),
    };
    let cmp = (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2));
    NativeResult::Ok(JsValue::float(match cmp {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

/// `Temporal.PlainDate.prototype.valueOf()`：Temporal 对象禁止转原始值。
pub fn plain_date_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.PlainDate has no valueOf"))
}

/// `Temporal.PlainDate.prototype.toJSON()`：输出 ISO 日期串（同 toString）。
pub fn plain_date_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(e) => return NativeResult::Err(e),
    };
    let obj = unsafe { &*ptr };
    if let Err(e) = ensure_plain_date(vm, obj) {
        return NativeResult::Err(e);
    }
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as u32;
    let day = get_double_prop(obj, 2) as u32;
    NativeResult::Ok(vm.new_string(&format!("{year:04}-{month:02}-{day:02}")))
}

/// 按 duration-like 日期字段对日期做加减（Temporal 大单位运算，月份不足日时取月末）。
fn date_apply_duration<H: VmHost>(vm: &mut H, args: &[u8], sign: i64) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    let date = match plain_date_naive(vm, args) {
        Ok(d) => d,
        Err(e) => return NativeResult::Err(e),
    };
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = match duration_like_values(vm, val) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
    let [y, m, w, d, h, min, s, ms, us, ns] = values;
    let mut result = date;
    let total_months = ((y * 12.0 + m) * sign as f64) as i64;
    if total_months != 0 {
        if let Some(nd) = add_signed_months(result, total_months) {
            result = nd;
        } else {
            return NativeResult::Err(crate::error::create_range_error(vm, "date out of range"));
        }
    }
    let time_days = (h / 24.0
        + min / 1_440.0
        + s / 86_400.0
        + ms / 86_400_000.0
        + us / 86_400_000_000.0
        + ns / 86_400_000_000_000.0)
        .trunc();
    let total_days = ((w * 7.0 + d + time_days) * sign as f64) as i64;
    if total_days != 0 {
        let next = if total_days >= 0 {
            result.checked_add_days(Days::new(total_days as u64))
        } else {
            result.checked_sub_days(Days::new(total_days.unsigned_abs()))
        };
        if let Some(nd) = next {
            result = nd;
        } else {
            return NativeResult::Err(crate::error::create_range_error(vm, "date out of range"));
        }
    }
    make_plain_date(vm, result.year(), result.month(), result.day(), &get_calendar_id(obj, 3))
}

/// `Temporal.PlainDate.prototype.add(durationLike)`。
pub fn plain_date_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_apply_duration(vm, args, 1)
}

/// `Temporal.PlainDate.prototype.subtract(durationLike)`。
pub fn plain_date_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_apply_duration(vm, args, -1)
}

/// PlainDate round 的最小单位表：year/month/week/day。
/// 返回 (单位日数, 最大增量)；日历单位无整除约束（最大增量 0）。
pub(crate) fn plain_date_round_unit(value: &str) -> Option<(i128, i128)> {
    match value {
        "year" | "years" => Some((0, 0)),
        "month" | "months" => Some((0, 0)),
        "week" | "weeks" => Some((7, 0)),
        "day" | "days" => Some((1, 0)),
        _ => None,
    }
}

/// `Temporal.PlainDate.prototype.round(roundTo)`：日历分量平衡到最小单位。
///
/// # 步骤
/// 1. receiver 品牌校验；读槽 0-3 入本地（日历槽继承 receiver）。
/// 2. roundTo 经 parse_round_options（undefined → TypeError；字符串简写；对象固定读序）。
/// 3. 单位表查 smallestUnit（year/month/week/day；None → RangeError）；缺失 → RangeError。
/// 4. day/week：日数按 increment × 单位日数舍入；month/year：总月数/年数按 increment 舍入。
/// 5. month → 日钳制到目标月末；year → 月/日钳制到 1/1。
/// 6. valid_iso_date 加 ISODateWithinLimits（±10^8 天，下界 -10^8-1）越界抛 RangeError。
/// 7. make_plain_date 重建（日历槽继承 receiver）。
///
/// # 边界与前提
/// - 日历单位（year/month/week）的 increment 无真因子约束。
/// - 日数换算走 days_from_civil/civil_from_days（proleptic Gregorian，精确）。
pub fn plain_date_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_date(vm, obj));
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as u32;
    let day = get_double_prop(obj, 2) as u32;
    let calendar = get_calendar_id(obj, 3);

    let round_to = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (increment, mode, unit_value) = native_try!(parse_round_options(vm, round_to));
    let Some(unit_value) = unit_value else {
        return NativeResult::Err(crate::error::create_range_error(vm, "smallestUnit is required"));
    };
    let (unit_days, max_increment) = match plain_date_round_unit(&unit_value) {
        Some(entry) => entry,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid smallest unit")),
    };
    if max_increment != 0 && (increment >= max_increment || max_increment % increment != 0) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }

    let (year, month, day) = match unit_value.as_str() {
        "day" | "days" | "week" | "weeks" => {
            let days = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
            let Some(quantum) = unit_days.checked_mul(increment) else {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
            };
            let Some(rounded_days) = round_instant_ns(days, quantum, mode) else {
                return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
            };
            let (year, month, day) = civil_from_days(rounded_days);
            (year as i32, month as u32, day as u32)
        }
        "month" | "months" => {
            let total_months = i128::from(year) * 12 + i128::from(month) - 1;
            let Some(rounded_months) = round_instant_ns(total_months, increment, mode) else {
                return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
            };
            let year = rounded_months.div_euclid(12);
            let month = rounded_months.rem_euclid(12) + 1;
            let day = days_in_month(year, month).unwrap_or(31);
            (year as i32, month as u32, day as u32)
        }
        "year" | "years" => {
            let Some(rounded_year) = round_instant_ns(i128::from(year), increment, mode) else {
                return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
            };
            (rounded_year as i32, 1, 1)
        }
        _ => unreachable!(),
    };
    if !valid_iso_date(year, month, day) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    // 表示范围（ISODateWithinLimits）：-271821-04-19 … +275760-09-13。
    let day_count = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    make_plain_date(vm, year, month, day, &calendar)
}

fn add_signed_months(date: NaiveDate, months: i64) -> Option<NaiveDate> {
    let total = date.year() as i64 * 12 + date.month0() as i64 + months;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) as u32 + 1;
    let year = i32::try_from(year).ok()?;
    let day = date.day().min(days_in_month_iso(year, month));
    NaiveDate::from_ymd_opt(year, month, day)
}

/// `Temporal.PlainDate.prototype.until(other, options)`：date-only 单位
/// （year/month/week/day）差值 + 舍入语义。
pub fn plain_date_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let start = match plain_date_ymd_checked(vm, args) {
        Ok(date) => date,
        Err(error) => return NativeResult::Err(error),
    };
    let other_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let other = match date_like_ymd(vm, other_value) {
        Ok(value) => value,
        Err(error) => return NativeResult::Err(error),
    };
    let options_value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let settings = match parse_difference_settings(vm, options_value, true, 3) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    difference_core(
        vm,
        IsoDateTime {
            date: (i128::from(start.0), i128::from(start.1), i128::from(start.2)),
            time_ns: 0,
        },
        IsoDateTime {
            date: (i128::from(other.0), i128::from(other.1), i128::from(other.2)),
            time_ns: 0,
        },
        settings,
        false,
        9,
    )
}

/// `Temporal.PlainDate.prototype.since(other, options)`：date-only 单位差值 + 舍入。
pub fn plain_date_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let start = match plain_date_ymd_checked(vm, args) {
        Ok(date) => date,
        Err(error) => return NativeResult::Err(error),
    };
    let other_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let other = match date_like_ymd(vm, other_value) {
        Ok(value) => value,
        Err(error) => return NativeResult::Err(error),
    };
    let options_value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let settings = match parse_difference_settings(vm, options_value, true, 3) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    difference_core(
        vm,
        IsoDateTime {
            date: (i128::from(start.0), i128::from(start.1), i128::from(start.2)),
            time_ns: 0,
        },
        IsoDateTime {
            date: (i128::from(other.0), i128::from(other.1), i128::from(other.2)),
            time_ns: 0,
        },
        settings,
        true,
        9,
    )
}
