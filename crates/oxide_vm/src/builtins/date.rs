use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike, Utc};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::coercion;
use crate::native::NativeResult;
use crate::vm::Vm;

fn get_timestamp(obj: &JsObject) -> f64 {
    obj.get_prop_at(0).as_double()
}

fn set_timestamp(obj: &mut JsObject, ms: f64) {
    obj.set_prop_at(0, JsValue::float(ms));
}

fn is_date(vm: &Vm, obj: &JsObject) -> bool {
    let date_proto = vm.kernel().builtin_world().date_proto.as_ptr() as *mut JsObject;
    if date_proto.is_null() {
        return false;
    }
    let proto_ptr = obj.proto().as_js_object_ptr();
    if proto_ptr.is_null() {
        return false;
    }
    std::ptr::eq(proto_ptr, date_proto)
}

fn ensure_date(vm: &mut Vm, obj: &JsObject) -> NativeResult {
    if !is_date(vm, obj) {
        return Err(crate::builtins::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(JsValue::undefined())
}

fn date_this_mut(vm: &mut Vm, args: &[u8]) -> Result<*mut JsObject, JsValue> {
    let raw = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !raw.is_object() {
        return Err(crate::builtins::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj_ptr = raw.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj = unsafe { &*obj_ptr };
    ensure_date(vm, obj)?;
    Ok(obj_ptr)
}

fn date_this(vm: &mut Vm, args: &[u8]) -> Result<*const JsObject, JsValue> {
    let raw = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !raw.is_object() {
        return Err(crate::builtins::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let obj_ptr = raw.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::builtins::error::create_type_error(vm, "called on incompatible receiver"));
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

fn naive_from_ms(ms: f64) -> Option<NaiveDateTime> {
    if !ms.is_finite() {
        return None;
    }
    dt_from_ms(ms).map(|dt| dt.naive_utc())
}

pub fn date_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let is_ctor_call = this_val.is_object() && {
        let ptr = this_val.as_js_object_ptr();
        if ptr.is_null() {
            false
        } else {
            let date_proto = vm.kernel().builtin_world().date_proto.as_ptr() as *mut JsObject;
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
        let y = coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref()) as i32;
        let m = coercion::to_number(vm.reg(args[2]), vm.kernel().string_forge().as_ref()) as u32;
        let d = if args.len() > 3 {
            coercion::to_number(vm.reg(args[3]), vm.kernel().string_forge().as_ref()) as u32
        } else {
            1
        };
        let h = if args.len() > 4 {
            coercion::to_number(vm.reg(args[4]), vm.kernel().string_forge().as_ref()) as u32
        } else {
            0
        };
        let min = if args.len() > 5 {
            coercion::to_number(vm.reg(args[5]), vm.kernel().string_forge().as_ref()) as u32
        } else {
            0
        };
        let sec = if args.len() > 6 {
            coercion::to_number(vm.reg(args[6]), vm.kernel().string_forge().as_ref()) as u32
        } else {
            0
        };
        let ms = if args.len() > 7 {
            coercion::to_number(vm.reg(args[7]), vm.kernel().string_forge().as_ref()) as u32
        } else {
            0
        };
        NaiveDate::from_ymd_opt(y, m + 1, d)
            .and_then(|nd| {
                nd.and_hms_milli_opt(h, min, sec, ms)
                    .and_then(|ndt| ndt.and_local_timezone(Utc).earliest())
            })
            .map(|dt| dt.timestamp_millis() as f64)
            .unwrap_or(f64::NAN)
    } else {
        let val = vm.reg(args[1]);
        if val.is_string() {
            let s = vm.kernel().string_forge().lookup(val.as_string_index()).unwrap_or_default();
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
                is_date(vm, unsafe { &*ptr })
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
        return Ok(vm.intern(&s));
    }

    let mut obj = JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(vm.kernel().builtin_world().date_proto.as_ptr() as *mut JsObject),
    );
    obj.set_prop_at(0, JsValue::float(timestamp));

    let ptr = vm.epoch().alloc(obj);
    Ok(JsValue::from_js_object(ptr))
}

pub fn date_now(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
    Ok(JsValue::float(Utc::now().timestamp_millis() as f64))
}

pub fn date_parse(vm: &mut Vm, args: &[u8]) -> NativeResult {
    if args.len() < 2 {
        return Ok(JsValue::float(f64::NAN));
    }
    let val = vm.reg(args[1]);
    if !val.is_string() {
        return Ok(JsValue::float(f64::NAN));
    }
    let s = vm.kernel().string_forge().lookup(val.as_string_index()).unwrap_or_default();
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
    Ok(JsValue::float(ts))
}

pub fn date_utc(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
    Ok(JsValue::float(f64::NAN))
}

macro_rules! make_getter {
    ($name:ident, $f:expr, $df:expr) => {
        pub fn $name(vm: &mut Vm, args: &[u8]) -> NativeResult {
            let obj = unsafe { &*date_this(vm, args)? };
            let ms = get_timestamp(obj);
            if !ms.is_finite() {
                return Ok(JsValue::float($df));
            }
            match dt_from_ms(ms) {
                Some(dt) => Ok(JsValue::float(($f)(dt))),
                None => Ok(JsValue::float($df)),
            }
        }
    };
}

macro_rules! make_utc_getter {
    ($name:ident, $f:expr, $df:expr) => {
        pub fn $name(vm: &mut Vm, args: &[u8]) -> NativeResult {
            let obj = unsafe { &*date_this(vm, args)? };
            let ms = get_timestamp(obj);
            if !ms.is_finite() {
                return Ok(JsValue::float($df));
            }
            match naive_from_ms(ms) {
                Some(ndt) => Ok(JsValue::float(($f)(ndt))),
                None => Ok(JsValue::float($df)),
            }
        }
    };
}

make_getter!(date_get_time, |dt: DateTime<Utc>| dt.timestamp_millis() as f64, f64::NAN);
make_getter!(date_get_full_year, |dt: DateTime<Utc>| dt.year() as f64, f64::NAN);
make_getter!(date_get_month, |dt: DateTime<Utc>| dt.month0() as f64, f64::NAN);
make_getter!(date_get_date, |dt: DateTime<Utc>| dt.day() as f64, f64::NAN);
make_getter!(date_get_day, |dt: DateTime<Utc>| dt.weekday().num_days_from_sunday() as f64, f64::NAN);
make_getter!(date_get_hours, |dt: DateTime<Utc>| dt.hour() as f64, f64::NAN);
make_getter!(date_get_minutes, |dt: DateTime<Utc>| dt.minute() as f64, f64::NAN);
make_getter!(date_get_seconds, |dt: DateTime<Utc>| dt.second() as f64, f64::NAN);
make_getter!(date_get_milliseconds, |dt: DateTime<Utc>| dt.timestamp_subsec_millis() as f64, f64::NAN);

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

pub fn date_get_timezone_offset(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let _obj = unsafe { &*date_this(vm, args)? };
    let offset_min = local_offset_minutes();
    Ok(JsValue::float(offset_min as f64))
}

fn local_offset_minutes() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

fn get_opt_arg(vm: &Vm, args: &[u8], idx: usize, default: u32) -> u32 {
    if args.len() > idx {
        coercion::to_number(vm.reg(args[idx]), vm.kernel().string_forge().as_ref()) as u32
    } else {
        default
    }
}

pub fn date_set_time(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let obj = unsafe { &mut *date_this_mut(vm, args)? };
    let val = if args.len() > 1 {
        coercion::to_number(vm.reg(args[1]), vm.kernel().string_forge().as_ref())
    } else {
        f64::NAN
    };
    set_timestamp(obj, val);
    Ok(JsValue::float(val))
}

pub fn date_set_full_year(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    let obj = unsafe { &mut *date_this_mut(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms(ms) {
        Some(d) => d,
        None => return Ok(JsValue::float(f64::NAN)),
    };
    let m = get_opt_arg(vm, args, 2, dt.month0());
    let d = get_opt_arg(vm, args, 3, dt.day0());
    let nd = dt
        .with_year(v as i32)
        .and_then(|x| x.with_month0(m))
        .and_then(|x| x.with_day0(d))
        .unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    Ok(JsValue::float(ts))
}
pub fn date_set_month(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    let obj = unsafe { &mut *date_this_mut(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms(ms) {
        Some(d) => d,
        None => return Ok(JsValue::float(f64::NAN)),
    };
    let d = get_opt_arg(vm, args, 2, dt.day0());
    let nd = dt.with_month0(v as u32).and_then(|x| x.with_day0(d)).unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    Ok(JsValue::float(ts))
}
pub fn date_set_date(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    let obj = unsafe { &mut *date_this_mut(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms(ms) {
        Some(d) => d,
        None => return Ok(JsValue::float(f64::NAN)),
    };
    let nd = dt.with_day0(v as u32).unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    Ok(JsValue::float(ts))
}
pub fn date_set_hours(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    let obj = unsafe { &mut *date_this_mut(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms(ms) {
        Some(d) => d,
        None => return Ok(JsValue::float(f64::NAN)),
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
    Ok(JsValue::float(ts))
}
pub fn date_set_minutes(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    let obj = unsafe { &mut *date_this_mut(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms(ms) {
        Some(d) => d,
        None => return Ok(JsValue::float(f64::NAN)),
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
    Ok(JsValue::float(ts))
}
pub fn date_set_seconds(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    let obj = unsafe { &mut *date_this_mut(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms(ms) {
        Some(d) => d,
        None => return Ok(JsValue::float(f64::NAN)),
    };
    let ms_arg = get_opt_arg(vm, args, 2, dt.timestamp_subsec_millis());
    let nd = dt
        .with_second(v as u32)
        .and_then(|x| x.with_nanosecond(ms_arg * 1_000_000))
        .unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    Ok(JsValue::float(ts))
}
pub fn date_set_milliseconds(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let v = coercion::to_number(val, vm.kernel().string_forge().as_ref());
    let obj = unsafe { &mut *date_this_mut(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(JsValue::float(f64::NAN));
    }
    let dt = match dt_from_ms(ms) {
        Some(d) => d,
        None => return Ok(JsValue::float(f64::NAN)),
    };
    let nd = dt.with_nanosecond(v as u32 * 1_000_000).unwrap_or(dt);
    let ts = nd.timestamp_millis() as f64;
    set_timestamp(obj, ts);
    Ok(JsValue::float(ts))
}

fn date_to_string_inner(vm: &mut Vm, args: &[u8], format_str: &str, invalid: &str) -> NativeResult {
    let obj = unsafe { &*date_this(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(vm.intern(invalid));
    }
    let s = match dt_from_ms(ms) {
        Some(dt) => dt.format(format_str).to_string(),
        None => invalid.to_string(),
    };
    Ok(vm.intern(&s))
}

pub fn date_to_iso_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%Y-%m-%dT%H:%M:%S%.3fZ", "Invalid Date")
}

pub fn date_to_json(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*date_this(vm, args)? };
    let ms = get_timestamp(obj);
    if !ms.is_finite() {
        return Ok(JsValue::null());
    }
    let s = match dt_from_ms(ms) {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        None => return Ok(JsValue::null()),
    };
    Ok(vm.intern(&s))
}

pub fn date_to_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%a %b %d %Y %H:%M:%S %Z %z", "Invalid Date")
}

pub fn date_to_date_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%a %b %d %Y", "Invalid Date")
}

pub fn date_to_time_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%H:%M:%S %Z", "Invalid Date")
}

pub fn date_to_utc_string(vm: &mut Vm, args: &[u8]) -> NativeResult {
    date_to_string_inner(vm, args, "%a, %d %b %Y %H:%M:%S GMT", "Invalid Date")
}

pub fn date_value_of(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let obj = unsafe { &*date_this(vm, args)? };
    let ms = get_timestamp(obj);
    Ok(JsValue::float(ms))
}
