use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, Utc};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

// Temporal 命名空间的最小实现子集：Temporal.Now / Temporal.Instant /
// Temporal.PlainDate / Temporal.PlainTime。内部数据按对象类型存入 prop 槽：
// Instant 存纪元纳秒（f64，prop 0）、PlainDate 存年/月/日（prop 0-2）、
// PlainTime 存午夜后纳秒（f64，prop 0）。

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

fn get_double_prop(obj: &JsObject, pos: usize) -> f64 {
    let v = obj.get_prop_at(pos);
    if v.is_double() {
        v.as_double()
    } else {
        f64::NAN
    }
}

fn ensure_instant<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_instant_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

fn ensure_plain_date<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_date_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

fn ensure_plain_time<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_time_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

fn receiver_obj<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<*mut JsObject, JsValue> {
    let raw = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !raw.is_object() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    let ptr = raw.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(ptr)
}

/// 读 receiver 是否为构造调用：`new` 时 this 的原型是相应构造器的 prototype。
fn is_ctor_call<H: VmHost>(vm: &mut H, args: &[u8], proto_ptr: *const JsObject) -> bool {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return false;
    }
    let ptr = this_val.as_js_object_ptr();
    if ptr.is_null() {
        return false;
    }
    let obj = unsafe { &*ptr };
    let this_proto = obj.proto().as_js_object_ptr();
    !this_proto.is_null() && std::ptr::eq(this_proto, proto_ptr)
}

fn make_instant<H: VmHost>(vm: &mut H, epoch_ns: i128) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().instant_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_INSTANT;
    obj.set_prop_at(0, JsValue::float(epoch_ns as f64));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

/// 把 epoch 纳秒拆成秒 + 亚秒纳秒（供 chrono 时间转换）。
fn ns_to_datetime(epoch_ns: f64) -> Option<DateTime<Utc>> {
    if !epoch_ns.is_finite() {
        return None;
    }
    let secs = (epoch_ns / 1e9).trunc() as i64;
    let sub_ns = (epoch_ns - (epoch_ns / 1e9).trunc() * 1e9).round() as u32;
    DateTime::from_timestamp(secs, sub_ns)
}

/// 解析 `YYYY-MM-DDTHH:MM:SS[.fff](Z|±HH:MM)` 形式的 ISO 字符串到纪元纳秒。
fn parse_instant_string(s: &str) -> Option<i128> {
    let t = s.trim();
    let dt = DateTime::parse_from_rfc3339(t).ok()?;
    let utc = dt.with_timezone(&Utc);
    Some(utc.timestamp() as i128 * 1_000_000_000 + utc.timestamp_subsec_nanos() as i128)
}

/// `Temporal.Now.instant()`：返回当前时刻的 Temporal.Instant。
pub fn now_instant<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    let now = Utc::now();
    let epoch_ns = now.timestamp() as i128 * 1_000_000_000 + now.timestamp_subsec_nanos() as i128;
    make_instant(vm, epoch_ns)
}

/// `Temporal.Now.timeZoneId()`：引擎固定使用 UTC。
pub fn now_time_zone_id<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Ok(vm.new_string("UTC"))
}

/// `Temporal.Instant` 构造器：接受一个数值（纪元纳秒，BigInt 未实现故按 number 处理）。
pub fn instant_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().instant_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.Instant cannot be invoked without 'new'",
        ));
    }
    let epoch_ns = if args.len() < 2 {
        0.0
    } else {
        to_number(vm.reg(args[1]))
    };
    make_instant(vm, epoch_ns as i128)
}

/// `Temporal.Instant.from(value)`：接受 ISO 字符串、数值（纪元纳秒）或
/// 带 `epochMilliseconds`/`epochNanoseconds` 数值字段的对象。
pub fn instant_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let epoch_ns = if val.is_string() {
        let s = to_string(val);
        match parse_instant_string(&s) {
            Some(ns) => ns,
            None => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO 8601 string"));
            }
        }
    } else if val.is_int() || val.is_double() {
        to_number(val) as i128
    } else if val.is_object() && {
        let ptr = val.as_js_object_ptr();
        !ptr.is_null() && unsafe { &*ptr }.is_instant_obj()
    } {
        let obj = unsafe { &*val.as_js_object_ptr() };
        get_double_prop(obj, 0) as i128
    } else if val.is_object() {
        let obj = unsafe { &*val.as_js_object_ptr() };
        let obj_val = val;
        let ms = read_prop_number(vm, obj, obj_val, "epochMilliseconds");
        let ns = read_prop_number(vm, obj, obj_val, "epochNanoseconds");
        if !ns.is_nan() {
            ns as i128
        } else if !ms.is_nan() {
            (ms * 1e6) as i128
        } else {
            return NativeResult::Err(crate::error::create_type_error(vm, "cannot convert object to Instant"));
        }
    } else {
        return NativeResult::Err(crate::error::create_type_error(vm, "cannot convert value to Instant"));
    };
    make_instant(vm, epoch_ns)
}

/// 读对象的数值字段（普通对象访问器 getter 路径）。
fn read_prop_number<H: VmHost>(vm: &mut H, obj: &JsObject, receiver: JsValue, name: &str) -> f64 {
    let key_val = vm.new_string(name);
    let si = vm.property_key_si(key_val);
    vm.ordinary_get(obj, si, receiver).map(to_number).unwrap_or(f64::NAN)
}

/// `Temporal.Instant.prototype.epochSeconds` getter。
pub fn instant_epoch_seconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    NativeResult::Ok(JsValue::float((get_double_prop(obj, 0) / 1e9).trunc()))
}

/// `Temporal.Instant.prototype.epochMilliseconds` getter。
pub fn instant_epoch_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    NativeResult::Ok(JsValue::float((get_double_prop(obj, 0) / 1e6).trunc()))
}

/// `Temporal.Instant.prototype.epochMicroseconds` getter。
pub fn instant_epoch_microseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    NativeResult::Ok(JsValue::float((get_double_prop(obj, 0) / 1e3).trunc()))
}

/// `Temporal.Instant.prototype.epochNanoseconds` getter：BigInt 未实现，返回 number 近似。
pub fn instant_epoch_nanoseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    NativeResult::Ok(JsValue::float(get_double_prop(obj, 0)))
}

/// `Temporal.Instant.prototype.toString()`：输出 ISO 8601 UTC（如 `2024-01-01T00:00:00Z`）。
pub fn instant_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    match ns_to_datetime(get_double_prop(obj, 0)) {
        Some(dt) => {
            let s = dt.to_rfc3339_opts(SecondsFormat::AutoSi, true);
            NativeResult::Ok(vm.new_string(&s))
        }
        None => NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    }
}

/// `Temporal.Instant.prototype.valueOf()`：Temporal 对象禁止转原始值。
pub fn instant_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.Instant has no valueOf"))
}

fn make_plain_date<H: VmHost>(vm: &mut H, year: i32, month: u32, day: u32) -> NativeResult {
    let proto =
        JsValue::from_js_object(vm.session().builtin_world().plain_date_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_DATE;
    obj.set_prop_at(0, JsValue::float(year as f64));
    obj.set_prop_at(1, JsValue::float(month as f64));
    obj.set_prop_at(2, JsValue::float(day as f64));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

/// 校验 ISO 日期分量（month 1-12、day 按月份与闰年）。
fn valid_iso_date(year: i32, month: u32, day: u32) -> bool {
    NaiveDate::from_ymd_opt(year, month, day).is_some()
}

fn parse_plain_date_string(s: &str) -> Option<(i32, u32, u32)> {
    let t = s.trim();
    NaiveDate::parse_from_str(t, "%Y-%m-%d").ok().map(|d| {
        let (y, m, day) = (d.year(), d.month(), d.day());
        (y, m, day)
    })
}

/// `Temporal.PlainDate` 构造器：`new PlainDate(year, month, day)`。
pub fn plain_date_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().plain_date_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.PlainDate cannot be invoked without 'new'",
        ));
    }
    let year = if args.len() > 1 { to_number(vm.reg(args[1])).trunc() as i32 } else { 0 };
    let month = if args.len() > 2 { to_number(vm.reg(args[2])).trunc() as u32 } else { 1 };
    let day = if args.len() > 3 { to_number(vm.reg(args[3])).trunc() as u32 } else { 1 };
    if !valid_iso_date(year, month, day) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    make_plain_date(vm, year, month, day)
}

/// `Temporal.PlainDate.from(value)`：接受 `YYYY-MM-DD` 字符串或 `{year, month, day}` 对象。
pub fn plain_date_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (year, month, day) = if val.is_string() {
        let s = to_string(val);
        match parse_plain_date_string(&s) {
            Some(ymd) => ymd,
            None => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO 8601 date"));
            }
        }
    } else if val.is_object() && {
        let ptr = val.as_js_object_ptr();
        !ptr.is_null() && unsafe { &*ptr }.is_plain_date_obj()
    } {
        let obj = unsafe { &*val.as_js_object_ptr() };
        (
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
        )
    } else if val.is_object() {
        let obj = unsafe { &*val.as_js_object_ptr() };
        let y = read_prop_number(vm, obj, val, "year");
        let m = read_prop_number(vm, obj, val, "month");
        let d = read_prop_number(vm, obj, val, "day");
        if y.is_nan() || m.is_nan() || d.is_nan() {
            return NativeResult::Err(crate::error::create_type_error(vm, "cannot convert object to PlainDate"));
        }
        (y.trunc() as i32, m.trunc() as u32, d.trunc() as u32)
    } else {
        return NativeResult::Err(crate::error::create_type_error(vm, "cannot convert value to PlainDate"));
    };
    if !valid_iso_date(year, month, day) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    make_plain_date(vm, year, month, day)
}

fn plain_date_ymd<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(f64, f64, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_date(vm, obj)?;
    Ok((
        get_double_prop(obj, 0),
        get_double_prop(obj, 1),
        get_double_prop(obj, 2),
    ))
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

fn make_plain_time<H: VmHost>(vm: &mut H, hour: u32, minute: u32, second: u32, ms: u32, us: u32, ns: u32) -> NativeResult {
    let total_ns = hour as f64 * 3.6e12 + minute as f64 * 6e10 + second as f64 * 1e9 + ms as f64 * 1e6 + us as f64 * 1e3 + ns as f64;
    let proto =
        JsValue::from_js_object(vm.session().builtin_world().plain_time_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_TIME;
    obj.set_prop_at(0, JsValue::float(total_ns));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn valid_plain_time(hour: u32, minute: u32, second: u32, ms: u32, us: u32, ns: u32) -> bool {
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
    let get = |i: usize, default: f64| -> f64 {
        if args.len() > i {
            to_number(vm.reg(args[i])).trunc()
        } else {
            default
        }
    };
    let hour = get(1, 0.0) as u32;
    let minute = get(2, 0.0) as u32;
    let second = get(3, 0.0) as u32;
    let ms = get(4, 0.0) as u32;
    let us = get(5, 0.0) as u32;
    let ns = get(6, 0.0) as u32;
    if !valid_plain_time(hour, minute, second, ms, us, ns) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time component"));
    }
    make_plain_time(vm, hour, minute, second, ms, us, ns)
}

/// 拆解午夜后纳秒为各分量。
fn plain_time_components(total_ns: f64) -> (u32, u32, u32, u32, u32, u32) {
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

/// `Temporal.PlainTime.prototype.toString()`：`HH:MM:SS`，亚秒部分按需输出。
pub fn plain_time_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_time(vm, obj));
    let (h, m, s, ms, us, ns) = plain_time_components(get_double_prop(obj, 0));
    let frac = ms as u64 * 1_000_000 + us as u64 * 1_000 + ns as u64;
    if frac == 0 {
        NativeResult::Ok(vm.new_string(&format!("{h:02}:{m:02}:{s:02}")))
    } else {
        let mut digits = format!("{frac:09}");
        while digits.ends_with('0') {
            digits.pop();
        }
        NativeResult::Ok(vm.new_string(&format!("{h:02}:{m:02}:{s:02}.{digits}")))
    }
}
