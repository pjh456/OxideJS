use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, Timelike, Utc};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

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
    // Date 值域约 ±1e11 天；分量远超包络时直接越界，同时兜住后续乘积溢出。
    if yr.abs() > 1_000_000_000 || mq.abs() > 12_000_000_000 || dq.abs() > 4_000_000_000 {
        return None;
    }
    // 月溢出折入年份（欧几里得除法保持月在 [0, 12)）。
    let total = yr.checked_mul(12)?.checked_add(mq)?;
    Some(civil_day_count(total.div_euclid(12), total.rem_euclid(12) + 1, dq))
}

/// MakeTime(hour, minute, second, millisecond)：各分量截断后线性组合成日内毫秒；
/// 任一分量非有限或溢出返回 None。
fn make_time(h: f64, min: f64, s: f64, ms: f64) -> Option<i64> {
    if !h.is_finite() || !min.is_finite() || !s.is_finite() || !ms.is_finite() {
        return None;
    }
    (h.trunc() as i64)
        .checked_mul(3_600_000)?
        .checked_add((min.trunc() as i64).checked_mul(60_000)?)?
        .checked_add((s.trunc() as i64).checked_mul(1_000)?)?
        .checked_add(ms.trunc() as i64)
}

/// 日数与日内毫秒组合为最终时间戳；TimeClip 越界（|ts| > 8.64e15）返回 None。
fn make_date_ms(days: i64, time_ms: i64) -> Option<f64> {
    let ts = days.checked_mul(MS_PER_DAY)?.checked_add(time_ms)?;
    if !(-MS_LIMIT..=MS_LIMIT).contains(&ts) {
        return None;
    }
    Some(ts as f64)
}

/// UTC 时间的日内毫秒（时/分/秒/毫秒分量组合）。
fn utc_time_ms(ndt: &NaiveDateTime) -> i64 {
    ndt.time().hour() as i64 * 3_600_000
        + ndt.time().minute() as i64 * 60_000
        + ndt.time().second() as i64 * 1_000
        + ndt.time().nanosecond() as i64 / 1_000_000
}

/// 本地时间七分量（年、0-based 月、日、时、分、秒、毫秒）；
/// 时间戳无效或超出可表示范围返回 None。
fn local_components(ms: f64) -> Option<(i64, i64, i64, i64, i64, i64, i64)> {
    let dt = dt_from_ms_local(ms)?;
    Some((
        dt.year() as i64,
        dt.month0() as i64,
        dt.day() as i64,
        dt.hour() as i64,
        dt.minute() as i64,
        dt.second() as i64,
        dt.timestamp_subsec_millis() as i64,
    ))
}

/// TimeClip：|ts| > 8.64e15 或非有限的时刻值归 NaN（Invalid Date）。
fn time_clip(ts: f64) -> f64 {
    if ts.is_finite() && (-MS_LIMIT as f64..=MS_LIMIT as f64).contains(&ts) {
        ts
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
    match DateTime::from_timestamp_millis(naive_ms) {
        Some(dt) => dt
            .naive_utc()
            .and_local_timezone(Local)
            .earliest()
            .map(|dt| dt.timestamp_millis() as f64)
            .unwrap_or(f64::NAN),
        None => local_offset_sample(naive_ms).map(|ts| ts as f64).unwrap_or(f64::NAN),
    }
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

/// naive 时刻超出 chrono 可表示范围（约 262142-12-31 / -262143-01-01）时，
/// 用同一 400 年循环内的参考年采样本地偏移再映射回真实 UTC 时刻。
///
/// 规范将 LocalTime 映射定义为实现定义的近似，取同闰年周期的参考年满足该定义；
/// 参考时刻落在 DST 空档时返回 None（与范围内路径行为一致）。
fn local_offset_sample(naive_ms: i64) -> Option<i64> {
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
    let offset_ms = ref_naive_ms - earliest.timestamp_millis();
    Some(naive_ms - offset_ms)
}

/// 解析 ISO 8601 日期时间字符串（含时区偏移），返回 UTC 毫秒时间戳；无法解析返回 NaN。
///
/// # 边界与前提
/// - 偏移形态 `Z` / `+hh:mm` / `+hhmm` 均折算为 UTC；无偏移完整时间按 UTC；纯日期按 UTC 零点。
fn parse_iso_timestamp(s: &str) -> f64 {
    // 带偏移（含 Z）：DateTime 保留偏移直接折算 UTC。
    if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%#z") {
        return dt.timestamp_millis() as f64;
    }
    // 无偏移完整时间按 UTC。
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return ndt.and_utc().timestamp_millis() as f64;
    }
    // 纯日期按 UTC 零点。
    if let Ok(nd) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        if let Some(ndt) = nd.and_hms_opt(0, 0, 0) {
            return ndt.and_utc().timestamp_millis() as f64;
        }
    }
    f64::NAN
}

/// JS `Date()` 构造逻辑：构造调用无参取当前时间、单参支持时间戳/字符串/Date 对象、
/// 多参按本地时间字段（年/月/日/时/分/秒/毫秒）组合；普通调用忽略全部参数，
/// 返回当前时刻的日期字符串（与 `new Date().toString()` 同格式）。
pub fn date_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let is_ctor_call = this_val.is_object() && {
        let ptr = this_val.as_js_object_ptr();
        if ptr.is_null() {
            false
        } else {
            let date_proto = vm.session().builtin_world().date_proto.as_ptr() as *mut JsObject;
            if date_proto.is_null() {
                false
            } else {
                let obj = unsafe { &*ptr };
                let proto_ptr = obj.proto().as_js_object_ptr();
                if proto_ptr.is_null() {
                    false
                } else {
                    std::ptr::eq(proto_ptr, date_proto)
                }
            }
        }
    };

    if !is_ctor_call {
        // 普通调用：忽略全部参数（不做任何强制转换），返回当前时刻字符串。
        let ms = Utc::now().timestamp_millis() as f64;
        let s = match dt_from_ms(ms) {
            Some(dt) => dt.format("%a %b %d %Y %H:%M:%S %Z %z").to_string(),
            None => "Invalid Date".to_string(),
        };
        return NativeResult::Ok(vm.new_string_owned(s));
    }

    let timestamp = if args.len() < 2 {
        Utc::now().timestamp_millis() as f64
    } else if args.len() > 2 {
        let y_val = oxide_runtime_api::to_number(vm.reg(args[1]));
        // 年 0..99 按规范映射到 1900..1999（ToInteger 截断后判断）。
        let y_val = if (0.0..=99.0).contains(&y_val.trunc()) {
            y_val.trunc() + 1900.0
        } else {
            y_val
        };
        let m_val = oxide_runtime_api::to_number(vm.reg(args[2]));
        let d_val = if args.len() > 3 { oxide_runtime_api::to_number(vm.reg(args[3])) } else { 1.0 };
        let h_val = if args.len() > 4 { oxide_runtime_api::to_number(vm.reg(args[4])) } else { 0.0 };
        let min_val = if args.len() > 5 { oxide_runtime_api::to_number(vm.reg(args[5])) } else { 0.0 };
        let sec_val = if args.len() > 6 { oxide_runtime_api::to_number(vm.reg(args[6])) } else { 0.0 };
        let ms_val = if args.len() > 7 { oxide_runtime_api::to_number(vm.reg(args[7])) } else { 0.0 };
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
            parse_iso_timestamp(&s)
        } else if val.is_int() || val.is_double() {
            if val.is_int() {
                val.as_int() as f64
            } else {
                val.as_double()
            }
        } else if val.is_object() && {
            let ptr = val.as_js_object_ptr();
            if ptr.is_null() {
                false
            } else {
                unsafe { &*ptr }.is_date_obj()
            }
        } {
            let obj = unsafe { &*val.as_js_object_ptr() };
            get_timestamp(obj)
        } else {
            match oxide_runtime_api::to_number_full(val, vm) {
                Ok(n) => n,
                Err(_) => {
                    // 对象经 ToNumber 转换时 toString/valueOf 可抛异常，须原样传播原始异常。
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
                }
            }
        }
    };

    let mut obj = JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.session().builtin_world().date_proto.as_ptr() as *mut JsObject),
    );
    obj.type_tag = JsObject::OBJ_TYPE_DATE;
    obj.set_prop_at(0, JsValue::float(timestamp));

    let ptr = vm.alloc_object(obj);
    NativeResult::Ok(JsValue::from_js_object(ptr))
}

/// `Date.now()`：返回当前时间戳（毫秒）。
pub fn date_now<H: VmHost>(_vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Ok(JsValue::float(Utc::now().timestamp_millis() as f64))
}

/// `Date.parse(string)`：解析日期字符串（RFC 2822、ISO 8601 及常用格式）为时间戳；
/// 无法解析返回 NaN。
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
    let mut ts = f64::NAN;
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(&s) {
        ts = dt.timestamp_millis() as f64;
    }
    if ts.is_nan() {
        ts = parse_iso_timestamp(&s);
    }
    if ts.is_nan() {
        if let Ok(nd) = NaiveDate::parse_from_str(&s, "%Y/%m/%d") {
            if let Some(ndt) = nd.and_hms_opt(0, 0, 0).and_then(|n| n.and_local_timezone(Utc).earliest()) {
                ts = ndt.timestamp_millis() as f64;
            }
        }
    }
    NativeResult::Ok(JsValue::float(ts))
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
    // 年 0..99 按规范映射到 1900..1999（ToInteger 截断后判断）。
    let y = if (0.0..=99.0).contains(&y.trunc()) { y + 1900.0 } else { y };
    match make_day(y, m, d).and_then(|days| make_time(h, min, sec, ms).and_then(|t| make_date_ms(days, t))) {
        Some(ts) => NativeResult::Ok(JsValue::float(ts)),
        None => NativeResult::Ok(JsValue::float(f64::NAN)),
    }
}

macro_rules! make_getter {
    ($name:ident, $f:expr, $df:expr) => {
        /// UTC 视图 getter（如 `getTime`）：返回 UTC 时间的指定分量；Invalid Date 返回 NaN。
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let obj = unsafe { &*native_try!(date_this(vm, args)) };
            let ms = get_timestamp(obj);
            if !ms.is_finite() {
                return NativeResult::Ok(JsValue::float($df));
            }
            match dt_from_ms(ms) {
                Some(dt) => NativeResult::Ok(JsValue::float(($f)(dt))),
                None => NativeResult::Ok(JsValue::float($df)),
            }
        }
    };
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

make_getter!(date_get_time, |dt: DateTime<Utc>| dt.timestamp_millis() as f64, f64::NAN);
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

/// `Date.prototype.getTimezoneOffset()`：返回本地时区相对 UTC 的分钟偏移。
pub fn date_get_timezone_offset<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    let offset_min = local_offset_minutes(ms);
    NativeResult::Ok(JsValue::float(offset_min as f64))
}

fn local_offset_minutes(ms: f64) -> i32 {
    if !ms.is_finite() {
        return 0;
    }
    let offset = chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|dt| dt.with_timezone(&chrono::Local))
        .map(|dt| dt.offset().local_minus_utc())
        .unwrap_or(0);
    -offset / 60
}

/// `Date.prototype.setTime(ms)`：直接设置时间戳，返回新时间戳。
pub fn date_set_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = native_try!(date_arg_number(vm, val));
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
    let (_dy, dm, dd, dh, dmin, dsec, dms) = if ms.is_finite() {
        local_components(ms).unwrap_or((1970, 0, 1, 0, 0, 0, 0))
    } else {
        (1970, 0, 1, 0, 0, 0, 0)
    };
    let m = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        dm as f64
    };
    let d = if args.len() > 3 {
        native_try!(date_arg_number(vm, vm.reg(args[3])))
    } else {
        dd as f64
    };
    if y.is_nan() || m.is_nan() || d.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(obj, make_local_timestamp(y, m, d, dh as f64, dmin as f64, dsec as f64, dms as f64))
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
    let Some((dy, _dm, dd, dh, dmin, dsec, dms)) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    let d = if has_date { d } else { dd as f64 };
    if m.is_nan() || d.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(dy as f64, m, d, dh as f64, dmin as f64, dsec as f64, dms as f64),
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
    let Some((dy, dm, _dd, dh, dmin, dsec, dms)) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    if d.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(dy as f64, dm as f64, d, dh as f64, dmin as f64, dsec as f64, dms as f64),
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
    let Some((dy, dm, dd, _dh, dmin, dsec, dms)) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    // 缺省分量取本地时刻的 TimeWithinDay 值，与 MakeTime 组合后整体进位。
    let min_v = if args.len() > 2 { min } else { dmin as f64 };
    let sec_v = if args.len() > 3 { sec } else { dsec as f64 };
    let ms_v = if args.len() > 4 { ms_arg } else { dms as f64 };
    if h.is_nan() || min_v.is_nan() || sec_v.is_nan() || ms_v.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(obj, make_local_timestamp(dy as f64, dm as f64, dd as f64, h, min_v, sec_v, ms_v))
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
    let Some((dy, dm, dd, dh, _dmin, dsec, dms)) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    let sec_v = if args.len() > 2 { sec } else { dsec as f64 };
    let ms_v = if args.len() > 3 { ms_arg } else { dms as f64 };
    if min.is_nan() || sec_v.is_nan() || ms_v.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(obj, make_local_timestamp(dy as f64, dm as f64, dd as f64, dh as f64, min, sec_v, ms_v))
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
    let Some((dy, dm, dd, dh, dmin, _dsec, dms)) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    let ms_v = if args.len() > 2 { ms_arg } else { dms as f64 };
    if sec.is_nan() || ms_v.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(dy as f64, dm as f64, dd as f64, dh as f64, dmin as f64, sec, ms_v),
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
    let Some((dy, dm, dd, dh, dmin, dsec, _dms)) = local_components(ms) else {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    if ms_arg.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    finish_local_setter(
        obj,
        make_local_timestamp(dy as f64, dm as f64, dd as f64, dh as f64, dmin as f64, dsec as f64, ms_arg),
    )
}

/// `Date.prototype.setUTCFullYear(y, m, d)`：设置 UTC 年份（可选月/日），返回新时间戳。
pub fn date_set_utc_full_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let y = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let m = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        ndt.date().month0() as f64
    };
    let d = if args.len() > 3 {
        native_try!(date_arg_number(vm, vm.reg(args[3])))
    } else {
        ndt.date().day() as f64
    };
    let time_ms = utc_time_ms(&ndt);
    apply_date_result!(obj, make_day(y, m, d).and_then(|days| make_date_ms(days, time_ms)))
}

/// `Date.prototype.setUTCMonth(m, d)`：设置 UTC 月份（可选日），返回新时间戳。
pub fn date_set_utc_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let m = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let d = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        ndt.date().day() as f64
    };
    let time_ms = utc_time_ms(&ndt);
    apply_date_result!(obj, make_day(ndt.date().year() as f64, m, d).and_then(|days| make_date_ms(days, time_ms)))
}

/// `Date.prototype.setUTCDate(d)`：设置 UTC 日，返回新时间戳。
pub fn date_set_utc_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let d = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let time_ms = utc_time_ms(&ndt);
    apply_date_result!(
        obj,
        make_day(ndt.date().year() as f64, ndt.date().month0() as f64, d).and_then(|days| make_date_ms(days, time_ms))
    )
}

/// `Date.prototype.setUTCHours(h, min, s, ms)`：设置 UTC 小时（可选分/秒/毫秒），返回新时间戳。
pub fn date_set_utc_hours<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
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
    // 缺省分量取当前 UTC 时刻值，与 MakeTime 组合后整体进位。
    let min_v = if args.len() > 2 { min } else { ndt.time().minute() as f64 };
    let sec_v = if args.len() > 3 { sec } else { ndt.time().second() as f64 };
    let ms_v = if args.len() > 4 {
        ms_arg
    } else {
        ndt.time().nanosecond() as f64 / 1_000_000.0
    };
    let days = civil_day_count(ndt.date().year() as i64, ndt.date().month() as i64, ndt.date().day() as i64);
    let result = make_time(h, min_v, sec_v, ms_v).and_then(|time_ms| make_date_ms(days, time_ms));
    apply_date_result!(obj, result)
}

/// `Date.prototype.setUTCMinutes(min, s, ms)`：设置 UTC 分钟（可选秒/毫秒），返回新时间戳。
pub fn date_set_utc_minutes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
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
    let sec_v = if args.len() > 2 { sec } else { ndt.time().second() as f64 };
    let ms_v = if args.len() > 3 {
        ms_arg
    } else {
        ndt.time().nanosecond() as f64 / 1_000_000.0
    };
    let days = civil_day_count(ndt.date().year() as i64, ndt.date().month() as i64, ndt.date().day() as i64);
    let result = make_time(ndt.time().hour() as f64, min, sec_v, ms_v).and_then(|time_ms| make_date_ms(days, time_ms));
    apply_date_result!(obj, result)
}

/// `Date.prototype.setUTCSeconds(s, ms)`：设置 UTC 秒（可选毫秒），返回新时间戳。
pub fn date_set_utc_seconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let sec = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
    let ms_arg = if args.len() > 2 {
        native_try!(date_arg_number(vm, vm.reg(args[2])))
    } else {
        0.0
    };
    let ms_v = if args.len() > 2 {
        ms_arg
    } else {
        ndt.time().nanosecond() as f64 / 1_000_000.0
    };
    let days = civil_day_count(ndt.date().year() as i64, ndt.date().month() as i64, ndt.date().day() as i64);
    let result = make_time(ndt.time().hour() as f64, ndt.time().minute() as f64, sec, ms_v)
        .and_then(|time_ms| make_date_ms(days, time_ms));
    apply_date_result!(obj, result)
}

/// `Date.prototype.setUTCMilliseconds(ms)`：设置 UTC 毫秒，返回新时间戳。
pub fn date_set_utc_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ndt = match naive_from_ms(ms) {
        Some(n) => n,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let ms_arg = native_try!(date_arg_number(vm, if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() }));
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
pub fn date_set_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    // 缺省年份参数强制转为 NaN；按 Annex B 语义，NaN 年份把日期值置为
    // NaN 并返回 NaN。
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let y_num = oxide_runtime_api::to_number(arg);
    if y_num.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms_local(ms) {
        Some(d) => d,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let y_trunc = y_num.trunc();
    // f64 截断后可能超出 i32 范围，先做有界转换。
    let y = if y_trunc >= i32::MIN as f64 && y_trunc <= i32::MAX as f64 {
        y_trunc as i32
    } else {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    };
    let full_year = if (0..=99).contains(&y) { y + 1900 } else { y };
    let nd = dt.with_year(full_year).unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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

fn date_to_string_inner<H: VmHost>(vm: &mut H, args: &[u8], format_str: &str, invalid: &str) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(vm.new_string(invalid));
    }
    let s = match dt_from_ms(ms) {
        Some(dt) => dt.format(format_str).to_string(),
        None => invalid.to_string(),
    };
    NativeResult::Ok(vm.new_string_owned(s))
}

/// `Date.prototype.toISOString()`：输出 ISO 8601 格式（`YYYY-MM-DDTHH:MM:SS.mmmZ`）。
pub fn date_to_iso_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%Y-%m-%dT%H:%M:%S%.3fZ", "Invalid Date")
}

/// `Date.prototype.toJSON()`：输出 ISO 8601 字符串；Invalid Date 返回 null。
pub fn date_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::null());
    }
    let s = match dt_from_ms(ms) {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        None => return NativeResult::Ok(JsValue::null()),
    };
    NativeResult::Ok(vm.new_string_owned(s))
}

/// `Date.prototype.toString()`：本地时间完整字符串（如 `Wed Aug 05 2026 12:00:00 GMT+0000`）。
pub fn date_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%a %b %d %Y %H:%M:%S %Z %z", "Invalid Date")
}

/// `Date.prototype.toDateString()`：日期部分字符串（如 `Wed Aug 05 2026`）。
pub fn date_to_date_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%a %b %d %Y", "Invalid Date")
}

/// `Date.prototype.toTimeString()`：时间部分字符串（如 `12:00:00 GMT+0000`）。
pub fn date_to_time_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%H:%M:%S %Z", "Invalid Date")
}

/// `Date.prototype.toUTCString()`：UTC 完整字符串（如 `Wed, 05 Aug 2026 12:00:00 GMT`）。
pub fn date_to_utc_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%a, %d %b %Y %H:%M:%S GMT", "Invalid Date")
}

/// `Date.prototype.valueOf()`：返回时间戳（毫秒）。
pub fn date_value_of<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*native_try!(date_this(vm, args)) };
    let ms = get_timestamp(obj);
    NativeResult::Ok(JsValue::float(ms))
}
