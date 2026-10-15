use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, Timelike, Utc};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, ProtoKind, ToPrimitiveHint, VmHost};

/// 星期名（0 为周日，与 `(days + 4) mod 7` 的取值对齐）。
const WEEKDAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// 月名（1-based 下标）。
const MONTH_NAMES: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// 4 位年格式化：非负无符号零填充（年 0 写作 `0000`），负年 `-` 前缀加 4 位。
fn format_year_4(y: i64) -> String {
    if y < 0 {
        format!("-{:04}", -y)
    } else {
        format!("{:04}", y)
    }
}

/// GMT 偏移串：`GMT±HHMM`，时与分各 2 位（本地偏移毫秒）。
fn format_tz_ms(offset_ms: i64) -> String {
    let min = offset_ms / 60_000;
    let sign = if min < 0 { "-" } else { "+" };
    let min = min.abs();
    format!("GMT{}{:02}{:02}", sign, min / 60, min % 60)
}

fn get_timestamp(obj: &JsObject) -> f64 {
    let v = obj.get_prop_at(0);
    if v.is_double() {
        v.as_double()
    } else {
        f64::NAN
    }
}

fn set_timestamp(obj: &mut JsObject, ms: f64) {
    obj.set_prop_at(0, JsValue::float(ms));
}

fn ensure_date<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_date_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

fn date_this_mut<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<*mut JsObject, JsValue> {
    let raw = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !raw.is_object() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj_ptr = raw.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj = unsafe { &*obj_ptr };
    ensure_date(vm, obj)?;
    Ok(obj_ptr)
}

fn date_this<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<*const JsObject, JsValue> {
    let raw = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !raw.is_object() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj_ptr = raw.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj = unsafe { &*obj_ptr };
    ensure_date(vm, obj)?;
    Ok(obj_ptr)
}

fn dt_from_ms(ms: f64) -> Option<DateTime<Utc>> {
    if !ms.is_finite() {
        return None;
    }
    // f64 截断到 i64 可能溢出（超 ±9.22e18 ms），先做有界转换。
    let ms_i64 = if ms >= i64::MIN as f64 && ms <= i64::MAX as f64 {
        ms as i64
    } else {
        return None;
    };
    DateTime::from_timestamp_millis(ms_i64)
}

fn dt_from_ms_local(ms: f64) -> Option<DateTime<Local>> {
    if !ms.is_finite() {
        return None;
    }
    let ms_i64 = if ms >= i64::MIN as f64 && ms <= i64::MAX as f64 {
        ms as i64
    } else {
        return None;
    };
    DateTime::from_timestamp_millis(ms_i64).map(|dt| dt.with_timezone(&Local))
}

fn naive_from_ms(ms: f64) -> Option<NaiveDateTime> {
    if !ms.is_finite() {
        return None;
    }
    dt_from_ms(ms).map(|dt| dt.naive_utc())
}

/// 日期值上界（TimeClip：|t| > 8.64e15 ms 为 Invalid Date）。
const MS_LIMIT: i64 = 8_640_000_000_000_000;
const MS_PER_DAY: i64 = 86_400_000;

/// 日数公式（自 1970-01-01 的天数，proleptic Gregorian）：月 1-based、日
/// 1-based；越界分量直接吸收进线性日数（rollover 语义）。调用方须保证分量
/// 在 Date 可表示包络内（见 make_day 的上限判），否则乘积溢出。
fn civil_day_count(y: i64, m1: i64, d1: i64) -> i64 {
    let y = if m1 <= 2 { y - 1 } else { y };
    // era 取 400 年周期（负年向下取整），yoe 恒在 [0, 399]。
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m1 > 2 { m1 - 3 } else { m1 + 9 }) + 2) / 5 + d1 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// MakeDay(year, month, date)：各分量先截断，返回日数（月 0-based、日 1-based）；
/// 任一分量非有限或中间结果溢出返回 None（调用方按规范返回或置 NaN）。
fn make_day(y: f64, m: f64, d: f64) -> Option<i64> {
    if !y.is_finite() || !m.is_finite() || !d.is_finite() {
        return None;
    }
    let yr = y.trunc() as i64;
    let mq = m.trunc() as i64;
    let dq = d.trunc() as i64;
    // 分量远超包络时直接越界；上限兜住日数公式乘积的 i64 溢出带
    // （日分量乘积项量级为自身，4e15 远低于 i64 上限 9.2e18）。
    if yr.abs() > 1_000_000_000 || mq.abs() > 12_000_000_000 || dq.abs() > 4_000_000_000_000 {
        return None;
    }
    // 月溢出折入年份（欧几里得除法保持月在 [0, 12)）。
    let total = yr.checked_mul(12)?.checked_add(mq)?;
    Some(civil_day_count(total.div_euclid(12), total.rem_euclid(12) + 1, dq))
}

/// MakeTime(hour, minute, second, millisecond)：各分量截断后按 IEEE 754
/// 规则组合（`((h×msPerHour + m×msPerMinute) + s×msPerSecond) + milli`）；
/// 任一分量非有限返回 None。
fn make_time(h: f64, min: f64, s: f64, ms: f64) -> Option<f64> {
    if !h.is_finite() || !min.is_finite() || !s.is_finite() || !ms.is_finite() {
        return None;
    }
    Some((h.trunc() * 3_600_000.0 + min.trunc() * 60_000.0) + s.trunc() * 1_000.0 + ms.trunc())
}

/// MakeDate(day, time)：日数与日内毫秒按 IEEE 754 规则组合（`day×msPerDay + time`）；
/// 结果非有限或 TimeClip 越界（|ts| > 8.64e15）返回 None。
fn make_date_ms(days: i64, time_ms: f64) -> Option<f64> {
    let ts = days as f64 * 86_400_000.0 + time_ms;
    if !ts.is_finite() || ts.abs() > MS_LIMIT as f64 {
        return None;
    }
    Some(ts)
}

/// UTC 时间的日内毫秒（时/分/秒/毫秒分量组合）。
fn utc_time_ms(ndt: &NaiveDateTime) -> f64 {
    ndt.time().hour() as f64 * 3_600_000.0
        + ndt.time().minute() as f64 * 60_000.0
        + ndt.time().second() as f64 * 1_000.0
        + ndt.time().nanosecond() as f64 / 1_000_000.0
}

/// 本地时间八分量（年、0-based 月、日、时、分、秒、毫秒、本地偏移毫秒）。
#[derive(Clone, Copy)]
struct LocalParts {
    year: i64,
    month0: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    milli: i64,
    offset_ms: i64,
}

impl LocalParts {
    /// +0 基准分量（1970-01-01 00:00:00.000，全零）。
    const EPOCH: LocalParts = LocalParts {
        year: 1970,
        month0: 0,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        milli: 0,
        offset_ms: 0,
    };
}

/// 本地时间八分量；时间戳无效或本地偏移采样失败返回 None。
/// 纯算术路径，覆盖 TimeClip 全包络。
fn local_components(ms: f64) -> Option<LocalParts> {
    if !ms.is_finite() {
        return None;
    }
    let ts = ms as i64;
    let naive = local_offset_sample(ts)?;
    let days = naive.div_euclid(MS_PER_DAY);
    let rem = naive.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    Some(LocalParts {
        year: y,
        month0: m as i64 - 1,
        day: d as i64,
        hour: rem / 3_600_000,
        minute: (rem / 60_000) % 60,
        second: (rem / 1_000) % 60,
        milli: rem % 1_000,
        offset_ms: ts - naive,
    })
}

/// MakeFullYear（构造器与 Date.UTC 专用，setter 无此映射）：先按 ToIntegerOrInfinity
/// 截断，0 至 99 区间加 1900；NaN 与 ±Infinity 归 NaN。
fn make_full_year(y: f64) -> Option<f64> {
    if !y.is_finite() {
        return None;
    }
    let yt = y.trunc();
    Some(if (0.0..=99.0).contains(&yt) { yt + 1900.0 } else { yt })
}

/// TimeClip：|ts| > 8.64e15 或非有限的时刻值归 NaN（Invalid Date）；
/// -0 归一为 +0（Date 值域只含正零）。
fn time_clip(ts: f64) -> f64 {
    if ts.is_finite() && (-MS_LIMIT as f64..=MS_LIMIT as f64).contains(&ts) {
        if ts == 0.0 {
            0.0
        } else {
            ts
        }
    } else {
        f64::NAN
    }
}

/// 本地 setter 收尾：组合出的时刻值过 TimeClip 包络，越界（含非有限）为
/// Invalid Date（存 NaN 并返回 NaN）；有效值写回并原样返回。
fn finish_local_setter(obj: &mut JsObject, ts: f64) -> NativeResult {
    let ts = time_clip(ts);
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
}

/// Date 数值参数：完整 ToNumber（对象路径经 engine_error 恢复原始异常值）；
/// 缺参/undefined 得 NaN。
fn date_arg_number<H: VmHost>(vm: &mut H, val: JsValue) -> Result<f64, JsValue> {
    match oxide_runtime_api::to_number_full(val, vm) {
        Ok(n) => Ok(n),
        Err(e) => Err(crate::iterator::engine_error(vm, &e)),
    }
}

macro_rules! apply_date_result {
    ($obj:expr, $expr:expr) => {
        match $expr {
            Some(ts) => {
                set_timestamp($obj, ts);
                NativeResult::Ok(JsValue::float(ts))
            }
            None => {
                // 非有限或越界：置 Invalid Date 并返回 NaN。
                set_timestamp($obj, f64::NAN);
                NativeResult::Ok(JsValue::float(f64::NAN))
            }
        }
    };
}

/// MakeDay + MakeTime 语义组合本地时间戳：月/日/时/分/秒/毫秒分量越界时滚动进位，
/// 再按本地时区映射到真实 UTC 时刻。
///
/// # 边界与前提
/// - 任一分量非有限（±Infinity 或 NaN）直接返回 NaN。
/// - 分量按 ToIntegerOrInfinity 截断；年/月进位远超包络（日数公式乘积溢出带）
///   直接返回 NaN。
/// - naive 时刻超 TimeClip 包络（|naive_ms| > 8.64e15）返回 NaN，同时兜住
///   i64 转换的饱和带；最终 UTC 时刻的 TimeClip 由调用方收尾（finish_local_setter
///   或构造器 time_clip）。
fn make_local_timestamp(y: f64, m: f64, d: f64, h: f64, min: f64, sec: f64, ms: f64) -> f64 {
    // 任一非有限分量按规范无效，同时封死后续整型饱和带。
    if !y.is_finite()
        || !m.is_finite()
        || !d.is_finite()
        || !h.is_finite()
        || !min.is_finite()
        || !sec.is_finite()
        || !ms.is_finite()
    {
        return f64::NAN;
    }

    // 月溢出归一到 [0, 12)，商进位到年份（负月同样成立）。
    let m_norm = m.trunc().rem_euclid(12.0);
    let y_carry = (m.trunc() / 12.0).floor();

    // 年与月进位远超包络时直接拒绝，避免日数公式乘积溢出。
    if y.abs() > 2e15 || y_carry.abs() > 2e15 {
        return f64::NAN;
    }
    let y = (y.trunc() + y_carry) as i64;

    // 基准取当月 1 号零点，全部分量统一折算为 i64 毫秒，
    // 覆盖超出 chrono 可表示年份的区间。
    let day_ms = civil_day_count(y, m_norm as i64 + 1, 1) as f64 * 86_400_000.0;
    let time_ms = h.trunc() * 3_600_000.0 + min.trunc() * 60_000.0 + sec.trunc() * 1_000.0 + ms.trunc();
    let naive_ms = day_ms + (d.trunc() - 1.0) * 86_400_000.0 + time_ms;
    // 组合超 TimeClip 包络即无效，同时兜住 i64 转换的饱和带。
    if !naive_ms.is_finite() || naive_ms.abs() > MS_LIMIT as f64 {
        return f64::NAN;
    }
    let naive_ms = naive_ms as i64;

    // naive 时刻按本地时区解释（DST 歧义取最早），得到真实 UTC 时间戳。
    local_offset_sample(naive_ms).map(|ts| ts as f64).unwrap_or(f64::NAN)
}

/// 1970-01-01 起的日数转民用历法（年, 月, 日）。
fn civil_from_days(z0: i64) -> (i64, u32, u32) {
    let z = z0 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// 用同一 400 年循环内的参考年采样本地偏移（毫秒）。
///
/// 规范将本地时间映射定义为实现定义的近似，取同闰年周期的参考年满足该定义；
/// 参考时刻落在 DST 空档时返回 None（与范围内路径行为一致）。
fn sample_local_offset_ms(naive_ms: i64) -> Option<i64> {
    let days = naive_ms.div_euclid(86_400_000);
    let rem = naive_ms.rem_euclid(86_400_000);
    let (cy, cm, cd) = civil_from_days(days);
    // 参考年与原始年位于同一 400 年周期的相同位置，闰年状态一致（2 月 29 日仅闰年可达）。
    let (ry, rm, rd) = (2000 + cy.rem_euclid(400), cm, cd);
    let ref_naive = NaiveDate::from_ymd_opt(ry as i32, rm, rd).and_then(|nd| {
        nd.and_hms_milli_opt(
            (rem / 3_600_000) as u32,
            ((rem / 60_000) % 60) as u32,
            ((rem / 1_000) % 60) as u32,
            (rem % 1_000) as u32,
        )
    })?;
    let ref_naive_ms = civil_day_count(ry, rm as i64, rd as i64) * 86_400_000 + rem;
    let earliest = ref_naive.and_local_timezone(Local).earliest()?;
    Some(ref_naive_ms - earliest.timestamp_millis())
}

/// naive 时刻按本地时区解释（DST 歧义取最早），映射回真实 UTC 时刻；
/// 采样失败（DST 空档）返回 None。
fn local_offset_sample(naive_ms: i64) -> Option<i64> {
    sample_local_offset_ms(naive_ms).map(|off| naive_ms - off)
}

/// 整型时间戳的 TimeClip 收尾：|ts| > 8.64e15 归 NaN。
fn time_clip_i64(ts: i64) -> f64 {
    if (-MS_LIMIT..=MS_LIMIT).contains(&ts) {
        ts as f64
    } else {
        f64::NAN
    }
}

/// 严格 ISO 8601（Date Time String Format）字符级解析：
/// 返回已 TimeClip 的 UTC 毫秒时间戳；串不匹配格式返回 None。
///
/// # 边界
/// - 年为 4 位无符号（0000 至 9999）、6 位无符号扩展年，或 6 位带符号
///   扩展年；年 0 视为正，必须写作 `+000000`，`-000000` 非法。
/// - 月 01 至 12、日 01 至 31、时 00 至 24（仅 `24:00:00.000` 形态）、
///   分秒 00 至 59、毫秒 3 位；偏移为 `Z` 或 `±HH:mm` / `±HHmm` / `±HH`。
/// - 日期-only 形式解释为 UTC 零点；带偏移日期时间折算 UTC；
///   无偏移日期时间解释为本地时间（本地偏移采样失败归 NaN）。
/// - 扩展年越出时间值范围时整体按可识别处理，由 TimeClip 归 NaN，
///   不回退实现自定义格式。
fn parse_iso_strict(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut pos = 0;

    // 年：可选符号；带符号恒为 6 位扩展年，无符号为 4 位年或 6 位扩展年
    // （第 5 位起为数字时读满 6 位）。
    let y: i64;
    if pos < b.len() && (b[pos] == b'+' || b[pos] == b'-') {
        let neg = b[pos] == b'-';
        pos += 1;
        if b.len() - pos < 6 || !b[pos..pos + 6].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let yv = digits6(&b[pos..pos + 6]);
        // 扩展年 -000000 非法（年 0 视为正，须写作 +000000）。
        if neg && yv == 0 {
            return None;
        }
        pos += 6;
        y = if neg { -yv } else { yv };
    } else {
        if b.len() - pos < 4 || !b[pos..pos + 4].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut yv = digits4(&b[pos..pos + 4]);
        pos += 4;
        if pos + 2 <= b.len() && b[pos].is_ascii_digit() && b[pos + 1].is_ascii_digit() {
            yv = yv * 100 + (b[pos] - b'0') as i64 * 10 + (b[pos + 1] - b'0') as i64;
            pos += 2;
        }
        y = yv;
    }

    // 纯年形式（YYYY）：当年 1 月 1 日 UTC 零点。
    if pos == b.len() {
        return Some(time_clip_i64(civil_day_count(y, 1, 1) * MS_PER_DAY));
    }

    // 月、日各 2 位。
    if pos >= b.len() || b[pos] != b'-' {
        return None;
    }
    pos += 1;
    let m = two_digits(&b[pos..])?;
    if !(1..=12).contains(&m) {
        return None;
    }
    pos += 2;
    if pos >= b.len() || b[pos] != b'-' {
        return None;
    }
    pos += 1;
    let d = two_digits(&b[pos..])?;
    // 日须为该月合法日（ISO 8601 有效日期），越月非法。
    if !(1..=31).contains(&d) || d > days_in_month(y, m) {
        return None;
    }
    pos += 2;

    // 日期-only 形式：UTC 零点。
    if pos == b.len() {
        return Some(time_clip_i64(civil_day_count(y, m, d) * MS_PER_DAY));
    }

    // 日期时间形式：THH:mm[:ss[.sss]][偏移]。
    if b[pos] != b'T' {
        return None;
    }
    pos += 1;
    let h = two_digits(&b[pos..])?;
    if !(0..=24).contains(&h) {
        return None;
    }
    pos += 2;
    if pos >= b.len() || b[pos] != b':' {
        return None;
    }
    pos += 1;
    let mi = two_digits(&b[pos..])?;
    if mi > 59 || (h == 24 && mi != 0) {
        return None;
    }
    pos += 2;
    let mut sec = 0i64;
    let mut ms = 0i64;
    if pos < b.len() && b[pos] == b':' {
        pos += 1;
        sec = two_digits(&b[pos..])?;
        if sec > 59 || (h == 24 && sec != 0) {
            return None;
        }
        pos += 2;
        if pos < b.len() && b[pos] == b'.' {
            pos += 1;
            if b.len() - pos < 3 || !b[pos..pos + 3].iter().all(|c| c.is_ascii_digit()) {
                return None;
            }
            ms = (b[pos] - b'0') as i64 * 100 + (b[pos + 1] - b'0') as i64 * 10 + (b[pos + 2] - b'0') as i64;
            if h == 24 && ms != 0 {
                return None;
            }
            pos += 3;
        }
    }

    // 偏移：Z 或 ±HH:mm / ±HHmm / ±HH。
    let offset_min = if pos == b.len() {
        None
    } else if b[pos] == b'Z' {
        pos += 1;
        Some(0)
    } else {
        if b[pos] != b'+' && b[pos] != b'-' {
            return None;
        }
        let osign = if b[pos] == b'-' { -1 } else { 1 };
        pos += 1;
        if b.len() - pos < 2 || !b[pos..pos + 2].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let oh = (b[pos] - b'0') as i64 * 10 + (b[pos + 1] - b'0') as i64;
        if oh > 23 {
            return None;
        }
        pos += 2;
        let mut om = 0i64;
        if pos < b.len() && (b[pos] == b':' || b[pos].is_ascii_digit()) {
            if b[pos] == b':' {
                pos += 1;
            }
            if b.len() - pos < 2 || !b[pos..pos + 2].iter().all(|c| c.is_ascii_digit()) {
                return None;
            }
            om = (b[pos] - b'0') as i64 * 10 + (b[pos + 1] - b'0') as i64;
            if om > 59 {
                return None;
            }
            pos += 2;
        }
        Some(osign * (oh * 60 + om))
    };
    if pos != b.len() {
        return None;
    }

    // 组合：naive 时刻按解释方式折算 UTC。
    let naive = civil_day_count(y, m, d) * MS_PER_DAY + h * 3_600_000 + mi * 60_000 + sec * 1_000 + ms;
    let ts = match offset_min {
        Some(off) => naive - off * 60_000,
        None => local_offset_sample(naive)?,
    };
    Some(time_clip_i64(ts))
}

/// 月 1-based 的该月天数（proleptic Gregorian 闰年规则）。
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn digits4(b: &[u8]) -> i64 {
    (b[0] - b'0') as i64 * 1000 + (b[1] - b'0') as i64 * 100 + (b[2] - b'0') as i64 * 10 + (b[3] - b'0') as i64
}

fn digits6(b: &[u8]) -> i64 {
    digits4(&b[0..4]) * 100 + (b[4] - b'0') as i64 * 10 + (b[5] - b'0') as i64
}

fn two_digits(b: &[u8]) -> Option<i64> {
    if b.len() < 2 || !b[0].is_ascii_digit() || !b[1].is_ascii_digit() {
        return None;
    }
    Some((b[0] - b'0') as i64 * 10 + (b[1] - b'0') as i64)
}

/// 解析引擎自身 toString / toUTCString 输出（回退分支：圆环要求
/// `Date.parse(d.toString()) === d.valueOf()` 对整毫秒值成立）。
///
/// # 边界
/// - 两种形态：`Www Mmm DD YYYY HH:MM:SS GMT±HHMM`（toString）与
///   `Www, DD Mmm YYYY HH:MM:SS GMT`（toUTCString）；星期名不校验。
/// - 年号带符号（负年 `-` 前缀），时分秒与偏移均按十进制数字解析。
/// - 年号超出 ±275760（TimeClip 包络可表示的极端年）必越界，按不可解析
///   处理返回 None，日数乘积随之有界，不溢出 i64。
fn parse_engine_date_string(s: &str) -> Option<f64> {
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() != 6 {
        return None;
    }
    let (day_s, mon_s, year_s, time_s, off_s) = if tokens[0].ends_with(',') {
        (tokens[1], tokens[2], tokens[3], tokens[4], "GMT")
    } else {
        (tokens[2], tokens[1], tokens[3], tokens[4], tokens[5])
    };
    let m = MONTH_NAMES.iter().position(|n| *n == mon_s)? as i64 + 1;
    let d = two_digits(day_s.as_bytes())?;
    let y = parse_signed_year(year_s)?;
    // 年号超出包络可表示的极端年必越 TimeClip 归 NaN，且日数乘积需有界防溢出。
    if !(-275_760..=275_760).contains(&y) {
        return None;
    }
    let (h, mi, sec) = parse_hms(time_s)?;
    let offset_min = if off_s == "GMT" {
        0
    } else {
        let rest = off_s.strip_prefix("GMT")?;
        let (sign, rest) = rest.split_at(1);
        let sign = if sign == "-" { -1 } else { 1 };
        let mut om = 0i64;
        for c in rest.bytes() {
            if !c.is_ascii_digit() {
                return None;
            }
            om = om * 10 + (c - b'0') as i64;
        }
        sign * om
    };
    let ts = civil_day_count(y, m, d) * MS_PER_DAY + h * 3_600_000 + mi * 60_000 + sec * 1_000 - offset_min * 60_000;
    Some(time_clip_i64(ts))
}

fn parse_signed_year(s: &str) -> Option<i64> {
    let (neg, rest) = if let Some(r) = s.strip_prefix('-') { (true, r) } else { (false, s) };
    if rest.is_empty() || !rest.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut y = 0i64;
    for c in rest.bytes() {
        y = y.checked_mul(10)?.checked_add((c - b'0') as i64)?;
    }
    Some(if neg { -y } else { y })
}

fn parse_hms(s: &str) -> Option<(i64, i64, i64)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        two_digits(parts[0].as_bytes())?,
        two_digits(parts[1].as_bytes())?,
        two_digits(parts[2].as_bytes())?,
    ))
}

/// 日期字符串解析总入口：先严格 ISO，失败后按序回退实现自定义格式
/// （RFC 2822、引擎自身 toString / toUTCString 输出、YYYY/MM/DD）；
/// 全部不匹配返回 NaN。
fn parse_date_string(s: &str) -> f64 {
    if let Some(ts) = parse_iso_strict(s) {
        return ts;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(s) {
        return dt.timestamp_millis() as f64;
    }
    if let Some(ts) = parse_engine_date_string(s) {
        return ts;
    }
    if let Ok(nd) = NaiveDate::parse_from_str(s, "%Y/%m/%d") {
        if let Some(ndt) = nd.and_hms_opt(0, 0, 0) {
            return ndt.and_utc().timestamp_millis() as f64;
        }
    }
    f64::NAN
}

/// toString 完整格式：`Www Mmm DD YYYY HH:MM:SS GMT±HHMM`（本地时间）；
/// 时间戳无效或本地偏移采样失败返回 `"Invalid Date"`。
fn format_local_date_string(ms: f64) -> String {
    match local_components(ms) {
        Some(p) => {
            let naive = ms as i64 + p.offset_ms;
            let days = naive.div_euclid(MS_PER_DAY);
            let wd = (days + 4).rem_euclid(7);
            format!(
                "{} {} {:02} {} {:02}:{:02}:{:02} {}",
                WEEKDAY_NAMES[wd as usize],
                MONTH_NAMES[p.month0 as usize],
                p.day,
                format_year_4(p.year),
                p.hour,
                p.minute,
                p.second,
                format_tz_ms(p.offset_ms),
            )
        }
        None => "Invalid Date".to_string(),
    }
}

/// JS `Date()` 构造逻辑：构造调用无参取当前时间、单参支持时间戳/字符串/Date 对象、
/// 多参按本地时间字段（年/月/日/时/分/秒/毫秒）组合；普通调用忽略全部参数，
/// 返回当前时刻的日期字符串（与 `new Date().toString()` 同格式）。
///
/// # 边界
/// - 单参对象按序判定：Date 对象直取 [[DateValue]]；其余对象走
///   ToPrimitive（default 提示），字符串结果按日期解析、数值结果 TimeClip。
/// - 单参数值先截断再 TimeClip（Date 值为整毫秒）。
/// - 原型按 GetPrototypeFromConstructor(new.target, %Date.prototype%) 选取。
pub fn date_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if !vm.constructing_native() {
        // 普通调用：忽略全部参数（不做任何强制转换），返回当前时刻字符串。
        let ms = Utc::now().timestamp_millis() as f64;
        return NativeResult::Ok(vm.new_string_owned(format_local_date_string(ms)));
    }

    let timestamp = if args.len() < 2 {
        Utc::now().timestamp_millis() as f64
    } else if args.len() > 2 {
        // 七参按序经完整 ToNumber 强转（对象参走 valueOf/toString，异常原样传播）。
        let y_val = native_try!(date_arg_number(vm, vm.reg(args[1])));
        let m_val = native_try!(date_arg_number(vm, vm.reg(args[2])));
        let d_val = if args.len() > 3 {
            native_try!(date_arg_number(vm, vm.reg(args[3])))
        } else {
            1.0
        };
        let h_val = if args.len() > 4 {
            native_try!(date_arg_number(vm, vm.reg(args[4])))
        } else {
            0.0
        };
        let min_val = if args.len() > 5 {
            native_try!(date_arg_number(vm, vm.reg(args[5])))
        } else {
            0.0
        };
        let sec_val = if args.len() > 6 {
            native_try!(date_arg_number(vm, vm.reg(args[6])))
        } else {
            0.0
        };
        let ms_val = if args.len() > 7 {
            native_try!(date_arg_number(vm, vm.reg(args[7])))
        } else {
            0.0
        };
        // MakeFullYear：先截断后做 0..99 映射（±Infinity 归 NaN）。
        let y_val = make_full_year(y_val).unwrap_or(f64::NAN);
        if y_val.is_nan()
            || m_val.is_nan()
            || d_val.is_nan()
            || h_val.is_nan()
            || min_val.is_nan()
            || sec_val.is_nan()
            || ms_val.is_nan()
        {
            f64::NAN
        } else {
            // 组合结果与 setter 同收尾：TimeClip 包络外（含非有限）归 NaN。
            time_clip(make_local_timestamp(y_val, m_val, d_val, h_val, min_val, sec_val, ms_val))
        }
    } else {
        let val = vm.reg(args[1]);
        if val.is_string() {
            // SAFETY: val 已确认是字符串值。
            let s = unsafe { (*val.as_string_ptr()).to_owned_string() };
            parse_date_string(&s)
        } else if val.is_int() || val.is_double() {
            // 数值参数先截断再 TimeClip（Date 值为整毫秒）。
            let n = if val.is_int() { val.as_int() as f64 } else { val.as_double() };
            time_clip(n.trunc())
        } else if val.is_object() && {
            let ptr = val.as_js_object_ptr();
            if ptr.is_null() {
                false
            } else {
                unsafe { &*ptr }.is_date_obj()
            }
        } {
            // Date 对象直取 [[DateValue]]，不触发 ToPrimitive。
            let obj = unsafe { &*val.as_js_object_ptr() };
            get_timestamp(obj)
        } else {
            // 其余对象：ToPrimitive（default 提示）；字符串结果按日期解析，
            // 数值结果 TimeClip。用户方法抛错原值传播。
            let v = match oxide_runtime_api::to_primitive(val, ToPrimitiveHint::Default, vm) {
                Ok(v) => v,
                Err(msg) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(vm, &msg));
                }
            };
            if v.is_string() {
                // SAFETY: v 已确认是字符串值。
                let s = unsafe { (*v.as_string_ptr()).to_owned_string() };
                parse_date_string(&s)
            } else {
                match oxide_runtime_api::to_number_full(v, vm) {
                    Ok(n) => time_clip(n),
                    Err(_) => {
                        if let Some(exc) = vm.take_uncaught_value() {
                            return NativeResult::Err(exc);
                        }
                        return NativeResult::Err(crate::error::create_type_error(
                            vm,
                            "Cannot convert value to a number",
                        ));
                    }
                }
            }
        }
    };

    // GetPrototypeFromConstructor(new.target, %Date.prototype%)：读 new.target
    // 的 "prototype"（访问器触发，异常原值传播），非对象结果回落默认原型。
    let new_target = vm.reg(255);
    let proto = if new_target.is_object() {
        let nt_ptr = new_target.as_js_object_ptr();
        // SAFETY: is_object 保证指针非空且对象本 session 存活。
        let nt_obj = unsafe { &*nt_ptr };
        let proto_si = vm.perm_intern("prototype");
        match vm.ordinary_get(nt_obj, proto_si, new_target) {
            Ok(p) if p.is_object() => p,
            Ok(_) => JsValue::from_js_object(vm.builtin_proto(ProtoKind::DateProto)),
            Err(msg) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, &msg));
            }
        }
    } else {
        JsValue::from_js_object(vm.builtin_proto(ProtoKind::DateProto))
    };

    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_DATE;
    obj.set_prop_at(0, JsValue::float(timestamp));

    let ptr = vm.alloc_object(obj);
    NativeResult::Ok(JsValue::from_js_object(ptr))
}

/// `Date.now()`：返回当前时间戳（毫秒）。
pub fn date_now<H: VmHost>(_vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Ok(JsValue::float(Utc::now().timestamp_millis() as f64))
}

/// `Date.parse(string)`：解析日期字符串为时间戳；无法解析返回 NaN。
///
/// # 步骤
/// 1. 先按严格 Date Time String Format 解析（含扩展年）；可识别串直接
///    返回其 TimeClip 结果（越界扩展年归 NaN，不回退）。
/// 2. 不匹配时回退实现自定义格式：RFC 2822、引擎自身 toString /
///    toUTCString 输出、YYYY/MM/DD。
pub fn date_parse<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let val = vm.reg(args[1]);
    if !val.is_string() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    // SAFETY: val 已确认是字符串值。
    let s = unsafe { (*val.as_string_ptr()).to_owned_string() };
    NativeResult::Ok(JsValue::float(parse_date_string(&s)))
}

/// `Date.UTC(y, m, d, h, min, s, ms)`：按 UTC 各字段组合成时间戳（m 为 0-based 月）。
pub fn date_utc<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let y = native_try!(date_arg_number(vm, vm.reg(args[1])));
    let m = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    let d = if args.len() > 3 {
        native_try!(date_arg_number(vm, vm.reg(args[3])))
    } else {
        1.0
    };
    let h = if args.len() > 4 {
        native_try!(date_arg_number(vm, vm.reg(args[4])))
    } else {
        0.0
    };
    let min = if args.len() > 5 {
        native_try!(date_arg_number(vm, vm.reg(args[5])))
    } else {
        0.0
    };
    let sec = if args.len() > 6 {
        native_try!(date_arg_number(vm, vm.reg(args[6])))
    } else {
        0.0
    };
    let ms = if args.len() > 7 {
        native_try!(date_arg_number(vm, vm.reg(args[7])))
    } else {
        0.0
    };
    if y.is_nan() || m.is_nan() || d.is_nan() || h.is_nan() || min.is_nan() || sec.is_nan() || ms.is_nan() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    // MakeFullYear：先截断后做 0..99 映射（±Infinity 归 NaN）。
    let y = match make_full_year(y) {
        Some(y) => y,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    match make_day(y, m, d).and_then(|days| make_time(h, min, sec, ms).and_then(|t| make_date_ms(days, t))) {
        Some(ts) => NativeResult::Ok(JsValue::float(ts)),
        None => NativeResult::Ok(JsValue::float(f64::NAN)),
    }
}

macro_rules! make_utc_getter {
    ($name:ident, $f:expr, $df:expr) => {
        /// UTC getter（如 `getUTCFullYear`）：返回 UTC 时间的指定分量；Invalid Date 返回 NaN。
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let obj = unsafe { &*native_try!(date_this(vm, args)) };
            let ms = get_timestamp(obj);
            if !ms.is_finite() {
                return NativeResult::Ok(JsValue::float($df));
            }
            match naive_from_ms(ms) {
                Some(ndt) => NativeResult::Ok(JsValue::float(($f)(ndt))),
                None => NativeResult::Ok(JsValue::float($df)),
            }
        }
    };
}

macro_rules! make_local_getter {
    ($name:ident, $f:expr, $df:expr) => {
        /// 本地时区 getter（如 `getFullYear`）：返回本地时间的指定分量；Invalid Date 返回 NaN。
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let obj = unsafe { &*native_try!(date_this(vm, args)) };
            let ms = get_timestamp(obj);
            if !ms.is_finite() {
                return NativeResult::Ok(JsValue::float($df));
            }
            match dt_from_ms_local(ms) {
                Some(dt) => NativeResult::Ok(JsValue::float(($f)(dt))),
                None => NativeResult::Ok(JsValue::float($df)),
            }
        }
    };
}

/// `Date.prototype.getTime()`：返回 [[DateValue]]（无转换）；Invalid Date 返回 NaN。
pub fn date_get_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    NativeResult::Ok(JsValue::float(get_timestamp(obj)))
}

make_local_getter!(date_get_full_year, |dt: DateTime<Local>| dt.year() as f64, f64::NAN);
make_local_getter!(date_get_month, |dt: DateTime<Local>| dt.month0() as f64, f64::NAN);
make_local_getter!(date_get_date, |dt: DateTime<Local>| dt.day() as f64, f64::NAN);
make_local_getter!(date_get_day, |dt: DateTime<Local>| dt.weekday().num_days_from_sunday() as f64, f64::NAN);
make_local_getter!(date_get_hours, |dt: DateTime<Local>| dt.hour() as f64, f64::NAN);
make_local_getter!(date_get_minutes, |dt: DateTime<Local>| dt.minute() as f64, f64::NAN);
make_local_getter!(date_get_seconds, |dt: DateTime<Local>| dt.second() as f64, f64::NAN);
make_local_getter!(date_get_milliseconds, |dt: DateTime<Local>| dt.timestamp_subsec_millis() as f64, f64::NAN);

make_utc_getter!(date_get_utc_full_year, |ndt: NaiveDateTime| ndt.date().year() as f64, f64::NAN);
make_utc_getter!(date_get_utc_month, |ndt: NaiveDateTime| ndt.date().month0() as f64, f64::NAN);
make_utc_getter!(date_get_utc_date, |ndt: NaiveDateTime| ndt.date().day() as f64, f64::NAN);
make_utc_getter!(
    date_get_utc_day,
    |ndt: NaiveDateTime| ndt.date().weekday().num_days_from_sunday() as f64,
    f64::NAN
);
make_utc_getter!(date_get_utc_hours, |ndt: NaiveDateTime| ndt.time().hour() as f64, f64::NAN);
make_utc_getter!(date_get_utc_minutes, |ndt: NaiveDateTime| ndt.time().minute() as f64, f64::NAN);
make_utc_getter!(date_get_utc_seconds, |ndt: NaiveDateTime| ndt.time().second() as f64, f64::NAN);
make_utc_getter!(
    date_get_utc_milliseconds,
    |ndt: NaiveDateTime| ndt.time().nanosecond() as f64 / 1_000_000.0,
    f64::NAN
);

/// `Date.prototype.getTimezoneOffset()`：返回本地时区相对 UTC 的分钟偏移；
/// Invalid Date 返回 NaN。
pub fn date_get_timezone_offset<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let offset_min = local_offset_minutes(ms);
    NativeResult::Ok(JsValue::float(offset_min as f64))
}

fn local_offset_minutes(ms: f64) -> i32 {
    if !ms.is_finite() {
        return 0;
    }
    let ts = ms as i64;
    sample_local_offset_ms(ts).map(|off| -((off / 60_000) as i32)).unwrap_or(0)
}

/// `Date.prototype.setTime(ms)`：直接设置时间戳，返回新时间戳。
pub fn date_set_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = native_try!(date_arg_number(vm, val));
    // 新值过 TimeClip 包络，越界（含非有限）归 NaN。
    let v = time_clip(v);
    set_timestamp(obj, v);
    NativeResult::Ok(JsValue::float(v))
}

/// `Date.prototype.setFullYear(y, m, d)`：设置本地年份（可选月/日），返回新时间戳。
pub fn date_set_full_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // 先读 this 值；无效时取 +0（按规范不早退）。
    let ms = get_timestamp(obj);
    let y = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    // 无效 this 值取 +0 基准分量（1970-01-01 时刻全零，+0 不经 LocalTime 换算）。
    let p = if ms.is_finite() {
        local_components(ms).unwrap_or(LocalParts::EPOCH)
    } else {
        LocalParts::EPOCH
    };
    let m = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        p.month0 as f64
    };
    let d = if args.len() > 3 {
        native_try!(date_arg_number(vm, vm.reg(args[3])))
    } else {
        p.day as f64
    };
    if y.is_nan() || m.is_nan() || d.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(y, m, d, p.hour as f64, p.minute as f64, p.second as f64, p.milli as f64),
    )
}
/// `Date.prototype.setMonth(m, d)`：设置本地月份（可选日），返回新时间戳。
pub fn date_set_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    // 参数强转先于 NaN 判定（按规范序）。
    let m = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let d = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    let has_date = args.len() > 2;
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let Some(p) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    let d = if has_date { d } else { p.day as f64 };
    if m.is_nan() || d.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(p.year as f64, m, d, p.hour as f64, p.minute as f64, p.second as f64, p.milli as f64),
    )
}
/// `Date.prototype.setDate(d)`：设置本地日，返回新时间戳。
pub fn date_set_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    let d = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let Some(p) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    if d.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(
            p.year as f64,
            p.month0 as f64,
            d,
            p.hour as f64,
            p.minute as f64,
            p.second as f64,
            p.milli as f64,
        ),
    )
}
/// `Date.prototype.setHours(h, min, s, ms)`：设置本地小时（可选分/秒/毫秒），返回新时间戳。
pub fn date_set_hours<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    let h = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let min = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    let sec = if args.len() > 3 {
        native_try!(date_arg_number(vm, vm.reg(args[3])))
    } else {
        0.0
    };
    let ms_arg = if args.len() > 4 {
        native_try!(date_arg_number(vm, vm.reg(args[4])))
    } else {
        0.0
    };
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let Some(p) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    // 缺省分量取本地时刻的 TimeWithinDay 值，与 MakeTime 组合后整体进位。
    let min_v = if args.len() > 2 { min } else { p.minute as f64 };
    let sec_v = if args.len() > 3 { sec } else { p.second as f64 };
    let ms_v = if args.len() > 4 { ms_arg } else { p.milli as f64 };
    if h.is_nan() || min_v.is_nan() || sec_v.is_nan() || ms_v.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(p.year as f64, p.month0 as f64, p.day as f64, h, min_v, sec_v, ms_v),
    )
}
/// `Date.prototype.setMinutes(min, s, ms)`：设置本地分钟（可选秒/毫秒），返回新时间戳。
pub fn date_set_minutes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    let min = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let sec = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    let ms_arg = if args.len() > 3 {
        native_try!(date_arg_number(vm, vm.reg(args[3])))
    } else {
        0.0
    };
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let Some(p) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    let sec_v = if args.len() > 2 { sec } else { p.second as f64 };
    let ms_v = if args.len() > 3 { ms_arg } else { p.milli as f64 };
    if min.is_nan() || sec_v.is_nan() || ms_v.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(p.year as f64, p.month0 as f64, p.day as f64, p.hour as f64, min, sec_v, ms_v),
    )
}
/// `Date.prototype.setSeconds(s, ms)`：设置本地秒（可选毫秒），返回新时间戳。
pub fn date_set_seconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    let sec = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let ms_arg = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let Some(p) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    let ms_v = if args.len() > 2 { ms_arg } else { p.milli as f64 };
    if sec.is_nan() || ms_v.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(p.year as f64, p.month0 as f64, p.day as f64, p.hour as f64, p.minute as f64, sec, ms_v),
    )
}
/// `Date.prototype.setMilliseconds(ms)`：设置本地毫秒，返回新时间戳。
pub fn date_set_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    let ms_arg = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let Some(p) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    if ms_arg.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(
            p.year as f64,
            p.month0 as f64,
            p.day as f64,
            p.hour as f64,
            p.minute as f64,
            p.second as f64,
            ms_arg,
        ),
    )
}

/// `Date.prototype.setUTCFullYear(y, m, d)`：设置 UTC 年份（可选月/日），返回新时间戳。
///
/// 无效 this 不早退：取 +0 基准（1970-01-01 UTC 全零）继续计算并写槽；
/// 年/月/日参数在 this 值读取之后、NaN 判定之前逐一走完整 ToNumber。
pub fn date_set_utc_full_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // this 值先读，参数按序强转（规范步序），NaN 判定后置。
    let ms = get_timestamp(obj);
    let y = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let m = if args.len() > 2 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[2]))))
    } else {
        None
    };
    let d = if args.len() > 3 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[3]))))
    } else {
        None
    };
    if !ms.is_finite() {
        // +0 基准：1970-01-01 UTC 全零，日内毫秒为 0。
        return apply_date_result!(
            obj,
            make_day(y, m.unwrap_or(0.0), d.unwrap_or(1.0)).and_then(|days| make_date_ms(days, 0.0))
        );
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let m = m.unwrap_or(ndt.date().month0() as f64);
    let d = d.unwrap_or(ndt.date().day() as f64);
    let time_ms = utc_time_ms(&ndt);
    apply_date_result!(obj, make_day(y, m, d).and_then(|days| make_date_ms(days, time_ms)))
}

/// `Date.prototype.setUTCMonth(m, d)`：设置 UTC 月份（可选日），返回新时间戳。
///
/// 参数在 this 值读取之后、NaN 判定之前逐一走完整 ToNumber；无效 this 在
/// 强转完成后返回 NaN 且不写槽。
pub fn date_set_utc_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // this 值先读，参数按序强转（规范步序），NaN 判定后置。
    let ms = get_timestamp(obj);
    let m = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let d = if args.len() > 2 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[2]))))
    } else {
        None
    };
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let d = d.unwrap_or(ndt.date().day() as f64);
    let time_ms = utc_time_ms(&ndt);
    apply_date_result!(obj, make_day(ndt.date().year() as f64, m, d).and_then(|days| make_date_ms(days, time_ms)))
}

/// `Date.prototype.setUTCDate(d)`：设置 UTC 日，返回新时间戳。
///
/// 参数在 this 值读取之后、NaN 判定之前走完整 ToNumber；无效 this 在强转
/// 完成后返回 NaN 且不写槽。
pub fn date_set_utc_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // this 值先读，参数强转（规范步序），NaN 判定后置。
    let ms = get_timestamp(obj);
    let d = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let time_ms = utc_time_ms(&ndt);
    apply_date_result!(
        obj,
        make_day(ndt.date().year() as f64, ndt.date().month0() as f64, d).and_then(|days| make_date_ms(days, time_ms))
    )
}

/// `Date.prototype.setUTCHours(h, min, s, ms)`：设置 UTC 小时（可选分/秒/毫秒），返回新时间戳。
///
/// 参数在 this 值读取之后、NaN 判定之前逐一走完整 ToNumber；无效 this 在
/// 强转完成后返回 NaN 且不写槽。
pub fn date_set_utc_hours<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // this 值先读，参数按序强转（规范步序），NaN 判定后置。
    let ms = get_timestamp(obj);
    let h = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let min = if args.len() > 2 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[2]))))
    } else {
        None
    };
    let sec = if args.len() > 3 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[3]))))
    } else {
        None
    };
    let ms_arg = if args.len() > 4 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[4]))))
    } else {
        None
    };
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    // 缺省分量取当前 UTC 时刻值，与 MakeTime 组合后整体进位。
    let min_v = min.unwrap_or(ndt.time().minute() as f64);
    let sec_v = sec.unwrap_or(ndt.time().second() as f64);
    let ms_v = ms_arg.unwrap_or(ndt.time().nanosecond() as f64 / 1_000_000.0);
    let days = civil_day_count(ndt.date().year() as i64, ndt.date().month() as i64, ndt.date().day() as i64);
    let result = make_time(h, min_v, sec_v, ms_v).and_then(|time_ms| make_date_ms(days, time_ms));
    apply_date_result!(obj, result)
}

/// `Date.prototype.setUTCMinutes(min, s, ms)`：设置 UTC 分钟（可选秒/毫秒），返回新时间戳。
///
/// 参数在 this 值读取之后、NaN 判定之前逐一走完整 ToNumber；无效 this 在
/// 强转完成后返回 NaN 且不写槽。
pub fn date_set_utc_minutes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // this 值先读，参数按序强转（规范步序），NaN 判定后置。
    let ms = get_timestamp(obj);
    let min = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let sec = if args.len() > 2 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[2]))))
    } else {
        None
    };
    let ms_arg = if args.len() > 3 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[3]))))
    } else {
        None
    };
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    // 缺省分量取当前 UTC 时刻值，与 MakeTime 组合后整体进位。
    let sec_v = sec.unwrap_or(ndt.time().second() as f64);
    let ms_v = ms_arg.unwrap_or(ndt.time().nanosecond() as f64 / 1_000_000.0);
    let days = civil_day_count(ndt.date().year() as i64, ndt.date().month() as i64, ndt.date().day() as i64);
    let result = make_time(ndt.time().hour() as f64, min, sec_v, ms_v).and_then(|time_ms| make_date_ms(days, time_ms));
    apply_date_result!(obj, result)
}

/// `Date.prototype.setUTCSeconds(s, ms)`：设置 UTC 秒（可选毫秒），返回新时间戳。
///
/// 参数在 this 值读取之后、NaN 判定之前逐一走完整 ToNumber；无效 this 在
/// 强转完成后返回 NaN 且不写槽。
pub fn date_set_utc_seconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // this 值先读，参数按序强转（规范步序），NaN 判定后置。
    let ms = get_timestamp(obj);
    let sec = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let ms_arg = if args.len() > 2 {
        Some(native_try!(date_arg_number(vm, vm.reg(args[2]))))
    } else {
        None
    };
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    // 缺省分量取当前 UTC 时刻值，与 MakeTime 组合后整体进位。
    let ms_v = ms_arg.unwrap_or(ndt.time().nanosecond() as f64 / 1_000_000.0);
    let days = civil_day_count(ndt.date().year() as i64, ndt.date().month() as i64, ndt.date().day() as i64);
    let result = make_time(ndt.time().hour() as f64, ndt.time().minute() as f64, sec, ms_v)
        .and_then(|time_ms| make_date_ms(days, time_ms));
    apply_date_result!(obj, result)
}

/// `Date.prototype.setUTCMilliseconds(ms)`：设置 UTC 毫秒，返回新时间戳。
///
/// 参数在 this 值读取之后、NaN 判定之前走完整 ToNumber；无效 this 在强转
/// 完成后返回 NaN 且不写槽。
pub fn date_set_utc_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // this 值先读，参数强转（规范步序），NaN 判定后置。
    let ms = get_timestamp(obj);
    let ms_arg = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let days = civil_day_count(ndt.date().year() as i64, ndt.date().month() as i64, ndt.date().day() as i64);
    let result = make_time(ndt.time().hour() as f64, ndt.time().minute() as f64, ndt.time().second() as f64, ms_arg)
        .and_then(|time_ms| make_date_ms(days, time_ms));
    apply_date_result!(obj, result)
}

/// Annex B `Date.prototype.getYear()`：返回本地年减 1900。
pub fn date_get_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    match dt_from_ms_local(ms) {
        Some(dt) => NativeResult::Ok(JsValue::float((dt.year() - 1900) as f64)),
        None => NativeResult::Ok(JsValue::float(f64::NAN)),
    }
}

/// Annex B `Date.prototype.setYear(y)`：设置年份，0..99 自动加 1900；
/// 缺参/NaN 置为 Invalid Date。
///
/// 年份参数按完整 ToNumber 强转（对象走 valueOf/toString，Symbol 抛
/// TypeError）；this 值无效时取 +0 基准分量（1970-01-01 全零）继续，
/// 结果经 TimeClip 包络收尾。
pub fn date_set_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    // this 值先于 ToNumber(year) 读取（规范步序）。
    let ms = get_timestamp(obj);
    let y = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    // 无效 this 值取 +0 基准分量（1970-01-01 时刻全零，与 setFullYear 同口径）。
    let p = if ms.is_finite() {
        local_components(ms).unwrap_or(LocalParts::EPOCH)
    } else {
        LocalParts::EPOCH
    };
    if y.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    // MakeFullYear：ToIntegerOrInfinity 截断，0..99 映射到 1900..1999。
    let y_int = y.trunc() as i64;
    let full_year = if (0..=99).contains(&y_int) { y_int + 1900 } else { y_int };
    finish_local_setter(
        obj,
        make_local_timestamp(
            full_year as f64,
            p.month0 as f64,
            p.day as f64,
            p.hour as f64,
            p.minute as f64,
            p.second as f64,
            p.milli as f64,
        ),
    )
}

/// `Date.prototype.toGMTString()`：别名 `toUTCString`（GMT 格式）。
pub fn date_to_gmt_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_utc_string(vm, args)
}

/// `Date.prototype.toLocaleDateString()`：本地化日期字符串（当前等同于 `toDateString`）。
pub fn date_to_locale_date_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_date_string(vm, args)
}

/// `Date.prototype.toLocaleString()`：本地化日期时间字符串（当前等同于 `toString`）。
pub fn date_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_string(vm, args)
}

/// `Date.prototype.toLocaleTimeString()`：本地化时间字符串（当前等同于 `toTimeString`）。
pub fn date_to_locale_time_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_time_string(vm, args)
}

/// toISOString 的年文本：负年 `-` 前缀加 6 位零填充，年 > 9999 加 `+` 前缀
/// 加 6 位零填充，其余 4 位零填充。
fn format_iso_year(y: i64) -> String {
    if y < 0 {
        format!("-{:06}", -y)
    } else if y > 9999 {
        format!("+{:06}", y)
    } else {
        format!("{:04}", y)
    }
}

/// UTC 八分量（年、1-based 月、日、时、分、秒、毫秒、星期）。
#[derive(Clone, Copy)]
struct UtcParts {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    milli: i64,
    weekday: i64,
}

/// UTC 八分量；时间戳无效返回 None。
fn utc_components(ms: f64) -> Option<UtcParts> {
    if !ms.is_finite() {
        return None;
    }
    let ts = ms as i64;
    let days = ts.div_euclid(MS_PER_DAY);
    let rem = ts.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    let wd = (days + 4).rem_euclid(7);
    Some(UtcParts {
        year: y,
        month: m as i64,
        day: d as i64,
        hour: rem / 3_600_000,
        minute: (rem / 60_000) % 60,
        second: (rem / 1_000) % 60,
        milli: rem % 1_000,
        weekday: wd,
    })
}

/// `Date.prototype.toISOString()`：输出 ISO 8601 格式（6 位年，`Z` 收尾）。
///
/// # 边界
/// 时间值非有限或超出 TimeClip 包络（|t| > 8.64e15）抛 RangeError。
pub fn date_to_iso_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() || ms.abs() > MS_LIMIT as f64 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date"));
    }
    let p = match utc_components(ms) {
        Some(c) => c,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid date")),
    };
    let s = format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        format_iso_year(p.year),
        p.month,
        p.day,
        p.hour,
        p.minute,
        p.second,
        p.milli
    );
    NativeResult::Ok(vm.new_string_owned(s))
}

/// `Date.prototype.toJSON(key)`：泛型方法。
///
/// # 步骤
/// 1. O = ToObject(this)（null / undefined 抛 TypeError）。
/// 2. tv = ToPrimitive(O, hint Number)；tv 为非有限 Number 时返回 null
///    （非 Number 结果不拦截）。
/// 3. 返回 ? Invoke(O, "toISOString")（无参）。
///
/// # 注意事项
/// 接收者指针跨 ToPrimitive 与 Invoke 窗口钉返回寄存器保活，每窗口后重读。
pub fn date_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let o_val = match oxide_runtime_api::to_object(this_val, vm) {
        Ok(o) => o,
        Err(msg) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, &msg));
        }
    };
    vm.set_reg(0, o_val);
    let o_val = vm.reg(0);
    let tv = match oxide_runtime_api::to_primitive(o_val, ToPrimitiveHint::Number, vm) {
        Ok(t) => t,
        Err(msg) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, &msg));
        }
    };
    // 有限性检查仅对 Number 结果生效：tv 为非有限 Number 返回 null；
    // 字符串 / Symbol 等其余原始值不拦截，直落到 Invoke。
    if tv.is_int() || tv.is_double() {
        let n = if tv.is_int() { tv.as_int() as f64 } else { tv.as_double() };
        if !n.is_finite() {
            return NativeResult::Ok(JsValue::null());
        }
    }
    // Invoke(O, "toISOString")：读方法后调用（this = O、无参）。
    let o_val = vm.reg(0);
    let obj_ptr = o_val.as_js_object_ptr();
    // SAFETY: ToObject 保证 o_val 为对象且指针非空。
    let obj = unsafe { &*obj_ptr };
    let si = vm.perm_intern("toISOString");
    let method = match vm.ordinary_get(obj, si, o_val) {
        Ok(m) => m,
        Err(msg) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, &msg));
        }
    };
    if !crate::iterator::is_callable(method) {
        return NativeResult::Err(crate::error::create_type_error(vm, "toISOString is not a function"));
    }
    let result = match vm.call_function_sync(method, o_val, &[]) {
        Ok(r) => r,
        Err(msg) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, &msg));
        }
    };
    NativeResult::Ok(result)
}

/// `Date.prototype.toString()`：本地时间完整字符串（如 `Wed Aug 05 2026 12:00:00 GMT+0000`）。
pub fn date_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    NativeResult::Ok(vm.new_string_owned(format_local_date_string(ms)))
}

/// `Date.prototype.toDateString()`：日期部分字符串（如 `Wed Aug 05 2026`）。
pub fn date_to_date_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    let Some(p) = local_components(ms) else {
        return NativeResult::Ok(vm.new_string("Invalid Date"));
    };
    let days = (ms as i64 + p.offset_ms).div_euclid(MS_PER_DAY);
    let wd = (days + 4).rem_euclid(7);
    let s = format!(
        "{} {} {:02} {}",
        WEEKDAY_NAMES[wd as usize],
        MONTH_NAMES[p.month0 as usize],
        p.day,
        format_year_4(p.year)
    );
    NativeResult::Ok(vm.new_string_owned(s))
}

/// `Date.prototype.toTimeString()`：时间部分字符串（如 `12:00:00 GMT+0000`）。
pub fn date_to_time_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    let Some(p) = local_components(ms) else {
        return NativeResult::Ok(vm.new_string("Invalid Date"));
    };
    let s = format!("{:02}:{:02}:{:02} {}", p.hour, p.minute, p.second, format_tz_ms(p.offset_ms));
    NativeResult::Ok(vm.new_string_owned(s))
}

/// `Date.prototype.toUTCString()`：UTC 完整字符串（如 `Wed, 05 Aug 2026 12:00:00 GMT`）。
pub fn date_to_utc_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    let Some(p) = utc_components(ms) else {
        return NativeResult::Ok(vm.new_string("Invalid Date"));
    };
    let s = format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WEEKDAY_NAMES[p.weekday as usize],
        p.day,
        MONTH_NAMES[(p.month - 1) as usize],
        format_year_4(p.year),
        p.hour,
        p.minute,
        p.second
    );
    NativeResult::Ok(vm.new_string_owned(s))
}

/// `Date.prototype.valueOf()`：返回时间戳（毫秒）。
pub fn date_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    NativeResult::Ok(JsValue::float(ms))
}

/// `Date.prototype.toTemporalInstant()`：把时间戳换算为 epoch 纳秒并构造 Instant。
///
/// # 步骤
/// 1. `date_this` 品牌守卫（非 Date 对象抛 TypeError）。
/// 2. 取 `[[DateValue]]`，非有限（Invalid Date）抛 RangeError。
/// 3. 毫秒 × 10^6 得纳秒，交 `make_instant` 构造 Instant 对象。
///
/// # 边界
/// 有效 Date 时间戳恒为整数毫秒且在 TimeClip 包络内（|t| ≤ 8.64e15），
/// 乘积恒在 i128 内，无溢出面。
pub fn date_to_temporal_instant<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date"));
    }
    let ns = (ms as i128) * 1_000_000;
    crate::temporal::make_instant(vm, ns)
}

/// OrdinaryToPrimitive：按 [first, second] 序逐个查 `toString`/`valueOf`，
/// 首个产出非对象结果的方法返回其结果；两臂均不产出原始值抛 TypeError。
///
/// # 步骤
/// 1. 按 hint 定方法序（string 族 toString 优先，number 族 valueOf 优先）。
/// 2. 逐臂 `Get(O, name)`（receiver = O），null/undefined/非可调用跳过。
/// 3. 调用方法（this = O），结果非对象即返回。
///
/// # 注意事项
/// 用户 getter 与调用抛错经 `take_uncaught_value` 恢复原始异常值，不降级。
fn ordinary_to_primitive<H: VmHost>(vm: &mut H, obj: &JsObject, this_val: JsValue, try_first: &str) -> NativeResult {
    let (first, second) = if try_first == "string" {
        ("toString", "valueOf")
    } else {
        ("valueOf", "toString")
    };
    for name in [first, second] {
        let si = vm.perm_intern(name);
        let method = match vm.ordinary_get(obj, si, this_val) {
            Ok(m) => m,
            Err(msg) => {
                return NativeResult::Err(
                    vm.take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg)),
                );
            }
        };
        if method.is_undefined() || method.is_null() {
            continue;
        }
        if !crate::iterator::is_callable(method) {
            continue;
        }
        let result = match vm.call_function_sync(method, this_val, &[]) {
            Ok(r) => r,
            Err(msg) => {
                return NativeResult::Err(
                    vm.take_uncaught_value()
                        .unwrap_or_else(|| crate::error::create_type_error(vm, &msg)),
                );
            }
        };
        if !result.is_object() {
            return NativeResult::Ok(result);
        }
    }
    NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert object to primitive value"))
}

/// `Date.prototype[Symbol.toPrimitive](hint)`：OrdinaryToPrimitive 原始值转换。
///
/// # 步骤
/// 1. this 非对象抛 TypeError（不做 Date 品牌检查，普通对象合法）。
/// 2. hint 须为原始字符串值，严格比较（"string"/"default"/"number"），
///    其余（含 String 对象、带 toString 的对象）抛 TypeError，不做强转。
/// 3. 按 hint 定 tryFirst 走 OrdinaryToPrimitive。
///
/// # 注意事项
/// 不复用 runtime `to_primitive`：其先查 @@toPrimitive 再走 OrdinaryToPrimitive，
/// 语义漂移且对 Date 自身会递归。
pub fn date_to_primitive<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj = unsafe { &*obj_ptr };

    let hint_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !hint_val.is_string() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid hint"));
    }
    // SAFETY: hint_val 已确认是字符串值。
    let hint = unsafe { (*hint_val.as_string_ptr()).to_owned_string() };
    let try_first = match hint.as_str() {
        "string" | "default" => "string",
        "number" => "number",
        _ => return NativeResult::Err(crate::error::create_type_error(vm, "invalid hint")),
    };

    ordinary_to_primitive(vm, obj, this_val, try_first)
}
