use chrono::{DateTime, Datelike, Days, NaiveDate, SecondsFormat, Utc};

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

fn ensure_duration<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_duration_obj() {
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
    let epoch_ns = if args.len() < 2 { 0.0 } else { to_number(vm.reg(args[1])) };
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
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_date_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_DATE;
    obj.set_prop_at(0, JsValue::float(year as f64));
    obj.set_prop_at(1, JsValue::float(month as f64));
    obj.set_prop_at(2, JsValue::float(day as f64));
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

fn parse_duration_string(input: &str) -> Option<[f64; 10]> {
    let bytes = input.as_bytes();
    let mut cursor = 0usize;
    let negative = match bytes.first().copied() {
        Some(b'-') => {
            cursor += 1;
            true
        }
        Some(b'+') => {
            cursor += 1;
            false
        }
        _ => false,
    };
    if bytes.get(cursor) != Some(&b'P') {
        return None;
    }
    cursor += 1;

    let mut values = [0.0; 10];
    let mut in_time = false;
    let mut saw_unit = false;
    let mut saw_time_unit = false;
    let mut last_order = 0usize;
    let mut fraction_seen = false;
    while cursor < bytes.len() {
        if bytes[cursor] == b'T' {
            if in_time {
                return None;
            }
            in_time = true;
            cursor += 1;
            continue;
        }

        let number_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == number_start {
            return None;
        }
        let whole = input[number_start..cursor].parse::<f64>().ok()?;
        if !whole.is_finite() {
            return None;
        }

        let mut fraction = None;
        if matches!(bytes.get(cursor), Some(b'.' | b',')) {
            cursor += 1;
            let fraction_start = cursor;
            while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                cursor += 1;
            }
            let digits = cursor - fraction_start;
            if digits == 0 || digits > 9 {
                return None;
            }
            let numerator = input[fraction_start..cursor].parse::<u64>().ok()?;
            fraction = Some((numerator, 10_u64.pow(digits as u32)));
        }

        let unit = *bytes.get(cursor)?;
        cursor += 1;
        let (index, order, unit_nanos) = match (in_time, unit) {
            (false, b'Y') => (0, 1, None),
            (false, b'M') => (1, 2, None),
            (false, b'W') => (2, 3, None),
            (false, b'D') => (3, 4, None),
            (true, b'H') => (4, 5, Some(3_600_000_000_000_u64)),
            (true, b'M') => (5, 6, Some(60_000_000_000_u64)),
            (true, b'S') => (6, 7, Some(1_000_000_000_u64)),
            _ => return None,
        };
        if order <= last_order || (fraction_seen && (whole != 0.0 || fraction.is_some_and(|f| f.0 != 0))) {
            return None;
        }
        last_order = order;
        values[index] = whole;
        saw_unit = true;
        saw_time_unit |= in_time;

        if let Some((numerator, scale)) = fraction {
            let unit_nanos = unit_nanos?;
            let mut nanos = (numerator as u128 * unit_nanos as u128 / scale as u128) as u64;
            if index <= 4 {
                values[5] += (nanos / 60_000_000_000) as f64;
                nanos %= 60_000_000_000;
            }
            if index <= 5 {
                values[6] += (nanos / 1_000_000_000) as f64;
                nanos %= 1_000_000_000;
            }
            values[7] += (nanos / 1_000_000) as f64;
            nanos %= 1_000_000;
            values[8] += (nanos / 1_000) as f64;
            values[9] += (nanos % 1_000) as f64;
            fraction_seen = true;
        }
    }
    if !saw_unit || (in_time && !saw_time_unit) {
        return None;
    }
    if negative {
        for value in &mut values {
            if *value != 0.0 {
                *value = -*value;
            }
        }
    }
    Some(values)
}

fn duration_like_values<H: VmHost>(vm: &mut H, val: JsValue) -> Result<[f64; 10], JsValue> {
    if val.is_string() {
        return parse_duration_string(&to_string(val))
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid duration string"));
    }
    if !val.is_object() {
        return Err(crate::error::create_type_error(vm, "cannot convert value to Duration"));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "cannot convert value to Duration"));
    }
    let obj = unsafe { &*ptr };
    let names = [
        "years",
        "months",
        "weeks",
        "days",
        "hours",
        "minutes",
        "seconds",
        "milliseconds",
        "microseconds",
        "nanoseconds",
    ];
    let mut values = [0.0; 10];
    for (index, name) in names.iter().enumerate() {
        let value = if obj.is_duration_obj() {
            get_double_prop(obj, index)
        } else {
            read_prop_number(vm, obj, val, name)
        };
        if value.is_nan() {
            continue;
        }
        if !value.is_finite() || value.fract() != 0.0 {
            return Err(crate::error::create_range_error(vm, "invalid duration"));
        }
        values[index] = value;
    }
    Ok(values)
}

/// `Temporal.Duration` 构造器，保存十个整数时长分量。
pub fn duration_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().duration_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.Duration cannot be invoked without 'new'",
        ));
    }
    let mut values = [0.0; 10];
    for (index, value) in values.iter_mut().enumerate() {
        if args.len() <= index + 1 {
            continue;
        }
        let number = to_number(vm.reg(args[index + 1]));
        if !number.is_finite() || number.fract() != 0.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid duration"));
        }
        *value = number;
    }
    make_duration(vm, values)
}

/// `Temporal.Duration.from(value)`，复制 Duration 或读取同名字段。
pub fn duration_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = native_try!(duration_like_values(vm, val));
    make_duration(vm, values)
}

fn duration_values(obj: &JsObject) -> [f64; 10] {
    std::array::from_fn(|index| get_double_prop(obj, index))
}

fn format_duration_number(value: f64) -> String {
    format!("{}", value as i64)
}

fn format_duration_seconds(seconds: f64, milliseconds: f64, microseconds: f64, nanoseconds: f64) -> Option<String> {
    let subsecond = milliseconds * 1_000_000.0 + microseconds * 1_000.0 + nanoseconds;
    if seconds == 0.0 && subsecond == 0.0 {
        return None;
    }
    let negative = seconds < 0.0 || (seconds == 0.0 && subsecond < 0.0);
    let whole = seconds.abs() as i64;
    let fraction = subsecond.abs() as u64;
    let mut result = format!("{}", whole);
    if fraction != 0 {
        let mut digits = format!("{fraction:09}");
        while digits.ends_with('0') {
            digits.pop();
        }
        result.push('.');
        result.push_str(&digits);
    }
    if negative {
        result.insert(0, '-');
    }
    Some(result)
}

/// `Temporal.Duration.prototype.toString()` 的 ISO 8601 表示。
pub fn duration_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let mut values = duration_values(obj);
    let negative = values.iter().any(|value| *value < 0.0);
    for value in &mut values {
        *value = value.abs();
    }
    let mut output = String::from(if negative { "-P" } else { "P" });
    let date_units = [(0, "Y"), (1, "M"), (2, "W"), (3, "D")];
    let mut has_date = false;
    for (index, suffix) in date_units {
        if values[index] != 0.0 {
            has_date = true;
            output.push_str(&format_duration_number(values[index]));
            output.push_str(suffix);
        }
    }
    let time_units = [(4, "H"), (5, "M")];
    let has_time = values[4..].iter().any(|value| *value != 0.0);
    if has_time || !has_date {
        output.push('T');
        for (index, suffix) in time_units {
            if values[index] != 0.0 {
                output.push_str(&format_duration_number(values[index]));
                output.push_str(suffix);
            }
        }
        if let Some(seconds) = format_duration_seconds(values[6], values[7], values[8], values[9]) {
            output.push_str(&seconds);
            output.push('S');
        } else if output.ends_with('T') {
            output.push_str("0S");
        }
    }
    NativeResult::Ok(vm.new_string(&output))
}

/// `Temporal.Duration.prototype.valueOf()` 始终拒绝隐式数值转换。
pub fn duration_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.Duration has no valueOf"))
}

/// `Temporal.Duration.prototype.abs()`，返回所有分量的绝对值副本。
pub fn duration_abs<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let mut values = duration_values(obj);
    for value in &mut values {
        *value = value.abs();
    }
    make_duration(vm, values)
}

/// `Temporal.Duration.prototype.negated()`，返回非零分量取反的副本。
pub fn duration_negated<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let mut values = duration_values(obj);
    for value in &mut values {
        if *value != 0.0 {
            *value = -*value;
        }
    }
    make_duration(vm, values)
}

macro_rules! duration_getter {
    ($name:ident, $index:expr) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let ptr = native_try!(receiver_obj(vm, args));
            let obj = unsafe { &*ptr };
            native_try!(ensure_duration(vm, obj));
            NativeResult::Ok(JsValue::float(get_double_prop(obj, $index)))
        }
    };
}

duration_getter!(duration_years, 0);
duration_getter!(duration_months, 1);
duration_getter!(duration_weeks, 2);
duration_getter!(duration_days, 3);
duration_getter!(duration_hours, 4);
duration_getter!(duration_minutes, 5);
duration_getter!(duration_seconds, 6);
duration_getter!(duration_milliseconds, 7);
duration_getter!(duration_microseconds, 8);
duration_getter!(duration_nanoseconds, 9);

/// 校验 ISO 日期分量（month 1-12、day 按月份与闰年）。
fn valid_iso_date(year: i32, month: u32, day: u32) -> bool {
    NaiveDate::from_ymd_opt(year, month, day).is_some()
}

/// 读两位数字（basic 格式的月/日等紧凑分量）。
fn read_iso2(t: &str, i: &mut usize, bytes: &[u8]) -> Result<u32, String> {
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
fn parse_iso_date(s: &str) -> Result<(i32, u32, u32), String> {
    let t = s.trim().replace('\u{2212}', "-");
    if t.is_empty() {
        return Err("invalid ISO string".into());
    }
    let bytes = t.as_bytes();
    let mut i = 0usize;
    let negative = if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
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
    if year_digits.len() < 4 || year_digits.len() > 6 {
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
        let ok = rest.starts_with(['T', 't', '[', '+', '-', 'Z', 'z']);
        if !ok {
            return Err("invalid trailing content".into());
        }
    }
    if month == 0 || month > 12 || day == 0 {
        return Err("invalid ISO date".into());
    }
    if year == 0 {
        return Err("invalid ISO year zero".into());
    }
    Ok((year as i32, month, day))
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

/// `Temporal.PlainDate.from(value)`：接受 ISO 日期字符串或 `{year, month, day}` 对象。
pub fn plain_date_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (year, month, day) = if val.is_string() {
        let s = to_string(val);
        match parse_iso_date(&s) {
            Ok(ymd) => ymd,
            Err(_) => {
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

fn make_plain_time<H: VmHost>(
    vm: &mut H, hour: u32, minute: u32, second: u32, ms: u32, us: u32, ns: u32,
) -> NativeResult {
    let total_ns = hour as f64 * 3.6e12
        + minute as f64 * 6e10
        + second as f64 * 1e9
        + ms as f64 * 1e6
        + us as f64 * 1e3
        + ns as f64;
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_time_proto.as_ptr() as *mut JsObject);
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

// ───────────────────── PlainDate 扩展方法 ─────────────────────

/// 从 PlainDate receiver 读 year/month/day 并构造 chrono NaiveDate（供日期计算）。
fn plain_date_naive<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<NaiveDate, JsValue> {
    let (y, m, d) = plain_date_ymd(vm, args)?;
    NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32)
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid date"))
}

/// 从对象式日期字段（`{year, month, day}`）读三字段；PlainDate 对象直接读内部槽；
/// 缺字段返回 None。
fn object_ymd<H: VmHost>(vm: &mut H, val: JsValue) -> Option<(i32, u32, u32)> {
    if !val.is_object() {
        return None;
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return None;
    }
    let obj = unsafe { &*ptr };
    if obj.is_plain_date_obj() {
        return Some((
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
        ));
    }
    let y = read_prop_number(vm, obj, val, "year");
    let m = read_prop_number(vm, obj, val, "month");
    let d = read_prop_number(vm, obj, val, "day");
    if y.is_nan() || m.is_nan() || d.is_nan() {
        return None;
    }
    Some((y.trunc() as i32, m.trunc() as u32, d.trunc() as u32))
}

fn date_like_ymd<H: VmHost>(vm: &mut H, val: JsValue) -> Result<(i32, u32, u32), JsValue> {
    let ymd = if val.is_string() {
        parse_iso_date(&to_string(val)).map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 date"))?
    } else {
        object_ymd(vm, val).ok_or_else(|| crate::error::create_type_error(vm, "cannot convert to PlainDate"))?
    };
    if !valid_iso_date(ymd.0, ymd.1, ymd.2) {
        return Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    Ok(ymd)
}

fn read_prop_text<H: VmHost>(vm: &mut H, obj: &JsObject, receiver: JsValue, name: &str) -> Option<String> {
    let key_val = vm.new_string(name);
    let si = vm.property_key_si(key_val);
    vm.ordinary_get(obj, si, receiver)
        .ok()
        .filter(|value| value.is_string())
        .map(to_string)
}

fn largest_unit<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<String, JsValue> {
    if args.len() <= 2 {
        return Ok("days".to_string());
    }
    let options = vm.reg(args[2]);
    if options.is_nullish() {
        return Ok("days".to_string());
    }
    if !options.is_object() {
        return Err(crate::error::create_type_error(vm, "options must be an object"));
    }
    let ptr = options.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "options must be an object"));
    }
    let unit = read_prop_text(vm, unsafe { &*ptr }, options, "largestUnit").unwrap_or_else(|| "days".to_string());
    let unit = unit.to_ascii_lowercase();
    let unit = unit.strip_suffix('s').unwrap_or(&unit);
    match unit {
        "year" | "month" | "week" | "day" => Ok(unit.to_string()),
        "auto" => Ok("day".to_string()),
        _ => Err(crate::error::create_range_error(vm, "invalid largestUnit")),
    }
}

fn date_difference(start: NaiveDate, end: NaiveDate, unit: &str) -> [f64; 10] {
    let total_days = end.signed_duration_since(start).num_days();
    if unit == "day" {
        return [0.0, 0.0, 0.0, total_days as f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    }
    if unit == "week" {
        return [0.0, 0.0, (total_days / 7) as f64, (total_days % 7) as f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    }

    let direction = if end >= start { 1_i64 } else { -1_i64 };
    let mut total_months = (end.year() as i64 - start.year() as i64) * 12 + end.month0() as i64 - start.month0() as i64;
    let mut candidate = add_signed_months(start, total_months).unwrap_or(start);
    if (direction > 0 && candidate > end) || (direction < 0 && candidate < end) {
        total_months -= direction;
        candidate = add_signed_months(start, total_months).unwrap_or(start);
    }
    let remainder_days = end.signed_duration_since(candidate).num_days();
    let (years, months) = if unit == "year" {
        (total_months / 12, total_months % 12)
    } else {
        (0, total_months)
    };
    [years as f64, months as f64, 0.0, remainder_days as f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
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
fn days_in_month_iso(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(year, month + 1, 1)
        .and_then(|next| next.checked_sub_days(Days::new(1)))
        .map(|d| d.day())
        .unwrap_or(31)
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
    let _ = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::float(7.0))
}

fn is_leap_year_iso(year: i32) -> bool {
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
    let _ = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(e) => return NativeResult::Err(e),
    };
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
    let _ = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::undefined())
}

/// `Temporal.PlainDate.prototype.eraYear` getter：ISO 日历无纪元，恒 undefined。
pub fn plain_date_era_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(JsValue::undefined())
}

/// `Temporal.PlainDate.prototype.calendarId` getter：恒 `"iso8601"`。
pub fn plain_date_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(e) => return NativeResult::Err(e),
    };
    NativeResult::Ok(vm.new_string("iso8601"))
}

/// `Temporal.PlainDate.prototype.equals(other)`：比较年月日是否相等。
pub fn plain_date_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (y, m, d) = match plain_date_ymd(vm, args) {
        Ok(x) => x,
        Err(e) => return NativeResult::Err(e),
    };
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let equal = match object_ymd(vm, other_val) {
        Some((oy, om, od)) => oy as f64 == y && om as f64 == m && od as f64 == d,
        None => false,
    };
    NativeResult::Ok(JsValue::bool(equal))
}

/// `Temporal.PlainDate.compare(a, b)`：静态比较，返回 -1/0/1。
pub fn plain_date_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let a_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let b_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let a = match object_ymd(vm, a_val) {
        Some(x) => x,
        None => {
            return NativeResult::Err(crate::error::create_type_error(vm, "cannot convert to PlainDate"));
        }
    };
    let b = match object_ymd(vm, b_val) {
        Some(x) => x,
        None => {
            return NativeResult::Err(crate::error::create_type_error(vm, "cannot convert to PlainDate"));
        }
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
    make_plain_date(vm, result.year(), result.month(), result.day())
}

/// `Temporal.PlainDate.prototype.add(durationLike)`。
pub fn plain_date_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_apply_duration(vm, args, 1)
}

/// `Temporal.PlainDate.prototype.subtract(durationLike)`。
pub fn plain_date_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    date_apply_duration(vm, args, -1)
}

fn add_signed_months(date: NaiveDate, months: i64) -> Option<NaiveDate> {
    let total = date.year() as i64 * 12 + date.month0() as i64 + months;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) as u32 + 1;
    let year = i32::try_from(year).ok()?;
    let day = date.day().min(days_in_month_iso(year, month));
    NaiveDate::from_ymd_opt(year, month, day)
}

/// `Temporal.PlainDate.prototype.until(other)`，按默认天单位返回差值。
pub fn plain_date_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let start = match plain_date_naive(vm, args) {
        Ok(date) => date,
        Err(error) => return NativeResult::Err(error),
    };
    let other_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (year, month, day) = match date_like_ymd(vm, other_value) {
        Ok(value) => value,
        Err(error) => return NativeResult::Err(error),
    };
    let other = NaiveDate::from_ymd_opt(year, month, day).expect("date_like_ymd validates ISO date");
    let unit = match largest_unit(vm, args) {
        Ok(unit) => unit,
        Err(error) => return NativeResult::Err(error),
    };
    make_duration(vm, date_difference(start, other, &unit))
}

/// `Temporal.PlainDate.prototype.since(other)`，按默认天单位返回差值。
pub fn plain_date_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let start = match plain_date_naive(vm, args) {
        Ok(date) => date,
        Err(error) => return NativeResult::Err(error),
    };
    let other_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (year, month, day) = match date_like_ymd(vm, other_value) {
        Ok(value) => value,
        Err(error) => return NativeResult::Err(error),
    };
    let other = NaiveDate::from_ymd_opt(year, month, day).expect("date_like_ymd validates ISO date");
    let unit = match largest_unit(vm, args) {
        Ok(unit) => unit,
        Err(error) => return NativeResult::Err(error),
    };
    let mut values = date_difference(start, other, &unit);
    for value in &mut values {
        if *value != 0.0 {
            *value = -*value;
        }
    }
    make_duration(vm, values)
}
