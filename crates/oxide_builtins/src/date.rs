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
    DateTime::from_timestamp_millis(ms as i64)
}

fn dt_from_ms_local(ms: f64) -> Option<DateTime<Local>> {
    if !ms.is_finite() {
        return None;
    }
    DateTime::from_timestamp_millis(ms as i64).map(|dt| dt.with_timezone(&Local))
}

fn naive_from_ms(ms: f64) -> Option<NaiveDateTime> {
    if !ms.is_finite() {
        return None;
    }
    dt_from_ms(ms).map(|dt| dt.naive_utc())
}

/// JS `Date()` 构造逻辑：无参取当前时间；单参支持时间戳/字符串/Date 对象；
/// 多参按本地时间字段（年/月/日/时/分/秒/毫秒）组合。非构造调用返回日期字符串。
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

    let timestamp = if args.len() < 2 {
        Utc::now().timestamp_millis() as f64
    } else if args.len() > 2 {
        let now = chrono::Local::now();
        let y_val = oxide_runtime_api::to_number(vm.reg(args[1]));
        let m_val = oxide_runtime_api::to_number(vm.reg(args[2]));
        if y_val.is_nan() || m_val.is_nan() {
            f64::NAN
        } else {
            let y = y_val.trunc() as i32;
            let m = m_val.trunc() as u32;
            let d = if args.len() > 3 {
                oxide_runtime_api::to_number(vm.reg(args[3])).trunc() as u32
            } else {
                now.day()
            };
            let h = if args.len() > 4 {
                oxide_runtime_api::to_number(vm.reg(args[4])).trunc() as u32
            } else {
                now.hour()
            };
            let min = if args.len() > 5 {
                oxide_runtime_api::to_number(vm.reg(args[5])).trunc() as u32
            } else {
                now.minute()
            };
            let sec = if args.len() > 6 {
                oxide_runtime_api::to_number(vm.reg(args[6])).trunc() as u32
            } else {
                now.second()
            };
            let ms = if args.len() > 7 {
                oxide_runtime_api::to_number(vm.reg(args[7])).trunc() as u32
            } else {
                now.timestamp_subsec_millis()
            };
            NaiveDate::from_ymd_opt(y, m + 1, d)
                .and_then(|nd| {
                    nd.and_hms_milli_opt(h, min, sec, ms)
                        .and_then(|ndt| ndt.and_local_timezone(Local).earliest())
                })
                .map(|dt| dt.timestamp_millis() as f64)
                .unwrap_or(f64::NAN)
        }
    } else {
        let val = vm.reg(args[1]);
        if val.is_string() {
            // SAFETY: val 已确认是字符串值。
            let s = unsafe { (*val.as_string_ptr()).data.clone() };
            let formats = ["%Y-%m-%dT%H:%M:%S%.fZ", "%Y-%m-%dT%H:%M:%S%.f"];
            let mut ts = f64::NAN;
            for fmt in &formats {
                if let Ok(ndt) = NaiveDateTime::parse_from_str(&s, fmt) {
                    ts = ndt.and_utc().timestamp_millis() as f64;
                    break;
                }
            }
            if ts.is_nan() {
                if let Ok(nd) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
                    if let Some(ndt) = nd.and_hms_opt(0, 0, 0).and_then(|n| n.and_local_timezone(Utc).earliest()) {
                        ts = ndt.timestamp_millis() as f64;
                    }
                }
            }
            ts
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
            f64::NAN
        }
    };

    if !is_ctor_call {
        let s = if timestamp.is_finite() {
            dt_from_ms(timestamp)
                .map(|dt| dt.to_rfc2822())
                .unwrap_or_else(|| "Invalid Date".to_string())
        } else {
            "Invalid Date".to_string()
        };
        return NativeResult::Ok(vm.new_string(&s));
    }

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
    let s = unsafe { (*val.as_string_ptr()).data.clone() };
    let mut ts = f64::NAN;
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(&s) {
        ts = dt.timestamp_millis() as f64;
    }
    if ts.is_nan() {
        let formats = [
            "%Y-%m-%dT%H:%M:%S%.fZ",
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%dT%H:%M:%S%.f%:z",
            "%Y-%m-%dT%H:%M:%S%.f%#z",
        ];
        for fmt in &formats {
            if let Ok(ndt) = NaiveDateTime::parse_from_str(&s, fmt) {
                ts = ndt.and_utc().timestamp_millis() as f64;
                break;
            }
        }
    }
    if ts.is_nan() {
        if let Ok(nd) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
            if let Some(ndt) = nd.and_hms_opt(0, 0, 0).and_then(|n| n.and_local_timezone(Utc).earliest()) {
                ts = ndt.timestamp_millis() as f64;
            }
        }
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
    if args.len() < 3 {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let y = oxide_runtime_api::to_number(vm.reg(args[1]));
    let m = oxide_runtime_api::to_number(vm.reg(args[2]));
    if y.is_nan() || m.is_nan() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let y = y.trunc() as i32;
    let m = m.trunc() as u32;
    let d = if args.len() > 3 {
        oxide_runtime_api::to_number(vm.reg(args[3])).trunc() as u32
    } else {
        1
    };
    let h = if args.len() > 4 {
        oxide_runtime_api::to_number(vm.reg(args[4])).trunc() as u32
    } else {
        0
    };
    let min = if args.len() > 5 {
        oxide_runtime_api::to_number(vm.reg(args[5])).trunc() as u32
    } else {
        0
    };
    let sec = if args.len() > 6 {
        oxide_runtime_api::to_number(vm.reg(args[6])).trunc() as u32
    } else {
        0
    };
    let ms = if args.len() > 7 {
        oxide_runtime_api::to_number(vm.reg(args[7])).trunc() as u32
    } else {
        0
    };
    let ts = NaiveDate::from_ymd_opt(y, m + 1, d)
        .and_then(|nd| nd.and_hms_milli_opt(h, min, sec, ms))
        .and_then(|ndt| ndt.and_local_timezone(Utc).earliest())
        .map(|dt| dt.timestamp_millis() as f64)
        .unwrap_or(f64::NAN);
    NativeResult::Ok(JsValue::float(ts))
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

fn get_opt_arg<H: VmHost>(vm: &H, args: &[u8], idx: usize, default: u32) -> u32 {
    if args.len() > idx {
        oxide_runtime_api::to_number(vm.reg(args[idx])) as u32
    } else {
        default
    }
}

/// `Date.prototype.setTime(ms)`：直接设置时间戳，返回新时间戳。
pub fn date_set_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let val = if args.len() > 1 {
        vm.coerce_number_bounded(vm.reg(args[1])).unwrap_or(f64::NAN)
    } else {
        f64::NAN
    };
    set_timestamp(obj, val);
    NativeResult::Ok(JsValue::float(val))
}

/// `Date.prototype.setFullYear(y, m, d)`：设置本地年份（可选月/日），返回新时间戳。
pub fn date_set_full_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = vm.coerce_number_bounded(val).unwrap_or(f64::NAN);
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms_local(ms) {
        Some(d) => d,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let m = get_opt_arg(vm, args, 2, dt.month0());
    let d = get_opt_arg(vm, args, 3, dt.day0());
    let nd = dt
        .with_year(v as i32)
        .and_then(|x| x.with_month0(m))
        .and_then(|x| x.with_day0(if args.len() > 3 { d - 1 } else { d }))
        .unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
}
/// `Date.prototype.setMonth(m, d)`：设置本地月份（可选日），返回新时间戳。
pub fn date_set_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = vm.coerce_number_bounded(val).unwrap_or(f64::NAN);
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms_local(ms) {
        Some(d) => d,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let d = get_opt_arg(vm, args, 2, dt.day0());
    let nd = dt
        .with_month0(v as u32)
        .and_then(|x| x.with_day0(if args.len() > 2 { d - 1 } else { d }))
        .unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
}
/// `Date.prototype.setDate(d)`：设置本地日，返回新时间戳。
pub fn date_set_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = vm.coerce_number_bounded(val).unwrap_or(f64::NAN);
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms_local(ms) {
        Some(d) => d,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let nd = dt.with_day0(v as u32).unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
}
/// `Date.prototype.setHours(h, min, s, ms)`：设置本地小时（可选分/秒/毫秒），返回新时间戳。
pub fn date_set_hours<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = vm.coerce_number_bounded(val).unwrap_or(f64::NAN);
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms_local(ms) {
        Some(d) => d,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let min = get_opt_arg(vm, args, 2, dt.minute());
    let sec = get_opt_arg(vm, args, 3, dt.second());
    let ms_arg = get_opt_arg(vm, args, 4, dt.timestamp_subsec_millis());
    let nd = dt
        .with_hour(v as u32)
        .and_then(|x| x.with_minute(min))
        .and_then(|x| x.with_second(sec))
        .and_then(|x| x.with_nanosecond(ms_arg * 1_000_000))
        .unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
}
/// `Date.prototype.setMinutes(min, s, ms)`：设置本地分钟（可选秒/毫秒），返回新时间戳。
pub fn date_set_minutes<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = vm.coerce_number_bounded(val).unwrap_or(f64::NAN);
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms_local(ms) {
        Some(d) => d,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let sec = get_opt_arg(vm, args, 2, dt.second());
    let ms_arg = get_opt_arg(vm, args, 3, dt.timestamp_subsec_millis());
    let nd = dt
        .with_minute(v as u32)
        .and_then(|x| x.with_second(sec))
        .and_then(|x| x.with_nanosecond(ms_arg * 1_000_000))
        .unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
}
/// `Date.prototype.setSeconds(s, ms)`：设置本地秒（可选毫秒），返回新时间戳。
pub fn date_set_seconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = vm.coerce_number_bounded(val).unwrap_or(f64::NAN);
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms_local(ms) {
        Some(d) => d,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let ms_arg = get_opt_arg(vm, args, 2, dt.timestamp_subsec_millis());
    let nd = dt
        .with_second(v as u32)
        .and_then(|x| x.with_nanosecond(ms_arg * 1_000_000))
        .unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
}
/// `Date.prototype.setMilliseconds(ms)`：设置本地毫秒，返回新时间戳。
pub fn date_set_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = vm.coerce_number_bounded(val).unwrap_or(f64::NAN);
    let obj = unsafe { &mut *native_try!(date_this_mut(vm, args)) };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms_local(ms) {
        Some(d) => d,
        None => return NativeResult::Ok(JsValue::float(f64::NAN)),
    };
    let nd = dt.with_nanosecond(v as u32 * 1_000_000).unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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
    let year_arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let y_num = oxide_runtime_api::to_number(year_arg);
    if y_num.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let y = y_num.trunc() as i32;
    let m = get_opt_arg(vm, args, 2, ndt.date().month0());
    let d = get_opt_arg(vm, args, 3, ndt.date().day0());
    let nd = NaiveDate::from_ymd_opt(y, m + 1, if args.len() > 3 { d } else { d + 1 })
        .and_then(|date| {
            date.and_hms_nano_opt(ndt.time().hour(), ndt.time().minute(), ndt.time().second(), ndt.time().nanosecond())
        })
        .unwrap_or(ndt);
    let ts = nd.and_utc().timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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
    let month_arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let m_num = oxide_runtime_api::to_number(month_arg);
    if m_num.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let m = m_num.trunc() as u32;
    let d = get_opt_arg(vm, args, 2, ndt.date().day0());
    let nd = NaiveDate::from_ymd_opt(ndt.date().year(), m + 1, if args.len() > 2 { d } else { d + 1 })
        .and_then(|date| {
            date.and_hms_nano_opt(ndt.time().hour(), ndt.time().minute(), ndt.time().second(), ndt.time().nanosecond())
        })
        .unwrap_or(ndt);
    let ts = nd.and_utc().timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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
    let date_arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let d_num = oxide_runtime_api::to_number(date_arg);
    if d_num.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let d = d_num.trunc() as u32;
    let nd = NaiveDate::from_ymd_opt(ndt.date().year(), ndt.date().month(), d)
        .and_then(|date| {
            date.and_hms_nano_opt(ndt.time().hour(), ndt.time().minute(), ndt.time().second(), ndt.time().nanosecond())
        })
        .unwrap_or(ndt);
    let ts = nd.and_utc().timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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
    let hours_arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let h_num = oxide_runtime_api::to_number(hours_arg);
    if h_num.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let h = h_num.trunc() as u32;
    let min = get_opt_arg(vm, args, 2, ndt.time().minute());
    let sec = get_opt_arg(vm, args, 3, ndt.time().second());
    let ms_arg = get_opt_arg(vm, args, 4, ndt.time().nanosecond() / 1_000_000);
    let nd = ndt
        .with_hour(h)
        .and_then(|x| x.with_minute(min))
        .and_then(|x| x.with_second(sec))
        .and_then(|x| x.with_nanosecond(ms_arg * 1_000_000))
        .unwrap_or(ndt);
    let ts = nd.and_utc().timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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
    let minutes_arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let min_num = oxide_runtime_api::to_number(minutes_arg);
    if min_num.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let min = min_num.trunc() as u32;
    let sec = get_opt_arg(vm, args, 2, ndt.time().second());
    let ms_arg = get_opt_arg(vm, args, 3, ndt.time().nanosecond() / 1_000_000);
    let nd = ndt
        .with_minute(min)
        .and_then(|x| x.with_second(sec))
        .and_then(|x| x.with_nanosecond(ms_arg * 1_000_000))
        .unwrap_or(ndt);
    let ts = nd.and_utc().timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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
    let seconds_arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let sec_num = oxide_runtime_api::to_number(seconds_arg);
    if sec_num.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let sec = sec_num.trunc() as u32;
    let ms_arg = get_opt_arg(vm, args, 2, ndt.time().nanosecond() / 1_000_000);
    let nd = ndt
        .with_second(sec)
        .and_then(|x| x.with_nanosecond(ms_arg * 1_000_000))
        .unwrap_or(ndt);
    let ts = nd.and_utc().timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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
    let millis_arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let ms_num = oxide_runtime_api::to_number(millis_arg);
    if ms_num.is_nan() {
        set_timestamp(obj, f64::NAN);
        return NativeResult::Ok(JsValue::float(f64::NAN));
    }
    let ms_arg = ms_num.trunc() as u32;
    let nd = ndt.with_nanosecond(ms_arg * 1_000_000).unwrap_or(ndt);
    let ts = nd.and_utc().timestamp_millis() as f64;
    set_timestamp(obj, ts);
    NativeResult::Ok(JsValue::float(ts))
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
    let y = y_num.trunc() as i32;
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
    NativeResult::Ok(vm.new_string(&s))
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
    NativeResult::Ok(vm.new_string(&s))
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
