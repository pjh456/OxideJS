//! until/since/add/subtract 共享差量层：差值设置解析（GetDifferenceSettings 族）、
//! nudge/舍入（窗口取整、日历单位进位）、ISO 日期差量计算与单位表。

use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::value::JsValue;

use super::common::{
    civil_from_days, days_from_civil, days_in_month, temporal_option_number, temporal_option_string,
    temporal_option_value,
};
use super::{instant_rounding_mode, make_duration, plain_time_components_signed, InstantRoundingMode};

/// 按普通有符号数语义舍入 Instant 差值。
pub(crate) fn round_instant_difference(value: i128, increment: i128, mode: InstantRoundingMode) -> Option<i128> {
    let negative = value < 0;
    let magnitude = value.checked_abs()?;
    let quotient = magnitude / increment;
    let remainder = magnitude % increment;
    if remainder == 0 {
        return Some(value);
    }
    let use_upper = match mode {
        InstantRoundingMode::Ceil => !negative,
        InstantRoundingMode::Expand => true,
        InstantRoundingMode::Floor => negative,
        InstantRoundingMode::Trunc => false,
        _ => match (remainder * 2).cmp(&increment) {
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => match mode {
                InstantRoundingMode::HalfCeil => !negative,
                InstantRoundingMode::HalfEven => quotient.rem_euclid(2) != 0,
                InstantRoundingMode::HalfExpand => true,
                InstantRoundingMode::HalfFloor => negative,
                InstantRoundingMode::HalfTrunc => false,
                _ => unreachable!(),
            },
        },
    };
    let rounded = quotient.checked_add(i128::from(use_upper))?.checked_mul(increment)?;
    Some(if negative { -rounded } else { rounded })
}

pub(crate) fn balance_instant_difference(value: i128, largest_unit: usize) -> Option<[f64; 10]> {
    const SCALES: [i128; 6] = [3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    let negative = value < 0;
    let mut remainder = value.checked_abs()?;
    let mut values = [0.0; 10];
    for (index, scale) in SCALES.iter().enumerate().skip(largest_unit) {
        let component = remainder / scale;
        remainder %= scale;
        if component != 0 {
            values[index + 4] = if negative { -(component as f64) } else { component as f64 };
        }
    }
    Some(values)
}

/// PlainDateTime 差值单位层级：year=0 … nanosecond=9；"auto" 仅限 largestUnit。
pub(crate) fn plain_date_time_unit_index(value: &str) -> Option<usize> {
    match value {
        "year" | "years" => Some(0),
        "month" | "months" => Some(1),
        "week" | "weeks" => Some(2),
        "day" | "days" => Some(3),
        "hour" | "hours" => Some(4),
        "minute" | "minutes" => Some(5),
        "second" | "seconds" => Some(6),
        "millisecond" | "milliseconds" => Some(7),
        "microsecond" | "microseconds" => Some(8),
        "nanosecond" | "nanoseconds" => Some(9),
        _ => None,
    }
}

pub(crate) const DAY_NS: i128 = 86_400_000_000_000;

pub(crate) const MAX_ISO_DAY: i128 = 100_000_000;

/// 比较两个 ISO 日期，返回 -1/0/+1。
pub(crate) fn compare_iso_date(a: (i128, i128, i128), b: (i128, i128, i128)) -> i128 {
    if a.0 != b.0 {
        return if a.0 < b.0 { -1 } else { 1 };
    }
    if a.1 != b.1 {
        return if a.1 < b.1 { -1 } else { 1 };
    }
    if a.2 != b.2 {
        return if a.2 < b.2 { -1 } else { 1 };
    }
    0
}

/// 候选日期（日以 day_override 参与比较）是否在 sign 方向上越过终点。
fn surpasses_with_day(sign: i128, candidate: (i128, i128, i128), day_override: i128, end: (i128, i128, i128)) -> bool {
    let cmp = compare_iso_date((candidate.0, candidate.1, day_override), end);
    if sign > 0 {
        cmp > 0
    } else {
        cmp < 0
    }
}

/// 日期加天数。
pub(crate) fn add_days_iso(date: (i128, i128, i128), days: i128) -> (i128, i128, i128) {
    let (y, m, d) = civil_from_days(days_from_civil(date.0, date.1, date.2) + days);
    (y, m, d)
}

/// ISO8601 日期差分解，对齐 polyfill 的 dateUntil/untilCalendar。
/// largest：0=year 1=month 2=week 3=day。
pub(crate) fn date_until_iso(start: (i128, i128, i128), end: (i128, i128, i128), largest: usize) -> [f64; 10] {
    let mut values = [0.0; 10];
    if largest >= 2 {
        let mut days = days_from_civil(end.0, end.1, end.2) - days_from_civil(start.0, start.1, start.2);
        if largest == 2 {
            values[2] = (days / 7) as f64;
            days %= 7;
        }
        values[3] = days as f64;
        return values;
    }
    let sign = compare_iso_date(end, start);
    if sign == 0 {
        return values;
    }
    let diff_years = end.0 - start.0;
    let diff_days = end.2 - start.2;
    let diff_in_year_sign = if end.1 > start.1 {
        1
    } else if end.1 < start.1 {
        -1
    } else if diff_days > 0 {
        1
    } else if diff_days < 0 {
        -1
    } else {
        0
    };
    // 终点的月-日早于起点的月-日时，年份差需沿 sign 方向修正 1。
    let mut years = if diff_in_year_sign * sign < 0 { diff_years - sign } else { diff_years };
    let mut months = 0_i128;
    if largest == 1 {
        months = years * 12;
        years = 0;
    }
    let intermediate = add_months_i128(start.0, start.1, start.2, years * 12 + months);
    // 闰日校正：intermediate 已越过终点时年份回退 1。
    if surpasses_with_day(sign, intermediate, start.2, end) {
        years -= sign;
    }
    // 从 start 按锚定日逐月推进（月末钳制）；current 始终是未越界的真实日期，
    // 锚定日覆盖只用于越界比较，避免伪日期（如 2 月 29 日）污染天数计算。
    let mut current = add_months_i128(start.0, start.1, start.2, years * 12 + months);
    loop {
        months += sign;
        let candidate = add_months_i128(start.0, start.1, start.2, years * 12 + months);
        let mut cmp = candidate;
        cmp.2 = start.2;
        if surpasses_with_day(sign, cmp, start.2, end) {
            break;
        }
        current = candidate;
    }
    months -= sign;
    let days = days_from_civil(end.0, end.1, end.2) - days_from_civil(current.0, current.1, current.2);
    values[0] = years as f64;
    values[1] = months as f64;
    values[3] = days as f64;
    values
}

/// 向零截断到 increment 的倍数。
fn round_to_increment_trunc(value: i128, increment: i128) -> i128 {
    (value / increment) * increment
}

/// 时长日期部分符号（首个非零分量决定）。
pub(crate) fn date_duration_sign(values: &[f64; 10]) -> i128 {
    for value in values[0..4].iter().copied() {
        if value != 0.0 {
            return if value > 0.0 { 1 } else { -1 };
        }
    }
    0
}

/// 在日期上叠加时长（年/月/周/日）。
pub(crate) fn add_date_duration(date: (i128, i128, i128), values: &[f64; 10]) -> (i128, i128, i128) {
    let years = values[0] as i128;
    let months = values[1] as i128;
    let weeks = values[2] as i128;
    let days = values[3] as i128;
    let d = add_months_i128(date.0, date.1, date.2, years * 12 + months);
    add_days_iso(d, weeks * 7 + days)
}

/// 无符号舍入模式（ApplyUnsignedRoundingMode 用的模式集合）。
#[derive(Clone, Copy)]
enum UnsignedMode {
    Zero,
    Infinity,
    HalfEven,
    HalfInfinity,
    HalfZero,
}

/// ApplyUnsignedRoundingMode：在 r1/r2 间选择（均为非负量）。
fn apply_unsigned_rounding(
    r1: i128, r2: i128, cmp: std::cmp::Ordering, even: bool, mode: InstantRoundingMode, negative: bool,
) -> i128 {
    let unsigned_mode = match mode {
        InstantRoundingMode::Ceil => {
            if negative {
                UnsignedMode::Zero
            } else {
                UnsignedMode::Infinity
            }
        }
        InstantRoundingMode::Floor => {
            if negative {
                UnsignedMode::Infinity
            } else {
                UnsignedMode::Zero
            }
        }
        InstantRoundingMode::Expand => UnsignedMode::Infinity,
        InstantRoundingMode::Trunc => UnsignedMode::Zero,
        InstantRoundingMode::HalfCeil => {
            if negative {
                UnsignedMode::HalfZero
            } else {
                UnsignedMode::HalfInfinity
            }
        }
        InstantRoundingMode::HalfFloor => {
            if negative {
                UnsignedMode::HalfInfinity
            } else {
                UnsignedMode::HalfZero
            }
        }
        InstantRoundingMode::HalfEven => UnsignedMode::HalfEven,
        InstantRoundingMode::HalfExpand => UnsignedMode::HalfInfinity,
        InstantRoundingMode::HalfTrunc => UnsignedMode::HalfZero,
    };
    match unsigned_mode {
        UnsignedMode::Zero => r1,
        UnsignedMode::Infinity => r2,
        UnsignedMode::HalfEven => match cmp {
            std::cmp::Ordering::Less => r1,
            std::cmp::Ordering::Equal => {
                if even {
                    r1
                } else {
                    r2
                }
            }
            std::cmp::Ordering::Greater => r2,
        },
        UnsignedMode::HalfInfinity => {
            if cmp == std::cmp::Ordering::Less {
                r1
            } else {
                r2
            }
        }
        UnsignedMode::HalfZero => {
            if cmp == std::cmp::Ordering::Greater {
                r2
            } else {
                r1
            }
        }
    }
}

/// ComputeNudgeWindow：smallestUnit 为 day/week/month/year 时的舍入窗口。
/// unit：0=year 1=month 2=week 3=day。
pub(crate) fn nudge_window(
    sign: i128, values: &[f64; 10], date1: (i128, i128, i128), increment: i128, unit: usize, shift: bool,
) -> (i128, i128, [f64; 10], [f64; 10]) {
    let years = values[0] as i128;
    let months = values[1] as i128;
    let weeks = values[2] as i128;
    let days = values[3] as i128;
    let (r1, r2) = match unit {
        0 => {
            let y = round_to_increment_trunc(years, increment);
            let r1 = if !shift { y } else { y + increment * sign };
            (r1, r1 + increment * sign)
        }
        1 => {
            let m = round_to_increment_trunc(months, increment);
            let r1 = if !shift { m } else { m + increment * sign };
            (r1, r1 + increment * sign)
        }
        2 => {
            let weeks_start = add_months_i128(date1.0, date1.1, date1.2, years * 12 + months);
            let weeks_end = add_days_iso(weeks_start, days);
            let w = date_until_iso(weeks_start, weeks_end, 2)[2] as i128;
            let total = weeks + w;
            let r1 = round_to_increment_trunc(total, increment);
            let r1 = if !shift { r1 } else { r1 + increment * sign };
            (r1, r1 + increment * sign)
        }
        _ => {
            let d = round_to_increment_trunc(days, increment);
            let r1 = if !shift { d } else { d + increment * sign };
            (r1, r1 + increment * sign)
        }
    };
    let mut start = [0.0; 10];
    let mut end = [0.0; 10];
    match unit {
        0 => {
            start[0] = r1 as f64;
            end[0] = r2 as f64;
        }
        1 => {
            start[0] = years as f64;
            start[1] = r1 as f64;
            end[0] = years as f64;
            end[1] = r2 as f64;
        }
        2 => {
            start[0] = years as f64;
            start[1] = months as f64;
            start[2] = r1 as f64;
            end[0] = years as f64;
            end[1] = months as f64;
            end[2] = r2 as f64;
        }
        _ => {
            start[0] = years as f64;
            start[1] = months as f64;
            start[2] = weeks as f64;
            start[3] = r1 as f64;
            end[0] = years as f64;
            end[1] = months as f64;
            end[2] = weeks as f64;
            end[3] = r2 as f64;
        }
    }
    (r1, r2, start, end)
}

/// NudgeToCalendarUnit：日历单位用纪元纳秒边界取整。
#[allow(clippy::too_many_arguments)]
fn nudge_to_calendar_unit(
    sign: i128, values: &[f64; 10], origin_epoch: i128, dest_epoch: i128, date1: (i128, i128, i128), time1_ns: i128,
    increment: i128, unit: usize, mode: InstantRoundingMode,
) -> Result<([f64; 10], i128, bool), ()> {
    let epoch_of = |dur: &[f64; 10]| -> Result<i128, ()> {
        if date_duration_sign(dur) == 0 {
            return Ok(origin_epoch);
        }
        let date = add_date_duration(date1, dur);
        let days = days_from_civil(date.0, date.1, date.2);
        if days.abs() > MAX_ISO_DAY {
            return Err(());
        }
        Ok(days * DAY_NS + time1_ns)
    };
    let mut did_expand = false;
    let (mut r1, mut r2, mut start_dur, mut end_dur) = nudge_window(sign, values, date1, increment, unit, false);
    let mut start_epoch = epoch_of(&start_dur)?;
    let mut end_epoch = epoch_of(&end_dur)?;
    let in_window = if sign > 0 {
        dest_epoch >= start_epoch && dest_epoch <= end_epoch
    } else {
        dest_epoch <= start_epoch && dest_epoch >= end_epoch
    };
    if !in_window {
        (r1, r2, start_dur, end_dur) = nudge_window(sign, values, date1, increment, unit, true);
        start_epoch = epoch_of(&start_dur)?;
        end_epoch = epoch_of(&end_dur)?;
        did_expand = true;
        let in_window = if sign > 0 {
            dest_epoch >= start_epoch && dest_epoch <= end_epoch
        } else {
            dest_epoch <= start_epoch && dest_epoch >= end_epoch
        };
        if !in_window {
            return Err(());
        }
    }
    let numerator = dest_epoch - start_epoch;
    let denominator = end_epoch - start_epoch;
    let even = (r1.abs() / increment) % 2 == 0;
    let rounded_unit = if numerator == 0 {
        r1.abs()
    } else if numerator == denominator {
        r2.abs()
    } else {
        let cmp = (numerator * 2).abs().cmp(&denominator.abs());
        apply_unsigned_rounding(r1.abs(), r2.abs(), cmp, even, mode, sign < 0)
    };
    did_expand = did_expand || rounded_unit == r2.abs();
    let duration = if rounded_unit == r2.abs() { end_dur } else { start_dur };
    let nudged = if did_expand { end_epoch } else { start_epoch };
    Ok((duration, nudged, did_expand))
}

/// BubbleRelativeDuration：舍入越过小单位边界时向更大单位进位（到 largest 为止）。
fn bubble_relative_duration(
    sign: i128, mut values: [f64; 10], nudged_epoch: i128, date1: (i128, i128, i128), time1_ns: i128, largest: usize,
    start_unit: usize,
) -> Result<[f64; 10], ()> {
    if start_unit == 0 {
        return Ok(values);
    }
    let mut unit = start_unit - 1;
    loop {
        if unit >= largest {
            if unit == 2 && largest != 2 {
                // weeks 不向 months 进位，跳过。
                if unit == 0 {
                    return Ok(values);
                }
                unit -= 1;
                continue;
            }
            let mut end_dur = values;
            match unit {
                0 => {
                    end_dur[0] = (values[0] as i128 + sign) as f64;
                    end_dur[1] = 0.0;
                    end_dur[2] = 0.0;
                    end_dur[3] = 0.0;
                }
                1 => {
                    end_dur[1] = (values[1] as i128 + sign) as f64;
                    end_dur[2] = 0.0;
                    end_dur[3] = 0.0;
                }
                2 => {
                    end_dur[2] = (values[2] as i128 + sign) as f64;
                    end_dur[3] = 0.0;
                }
                _ => unreachable!(),
            }
            end_dur[4..10].fill(0.0);
            let end_date = add_date_duration(date1, &end_dur);
            // Bubble 边界只用于比较，不做 ISO 范围校验（对齐 bugzilla 2036259）。
            let end_epoch = days_from_civil(end_date.0, end_date.1, end_date.2) * DAY_NS + time1_ns;
            let reached_end = if sign > 0 { nudged_epoch >= end_epoch } else { nudged_epoch <= end_epoch };
            if reached_end {
                values = end_dur;
            } else {
                return Ok(values);
            }
            if unit == 0 {
                return Ok(values);
            }
            unit -= 1;
        } else {
            return Ok(values);
        }
    }
}

/// 在年月上推进指定月数并保持日（超出目标月末时截断）。
fn add_months_i128(year: i128, month: i128, day: i128, months: i128) -> (i128, i128, i128) {
    let total = year * 12 + (month - 1) + months;
    let ny = total.div_euclid(12);
    let nm = total.rem_euclid(12) + 1;
    let max_day = days_in_month(ny, nm).unwrap_or(31);
    (ny, nm, day.min(max_day))
}

/// `Temporal.PlainDateTime.prototype.until/since(other, options)`：按最大/最小单位
/// 差值设置（GetDifferenceSettings 产物：单位层级、增量、舍入模式）。
pub(crate) struct DifferenceSettings {
    pub(crate) largest_index: usize,
    pub(crate) smallest_index: usize,
    pub(crate) increment: i128,
    pub(crate) mode: InstantRoundingMode,
}

/// PlainDate 差值单位层级：0=year 1=month 2=week 3=day（不含时间单位）。
fn plain_date_unit_index(value: &str) -> Option<usize> {
    match value {
        "year" | "years" => Some(0),
        "month" | "months" => Some(1),
        "week" | "weeks" => Some(2),
        "day" | "days" => Some(3),
        _ => None,
    }
}

/// PlainYearMonth 差值单位层级：仅 0=year 1=month（week/day 及时间单位均不合法）。
fn plain_year_month_unit_index(value: &str) -> Option<usize> {
    match value {
        "year" | "years" => Some(0),
        "month" | "months" => Some(1),
        _ => None,
    }
}

/// 解析差值选项（对齐 GetDifferenceSettings）：读取顺序 largestUnit →
/// roundingIncrement → roundingMode → smallestUnit。date_only 时单位限定
/// year/month/week/day，smallestUnit 缺省 "day"（含时间时缺省 "nanosecond"）。
/// default_largest 为 largestUnit 缺省/auto 时相对 smallestUnit 的取小上限
/// （PDT/PD 传 3=day，ZDT 传 4=hour）。
pub(crate) fn parse_difference_settings<H: VmHost>(
    vm: &mut H, options_value: JsValue, date_only: bool, default_largest: usize,
) -> Result<DifferenceSettings, JsValue> {
    let unit_index = |value: &str| -> Option<usize> {
        if date_only {
            plain_date_unit_index(value)
        } else {
            plain_date_time_unit_index(value)
        }
    };
    let default_smallest = if date_only { "day" } else { "nanosecond" };
    let (largest_raw, increment_value, mode_value, smallest_raw) = if options_value.is_undefined() {
        (None, 1.0, "trunc".to_string(), default_smallest.to_string())
    } else {
        if !options_value.is_object() {
            return Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let largest_raw = match temporal_option_value(vm, options, options_value, "largestUnit") {
            Ok(raw) if raw.is_undefined() => None,
            Ok(raw) => Some(temporal_option_string(vm, raw)?),
            Err(error) => return Err(error),
        };
        let increment_raw = match temporal_option_value(vm, options, options_value, "roundingIncrement") {
            Ok(raw) if raw.is_undefined() => 1.0,
            Ok(raw) => temporal_option_number(vm, raw)?,
            Err(error) => return Err(error),
        };
        let mode_raw = match temporal_option_value(vm, options, options_value, "roundingMode") {
            Ok(raw) if raw.is_undefined() => "trunc".to_string(),
            Ok(raw) => temporal_option_string(vm, raw)?,
            Err(error) => return Err(error),
        };
        let smallest_raw = match temporal_option_value(vm, options, options_value, "smallestUnit") {
            Ok(raw) if raw.is_undefined() => default_smallest.to_string(),
            Ok(raw) => temporal_option_string(vm, raw)?,
            Err(error) => return Err(error),
        };
        (largest_raw, increment_raw, mode_raw, smallest_raw)
    };

    let smallest_index = match unit_index(&smallest_raw) {
        Some(index) => index,
        None => return Err(crate::error::create_range_error(vm, "invalid smallestUnit")),
    };
    // auto/缺省：LargerOfTwoTemporalUnits(default_largest, smallestUnit)。
    // 索引 0=year…3=day…9=nanosecond，更大单位取更小索引，故为 min(default_largest, smallest)。
    let largest_index = match largest_raw {
        Some(value) if value == "auto" => smallest_index.min(default_largest),
        Some(value) => match unit_index(&value) {
            Some(index) => index,
            None => return Err(crate::error::create_range_error(vm, "invalid largestUnit")),
        },
        None => smallest_index.min(default_largest),
    };
    if largest_index > smallest_index {
        return Err(crate::error::create_range_error(vm, "smallestUnit exceeds largestUnit"));
    }
    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return Err(crate::error::create_range_error(vm, "invalid roundingMode"));
    };
    if !increment_value.is_finite() {
        return Err(crate::error::create_range_error(vm, "invalid roundingIncrement"));
    }
    let increment = increment_value.trunc();
    if !(1.0..=1_000_000_000.0).contains(&increment) {
        return Err(crate::error::create_range_error(vm, "invalid roundingIncrement"));
    }
    let increment = increment as i128;
    const UNIT_LIMITS: [i128; 6] = [24, 60, 60, 1_000, 1_000, 1_000];
    if smallest_index >= 4 {
        let limit = UNIT_LIMITS[smallest_index - 4];
        if increment >= limit || limit % increment != 0 {
            return Err(crate::error::create_range_error(vm, "invalid roundingIncrement"));
        }
    }
    Ok(DifferenceSettings {
        largest_index,
        smallest_index,
        increment,
        mode,
    })
}

/// 解析 PlainYearMonth 差值选项（对齐 GetDifferenceSettings，unitGroup=date）：
/// 单位表仅 year/month（week/day/时间单位不合法）；smallestUnit 缺省 "month"，
/// largestUnit 缺省/auto 为 LargerOfTwo(year, smallest) = year。
/// 读取顺序 largestUnit → roundingIncrement → roundingMode → smallestUnit（字典序）。
pub(crate) fn parse_year_month_difference_settings<H: VmHost>(
    vm: &mut H, options_value: JsValue,
) -> Result<DifferenceSettings, JsValue> {
    let default_smallest = "month";
    let default_largest = 0usize; // year
    let (largest_raw, increment_value, mode_value, smallest_raw) = if options_value.is_undefined() {
        (None, 1.0, "trunc".to_string(), default_smallest.to_string())
    } else {
        if !options_value.is_object() {
            return Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let largest_raw = match temporal_option_value(vm, options, options_value, "largestUnit") {
            Ok(raw) if raw.is_undefined() => None,
            Ok(raw) => Some(temporal_option_string(vm, raw)?),
            Err(error) => return Err(error),
        };
        let increment_raw = match temporal_option_value(vm, options, options_value, "roundingIncrement") {
            Ok(raw) if raw.is_undefined() => 1.0,
            Ok(raw) => temporal_option_number(vm, raw)?,
            Err(error) => return Err(error),
        };
        let mode_raw = match temporal_option_value(vm, options, options_value, "roundingMode") {
            Ok(raw) if raw.is_undefined() => "trunc".to_string(),
            Ok(raw) => temporal_option_string(vm, raw)?,
            Err(error) => return Err(error),
        };
        let smallest_raw = match temporal_option_value(vm, options, options_value, "smallestUnit") {
            Ok(raw) if raw.is_undefined() => default_smallest.to_string(),
            Ok(raw) => temporal_option_string(vm, raw)?,
            Err(error) => return Err(error),
        };
        (largest_raw, increment_raw, mode_raw, smallest_raw)
    };

    let smallest_index = match plain_year_month_unit_index(&smallest_raw) {
        Some(index) => index,
        None => return Err(crate::error::create_range_error(vm, "invalid smallestUnit")),
    };
    // auto/缺省：LargerOfTwoTemporalUnits(year, smallestUnit)。更大单位取更小索引。
    let largest_index = match largest_raw {
        Some(value) if value == "auto" => smallest_index.min(default_largest),
        Some(value) => match plain_year_month_unit_index(&value) {
            Some(index) => index,
            None => return Err(crate::error::create_range_error(vm, "invalid largestUnit")),
        },
        None => smallest_index.min(default_largest),
    };
    if largest_index > smallest_index {
        return Err(crate::error::create_range_error(vm, "smallestUnit exceeds largestUnit"));
    }
    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return Err(crate::error::create_range_error(vm, "invalid roundingMode"));
    };
    if !increment_value.is_finite() {
        return Err(crate::error::create_range_error(vm, "invalid roundingIncrement"));
    }
    let increment = increment_value.trunc();
    if !(1.0..=1_000_000_000.0).contains(&increment) {
        return Err(crate::error::create_range_error(vm, "invalid roundingIncrement"));
    }
    Ok(DifferenceSettings {
        largest_index,
        smallest_index,
        increment: increment as i128,
        mode,
    })
}

/// 差值核心：internal = end - start，按设置取整；since 用 NegateRoundingMode
/// 的舍入模式并在最后整体取反（不调换两端，调换会改变 0.5 边界所在的年长）。
/// no_rounding = 单位索引空间中"最细单位"的索引：该单位且 increment=1 时
/// 舍入为恒等变换，跳过 nudge（边界处避免窗口端点的无效范围抛错）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn difference_core<H: VmHost>(
    vm: &mut H, start: (i128, i128, i128), start_time_ns: i128, end: (i128, i128, i128), end_time_ns: i128,
    settings: DifferenceSettings, since: bool, no_rounding: usize,
) -> NativeResult {
    let values = match nudge_iso_difference(vm, start, start_time_ns, end, end_time_ns, settings, since, no_rounding) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
    make_duration(vm, values)
}

/// 对 `end - start` 的 ISO 日期时间差按设置取整并平衡到最大单位。
///
/// # 步骤
/// 1. 日期差按 largestUnit 分解（时间单位时 days 并入时间）。
/// 2. 不要求舍入（smallest=nanosecond 且 increment=1）时直接按最大单位拆回。
/// 3. smallest 为 day/时间单位走 NudgeToDayOrTime；为 year/month/week 走 NudgeToCalendarUnit。
/// 4. since 用 NegateRoundingMode 舍入并在最后整体取反。
///
/// # 边界与前提
/// - Duration.round 的日历单位路径与本函数共用：round 把 duration 应用 relativeTo 后
///   以同样的 start/end 差输入本函数，输出即 balance 后的舍入结果。
/// - 时间分量为带符号 i128；start/end 日与时间各自独立，符号由差值推导。
#[allow(clippy::too_many_arguments)]
pub(crate) fn nudge_iso_difference<H: VmHost>(
    vm: &mut H, start: (i128, i128, i128), start_time_ns: i128, end: (i128, i128, i128), end_time_ns: i128,
    settings: DifferenceSettings, since: bool, no_rounding: usize,
) -> Result<[f64; 10], JsValue> {
    let DifferenceSettings {
        largest_index,
        smallest_index,
        increment,
        mut mode,
    } = settings;
    if since {
        mode = match mode {
            InstantRoundingMode::Ceil => InstantRoundingMode::Floor,
            InstantRoundingMode::Floor => InstantRoundingMode::Ceil,
            InstantRoundingMode::HalfCeil => InstantRoundingMode::HalfFloor,
            InstantRoundingMode::HalfFloor => InstantRoundingMode::HalfCeil,
            other => other,
        };
    }
    const UNIT_NS: [i128; 6] = [3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    const DAY_NS: i128 = 86_400_000_000_000;

    // 规范语义：internal = other - receiver；since 用 NegateRoundingMode 的舍入模式，最后整体取反。
    // 不调换两端（调换会改变 0.5 边界所在的年长，破坏对称性）。
    let origin_epoch = days_from_civil(start.0, start.1, start.2) * DAY_NS + start_time_ns;
    let dest_epoch = days_from_civil(end.0, end.1, end.2) * DAY_NS + end_time_ns;

    let date1 = start;
    let time1_ns = start_time_ns;
    let mut date2 = end;
    let mut time_ns = end_time_ns - time1_ns;
    let time_sign = if time_ns > 0 {
        1_i128
    } else if time_ns < 0 {
        -1_i128
    } else {
        0
    };
    let date_sign = compare_iso_date(date1, date2);
    // 日期差与时间差符号一致时，从日期差借一天给时间差，使时间落回一天内。
    if date_sign != 0 && date_sign == time_sign {
        date2 = add_days_iso(date2, time_sign);
        time_ns -= time_sign * DAY_NS;
    }

    // 日期部分：largestUnit 为时间单位时按 day 分解，再把 days 并入时间。
    let date_largest = largest_index.min(3);
    let mut date_values = date_until_iso(date1, date2, date_largest);
    let mut date_days = date_values[3] as i128;
    if largest_index >= 4 {
        time_ns += date_days * DAY_NS;
        date_values[3] = 0.0;
        date_days = 0;
    }

    let sign = {
        let date_sign = date_duration_sign(&date_values);
        if date_sign != 0 {
            date_sign
        } else if time_ns > 0 {
            1
        } else if time_ns < 0 {
            -1
        } else {
            1
        }
    };

    let mut values = if smallest_index == no_rounding && increment == 1 {
        // 不要求舍入：时间按最大单位拆回。
        let mut values = date_values;
        if largest_index >= 4 {
            let Some(time_values) = balance_instant_difference(time_ns, largest_index - 4) else {
                return Err(crate::error::create_range_error(vm, "difference is out of range"));
            };
            values[4..10].copy_from_slice(&time_values[4..10]);
        } else {
            let day_part = time_ns / DAY_NS;
            let rem = time_ns % DAY_NS;
            values[3] += day_part as f64;
            let time_values = plain_time_components_signed(rem);
            for i in 0..6 {
                values[4 + i] = time_values[i] as f64;
            }
        }
        values
    } else if smallest_index >= 3 {
        // NudgeToDayOrTime：合并天与时间为总纳秒后按单位取整（day 为均匀单位，同样走此路径）。
        let total_ns = time_ns + date_days * DAY_NS;
        let quantum = if smallest_index == 3 {
            DAY_NS * increment
        } else {
            UNIT_NS[smallest_index - 4] * increment
        };
        let Some(rounded_ns) = round_instant_difference(total_ns, quantum, mode) else {
            return Err(crate::error::create_range_error(vm, "difference is out of range"));
        };
        let whole_days = rounded_ns / DAY_NS;
        let rem = rounded_ns % DAY_NS;
        let old_whole = total_ns / DAY_NS;
        let did_expand_days = (whole_days - old_whole).signum() == total_ns.signum();
        let mut values = date_values;
        if largest_index >= 4 {
            let Some(time_values) = balance_instant_difference(rounded_ns, largest_index - 4) else {
                return Err(crate::error::create_range_error(vm, "difference is out of range"));
            };
            values[4..10].copy_from_slice(&time_values[4..10]);
        } else {
            values[3] = whole_days as f64;
            let time_values = plain_time_components_signed(rem);
            for i in 0..6 {
                values[4 + i] = time_values[i] as f64;
            }
        }
        if did_expand_days {
            let nudged = dest_epoch + (rounded_ns - total_ns);
            match bubble_relative_duration(sign, values, nudged, date1, time1_ns, largest_index, 3) {
                Ok(bubbled) => values = bubbled,
                Err(()) => return Err(crate::error::create_range_error(vm, "difference is out of range")),
            }
        }
        values
    } else {
        // NudgeToCalendarUnit：year/month/week 用纪元纳秒窗口取整。
        let (mut values, nudged_epoch, did_expand) = match nudge_to_calendar_unit(
            sign,
            &date_values,
            origin_epoch,
            dest_epoch,
            date1,
            time1_ns,
            increment,
            smallest_index,
            mode,
        ) {
            Ok(result) => result,
            Err(()) => return Err(crate::error::create_range_error(vm, "difference is out of range")),
        };
        if did_expand && smallest_index != 2 {
            match bubble_relative_duration(sign, values, nudged_epoch, date1, time1_ns, largest_index, smallest_index) {
                Ok(bubbled) => values = bubbled,
                Err(()) => return Err(crate::error::create_range_error(vm, "difference is out of range")),
            }
        }
        values
    };
    if since {
        for value in &mut values {
            if *value != 0.0 {
                *value = -*value;
            }
        }
    }
    Ok(values)
}
