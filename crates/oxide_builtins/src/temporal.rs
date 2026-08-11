use chrono::{Datelike, Days, NaiveDate, Utc};

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

// Temporal 命名空间的最小实现子集：Temporal.Now / Temporal.Instant /
// Temporal.PlainDate / Temporal.PlainTime / Temporal.PlainDateTime / Temporal.ZonedDateTime。
// 内部数据按对象类型存入 prop 槽：
// Instant 存纪元纳秒（BigInt，prop 0）、PlainDate 存年/月/日（prop 0-2）、
// PlainTime 存午夜后纳秒（f64，prop 0）、PlainDateTime 存年/月/日与午夜后纳秒（prop 0-3），
// ZonedDateTime 存纪元纳秒、时区 ID、日历 ID（prop 0-2）。

const MAX_INSTANT_NS: i128 = 8_640_000_000_000_000_000_000;

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

fn get_instant_epoch_ns(obj: &JsObject) -> Option<i128> {
    let value = obj.get_prop_at(0);
    if value.is_bigint() {
        return Some(unsafe { oxide_runtime_api::bigint_data(value) });
    }
    if value.is_int() || value.is_double() {
        return Some(to_number(value) as i128);
    }
    None
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

fn ensure_plain_date_time<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_date_time_obj() {
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

fn ensure_zoned_date_time<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_zoned_date_time_obj() {
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

fn initialize_temporal_receiver<H: VmHost, const N: usize>(
    vm: &mut H, args: &[u8], type_tag: u8, values: [JsValue; N],
) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &mut *ptr };
    obj.type_tag = type_tag;
    for (index, value) in values.into_iter().enumerate() {
        obj.set_prop_at(index, value);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// 读 receiver 是否为构造调用：`new` 时 this 的原型链包含相应构造器的 prototype。
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
    let mut this_proto = obj.proto();
    while this_proto.is_object() {
        let this_proto_ptr = this_proto.as_js_object_ptr();
        if this_proto_ptr.is_null() {
            return false;
        }
        if std::ptr::eq(this_proto_ptr, proto_ptr) {
            return true;
        }
        this_proto = unsafe { &*this_proto_ptr }.proto();
    }
    false
}

fn make_instant<H: VmHost>(vm: &mut H, epoch_ns: i128) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().instant_proto.as_ptr() as *mut JsObject);
    let epoch_value = vm.new_bigint(epoch_ns);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_INSTANT;
    obj.set_prop_at(0, epoch_value);
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn make_zoned_date_time<H: VmHost>(vm: &mut H, epoch_ns: i128, time_zone_id: &str) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().zoned_date_time_proto.as_ptr() as *mut JsObject);
    let epoch_value = vm.new_bigint(epoch_ns);
    let time_zone_value = vm.new_string(time_zone_id);
    let calendar_value = vm.new_string("iso8601");
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_ZONED_DATE_TIME;
    obj.set_prop_at(0, epoch_value);
    obj.set_prop_at(1, time_zone_value);
    obj.set_prop_at(2, calendar_value);
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn parse_digits(bytes: &[u8], cursor: &mut usize, count: usize) -> Option<i128> {
    let end = cursor.checked_add(count)?;
    let digits = bytes.get(*cursor..end)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    *cursor = end;
    digits
        .iter()
        .try_fold(0_i128, |value, digit| value.checked_mul(10)?.checked_add((digit - b'0') as i128))
}

fn is_leap_year(year: i128) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

fn days_in_month(year: i128, month: i128) -> Option<i128> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if is_leap_year(year) { 29 } else { 28 }),
        _ => None,
    }
}

fn days_from_civil(mut year: i128, month: i128, day: i128) -> i128 {
    year -= i128::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn instant_string_without_annotations(input: &str) -> Option<&str> {
    let Some(first_annotation) = input.find('[') else {
        return Some(input);
    };
    let mut rest = &input[first_annotation..];
    let mut saw_calendar = false;
    let mut saw_critical_calendar = false;
    let mut saw_time_zone = false;
    while !rest.is_empty() {
        let body_start = rest.strip_prefix('[')?;
        let close = body_start.find(']')?;
        let body = &body_start[..close];
        rest = &body_start[close + 1..];
        if body.is_empty() || (!rest.is_empty() && !rest.starts_with('[')) {
            return None;
        }
        let (critical, annotation) = match body.strip_prefix('!') {
            Some(value) => (true, value),
            None => (false, body),
        };
        if let Some((key, _value)) = annotation.split_once('=') {
            if key.is_empty()
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
            {
                return None;
            }
            if key == "u-ca" {
                if saw_calendar && (critical || saw_critical_calendar) {
                    return None;
                }
                saw_calendar = true;
                saw_critical_calendar |= critical;
            } else if critical {
                return None;
            }
        } else {
            if saw_time_zone || annotation.is_empty() {
                return None;
            }
            if matches!(annotation.as_bytes().first(), Some(b'+' | b'-')) {
                let bytes = annotation.as_bytes();
                let hour_only = bytes.len() == 3 && bytes[1..3].iter().all(u8::is_ascii_digit);
                let hour_minute = bytes.len() == 6
                    && bytes[3] == b':'
                    && bytes[1..3].iter().all(u8::is_ascii_digit)
                    && bytes[4..6].iter().all(u8::is_ascii_digit);
                if !hour_only && !hour_minute {
                    return None;
                }
                let hour = (bytes[1] - b'0') * 10 + bytes[2] - b'0';
                let minute = if hour_minute { (bytes[4] - b'0') * 10 + bytes[5] - b'0' } else { 0 };
                if hour > 23 || minute > 59 {
                    return None;
                }
            }
            saw_time_zone = true;
        }
    }
    Some(&input[..first_annotation])
}

/// 解析带 UTC 标识或数值偏移的 Temporal ISO 日期时间到纪元纳秒。
///
/// # 边界与前提
/// - 支持扩展年份、compact 格式、annotation 和亚分钟 offset。
/// - 闰秒按前一秒解释；超出 Instant 范围或语法无效时返回 `None`。
fn parse_instant_string(s: &str) -> Option<i128> {
    let input = s.trim();
    let bytes = instant_string_without_annotations(input)?.as_bytes();

    // 解析公历日期；带符号年份固定为六位，负零扩展年份无效。
    let mut cursor = 0usize;
    let negative_extended_year = bytes.first() == Some(&b'-');
    let year_sign = match bytes.first().copied() {
        Some(b'+') => {
            cursor += 1;
            1_i128
        }
        Some(b'-') => {
            cursor += 1;
            -1_i128
        }
        _ => 1_i128,
    };
    let year_digits = if cursor == 0 { 4 } else { 6 };
    let year_magnitude = parse_digits(bytes, &mut cursor, year_digits)?;
    if negative_extended_year && year_magnitude == 0 {
        return None;
    }
    let year = year_sign * year_magnitude;
    let separated_date = bytes.get(cursor) == Some(&b'-');
    if separated_date {
        cursor += 1;
    }
    let month = parse_digits(bytes, &mut cursor, 2)?;
    if separated_date {
        if bytes.get(cursor) != Some(&b'-') {
            return None;
        }
        cursor += 1;
    }
    let day = parse_digits(bytes, &mut cursor, 2)?;

    // 时间允许小时、分钟或秒精度，日期与时间可用 T、t 或空格分隔。
    if !matches!(bytes.get(cursor), Some(b'T' | b't' | b' ')) {
        return None;
    }
    cursor += 1;
    let hour = parse_digits(bytes, &mut cursor, 2)?;
    let colon_time = bytes.get(cursor) == Some(&b':');
    let minute = if colon_time {
        cursor += 1;
        parse_digits(bytes, &mut cursor, 2)?
    } else if matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
        parse_digits(bytes, &mut cursor, 2)?
    } else {
        0
    };
    let (second, has_second) = if bytes.get(cursor) == Some(&b':') {
        cursor += 1;
        (parse_digits(bytes, &mut cursor, 2)?, true)
    } else if !colon_time && matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
        (parse_digits(bytes, &mut cursor, 2)?, true)
    } else {
        (0, false)
    };

    let mut subsecond_ns = 0_i128;
    if matches!(bytes.get(cursor), Some(b'.' | b',')) {
        if !has_second {
            return None;
        }
        cursor += 1;
        let fraction_start = cursor;
        while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
            cursor += 1;
        }
        let fraction_len = cursor - fraction_start;
        if fraction_len == 0 || fraction_len > 9 {
            return None;
        }
        let mut fraction_cursor = fraction_start;
        subsecond_ns = parse_digits(bytes, &mut fraction_cursor, fraction_len)?;
        subsecond_ns *= 10_i128.pow((9 - fraction_len) as u32);
    }

    // 数值 offset 支持 basic/extended 形式及秒以下精度。
    let offset_ns = match bytes.get(cursor).copied() {
        Some(b'Z' | b'z') => {
            cursor += 1;
            0_i128
        }
        Some(sign @ (b'+' | b'-')) => {
            cursor += 1;
            let offset_hour = parse_digits(bytes, &mut cursor, 2)?;
            let colon_format = bytes.get(cursor) == Some(&b':');
            let offset_minute = if colon_format {
                cursor += 1;
                parse_digits(bytes, &mut cursor, 2)?
            } else if matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
                parse_digits(bytes, &mut cursor, 2)?
            } else {
                0
            };
            let (offset_second, has_offset_second) = if colon_format && bytes.get(cursor) == Some(&b':') {
                cursor += 1;
                (parse_digits(bytes, &mut cursor, 2)?, true)
            } else if !colon_format && matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
                (parse_digits(bytes, &mut cursor, 2)?, true)
            } else {
                (0, false)
            };
            let mut offset_subsecond_ns = 0_i128;
            if matches!(bytes.get(cursor), Some(b'.' | b',')) {
                if !has_offset_second {
                    return None;
                }
                cursor += 1;
                let fraction_start = cursor;
                while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
                    cursor += 1;
                }
                let fraction_len = cursor - fraction_start;
                if fraction_len == 0 || fraction_len > 9 {
                    return None;
                }
                let mut fraction_cursor = fraction_start;
                offset_subsecond_ns = parse_digits(bytes, &mut fraction_cursor, fraction_len)?;
                offset_subsecond_ns *= 10_i128.pow((9 - fraction_len) as u32);
            }
            if offset_hour > 23 || offset_minute > 59 || offset_second > 59 {
                return None;
            }
            let magnitude =
                (offset_hour * 3_600 + offset_minute * 60 + offset_second) * 1_000_000_000 + offset_subsecond_ns;
            if sign == b'-' {
                -magnitude
            } else {
                magnitude
            }
        }
        _ => return None,
    };
    if cursor != bytes.len() || hour > 23 || minute > 59 || second > 60 || day == 0 || day > days_in_month(year, month)?
    {
        return None;
    }

    // 使用 proleptic Gregorian 日数精确换算，并在纳秒层应用 offset。
    let epoch_seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second.min(59))?;
    let epoch_ns = epoch_seconds
        .checked_mul(1_000_000_000)?
        .checked_add(subsecond_ns)?
        .checked_sub(offset_ns)?;
    (epoch_ns.unsigned_abs() <= MAX_INSTANT_NS as u128).then_some(epoch_ns)
}

fn native_engine_error<H: VmHost>(vm: &mut H, error: &str) -> JsValue {
    vm.take_uncaught_value()
        .unwrap_or_else(|| crate::error::create_from_text(vm, error))
}

fn primitive_to_bigint<H: VmHost>(vm: &mut H, raw: JsValue) -> Result<i128, JsValue> {
    let primitive = oxide_runtime_api::to_primitive(raw, oxide_runtime_api::ToPrimitiveHint::Number, vm)
        .map_err(|error| native_engine_error(vm, &error))?;
    if primitive.is_bigint() {
        return Ok(vm.bigint_value(primitive));
    }
    if primitive.is_bool() {
        return Ok(i128::from(primitive.as_bool()));
    }
    if primitive.is_string() {
        let input = to_string(primitive);
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(0);
        }
        return trimmed.parse::<i128>().map_err(|_| {
            crate::error::create_syntax_error(vm, "Cannot convert string to BigInt: invalid integer literal")
        });
    }
    Err(crate::error::create_type_error(vm, "Cannot convert value to a BigInt"))
}

fn instant_like_epoch_ns<H: VmHost>(vm: &mut H, value: JsValue) -> Result<i128, JsValue> {
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_instant_obj() {
                return get_instant_epoch_ns(obj)
                    .ok_or_else(|| crate::error::create_range_error(vm, "invalid Instant"));
            }
            if obj.is_function() {
                return Err(crate::error::create_range_error(vm, "invalid ISO 8601 string"));
            }
        }
    }
    let input = oxide_runtime_api::to_string_full(value, vm).map_err(|error| native_engine_error(vm, &error))?;
    parse_instant_string(&input).ok_or_else(|| crate::error::create_range_error(vm, "invalid ISO 8601 string"))
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

/// `Temporal.Instant` 构造器：把参数按 ToBigInt 转换为纪元纳秒。
pub fn instant_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().instant_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.Instant cannot be invoked without 'new'",
        ));
    }
    let raw = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let epoch_ns = native_try!(primitive_to_bigint(vm, raw));
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    let epoch_value = vm.new_bigint(epoch_ns);
    initialize_temporal_receiver(vm, args, JsObject::OBJ_TYPE_INSTANT, [epoch_value])
}

/// `Temporal.Instant.from(value)`：接受 Instant、ISO 字符串或可转换为字符串的对象。
pub fn instant_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let epoch_ns = native_try!(instant_like_epoch_ns(vm, val));
    make_instant(vm, epoch_ns)
}

/// `Temporal.Instant.fromEpochMilliseconds(epochMilliseconds)`：从整数毫秒创建 Instant。
pub fn instant_from_epoch_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let raw = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let primitive = match oxide_runtime_api::to_primitive(raw, oxide_runtime_api::ToPrimitiveHint::Number, vm) {
        Ok(value) => value,
        Err(error) => return NativeResult::Err(native_engine_error(vm, &error)),
    };
    if primitive.is_bigint() || primitive.is_symbol() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a number"));
    }
    let epoch_ms = to_number(primitive);
    if !epoch_ms.is_finite() || epoch_ms.fract() != 0.0 || epoch_ms.abs() > 8_640_000_000_000_000.0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid epoch milliseconds"));
    }
    make_instant(vm, (epoch_ms as i128) * 1_000_000)
}

/// `Temporal.Instant.fromEpochNanoseconds(epochNanoseconds)`：从 BigInt 纳秒创建 Instant。
pub fn instant_from_epoch_nanoseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let raw = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let epoch_ns = native_try!(primitive_to_bigint(vm, raw));
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    make_instant(vm, epoch_ns)
}

/// `Temporal.Instant.compare(one, two)`：按纪元纳秒返回 -1、0 或 1。
pub fn instant_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let one = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let two = if args.len() < 3 { JsValue::undefined() } else { vm.reg(args[2]) };
    let one_ns = native_try!(instant_like_epoch_ns(vm, one));
    let two_ns = native_try!(instant_like_epoch_ns(vm, two));
    let result = match one_ns.cmp(&two_ns) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    NativeResult::Ok(JsValue::int(result))
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
    let epoch_ns = match get_instant_epoch_ns(obj) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    };
    NativeResult::Ok(JsValue::float(epoch_ns.div_euclid(1_000_000_000) as f64))
}

/// `Temporal.Instant.prototype.epochMilliseconds` getter。
pub fn instant_epoch_milliseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let epoch_ns = match get_instant_epoch_ns(obj) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    };
    NativeResult::Ok(JsValue::float(epoch_ns.div_euclid(1_000_000) as f64))
}

/// `Temporal.Instant.prototype.epochMicroseconds` getter。
pub fn instant_epoch_microseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let epoch_ns = match get_instant_epoch_ns(obj) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    };
    NativeResult::Ok(JsValue::float(epoch_ns.div_euclid(1_000) as f64))
}

/// `Temporal.Instant.prototype.epochNanoseconds` getter：返回精确 BigInt。
pub fn instant_epoch_nanoseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let value = obj.get_prop_at(0);
    if value.is_bigint() {
        NativeResult::Ok(value)
    } else {
        match get_instant_epoch_ns(obj) {
            Some(epoch_ns) => NativeResult::Ok(vm.new_bigint(epoch_ns)),
            None => NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
        }
    }
}

/// `Temporal.Instant.prototype.equals(other)`：比较两个 Instant 的纪元纳秒。
pub fn instant_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };

    let other = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other_ns = native_try!(instant_like_epoch_ns(vm, other));
    NativeResult::Ok(JsValue::bool(epoch_ns == other_ns))
}

fn instant_add_duration<H: VmHost>(vm: &mut H, args: &[u8], direction: i128) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };

    let duration_like = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let values = native_try!(duration_like_values(vm, duration_like));
    if values[..4].iter().any(|value| *value != 0.0) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "Instant arithmetic does not support date units",
        ));
    }
    let Some(delta_ns) = duration_time_nanoseconds(&values) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid duration"));
    };
    let Some(result_ns) = delta_ns.checked_mul(direction).and_then(|delta| epoch_ns.checked_add(delta)) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    };
    if result_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    make_instant(vm, result_ns)
}

/// `Temporal.Instant.prototype.add(durationLike)`：精确增加仅含时间单位的时长。
pub fn instant_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_add_duration(vm, args, 1)
}

/// `Temporal.Instant.prototype.subtract(durationLike)`：精确减去仅含时间单位的时长。
pub fn instant_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_add_duration(vm, args, -1)
}

#[derive(Clone, Copy)]
enum InstantRoundingMode {
    Ceil,
    Expand,
    Floor,
    HalfCeil,
    HalfEven,
    HalfExpand,
    HalfFloor,
    HalfTrunc,
    Trunc,
}

fn temporal_option_value<H: VmHost>(
    vm: &mut H, obj: &JsObject, receiver: JsValue, name: &str,
) -> Result<JsValue, JsValue> {
    let key = vm.new_string(name);
    let si = vm.property_key_si(key);
    vm.ordinary_get(obj, si, receiver)
        .map_err(|error| native_engine_error(vm, &error))
}

fn temporal_option_number<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    let primitive = oxide_runtime_api::to_primitive(value, oxide_runtime_api::ToPrimitiveHint::Number, vm)
        .map_err(|error| native_engine_error(vm, &error))?;
    if primitive.is_symbol() || primitive.is_bigint() {
        return Err(crate::error::create_type_error(vm, "cannot convert option to number"));
    }
    Ok(to_number(primitive))
}

fn temporal_option_string<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
    let primitive = oxide_runtime_api::to_primitive(value, oxide_runtime_api::ToPrimitiveHint::String, vm)
        .map_err(|error| native_engine_error(vm, &error))?;
    if primitive.is_symbol() {
        return Err(crate::error::create_type_error(vm, "cannot convert option to string"));
    }
    Ok(to_string(primitive))
}

fn instant_rounding_mode(value: &str) -> Option<InstantRoundingMode> {
    match value {
        "ceil" => Some(InstantRoundingMode::Ceil),
        "expand" => Some(InstantRoundingMode::Expand),
        "floor" => Some(InstantRoundingMode::Floor),
        "halfCeil" => Some(InstantRoundingMode::HalfCeil),
        "halfEven" => Some(InstantRoundingMode::HalfEven),
        "halfExpand" => Some(InstantRoundingMode::HalfExpand),
        "halfFloor" => Some(InstantRoundingMode::HalfFloor),
        "halfTrunc" => Some(InstantRoundingMode::HalfTrunc),
        "trunc" => Some(InstantRoundingMode::Trunc),
        _ => None,
    }
}

fn instant_round_unit(value: &str) -> Option<(i128, i128)> {
    match value {
        "hour" | "hours" => Some((3_600_000_000_000, 24)),
        "minute" | "minutes" => Some((60_000_000_000, 1_440)),
        "second" | "seconds" => Some((1_000_000_000, 86_400)),
        "millisecond" | "milliseconds" => Some((1_000_000, 86_400_000)),
        "microsecond" | "microseconds" => Some((1_000, 86_400_000_000)),
        "nanosecond" | "nanoseconds" => Some((1, 86_400_000_000_000)),
        _ => None,
    }
}

/// 按 Temporal 的 `RoundNumberToIncrementAsIfPositive` 语义舍入纳秒。
fn round_instant_ns(value: i128, increment: i128, mode: InstantRoundingMode) -> Option<i128> {
    let quotient = value.div_euclid(increment);
    let remainder = value.rem_euclid(increment);
    if remainder == 0 {
        return Some(value);
    }
    let use_upper = match mode {
        InstantRoundingMode::Ceil | InstantRoundingMode::Expand => true,
        InstantRoundingMode::Floor | InstantRoundingMode::Trunc => false,
        _ => match (remainder * 2).cmp(&increment) {
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => match mode {
                InstantRoundingMode::HalfCeil | InstantRoundingMode::HalfExpand => true,
                InstantRoundingMode::HalfFloor | InstantRoundingMode::HalfTrunc => false,
                InstantRoundingMode::HalfEven => quotient.rem_euclid(2) != 0,
                _ => unreachable!(),
            },
        },
    };
    quotient
        .checked_add(i128::from(use_upper))
        .and_then(|rounded| rounded.checked_mul(increment))
}

/// `Temporal.Instant.prototype.round(roundTo)`：按给定单位、增量和模式精确舍入。
pub fn instant_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };

    let round_to = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (increment_value, mode_value, unit_value) = if round_to.is_string() {
        (1.0, "halfExpand".to_string(), Some(to_string(round_to)))
    } else {
        if !round_to.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "roundTo must be a string or object"));
        }
        let options_ptr = round_to.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "roundTo must be a string or object"));
        }
        let options = unsafe { &*options_ptr };
        let increment_raw = native_try!(temporal_option_value(vm, options, round_to, "roundingIncrement"));
        let increment = if increment_raw.is_undefined() {
            1.0
        } else {
            native_try!(temporal_option_number(vm, increment_raw))
        };
        let mode_raw = native_try!(temporal_option_value(vm, options, round_to, "roundingMode"));
        let mode = if mode_raw.is_undefined() {
            "halfExpand".to_string()
        } else {
            native_try!(temporal_option_string(vm, mode_raw))
        };
        let unit_raw = native_try!(temporal_option_value(vm, options, round_to, "smallestUnit"));
        let unit = if unit_raw.is_undefined() {
            None
        } else {
            Some(native_try!(temporal_option_string(vm, unit_raw)))
        };
        (increment, mode, unit)
    };

    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
    };
    let Some(unit_value) = unit_value else {
        return NativeResult::Err(crate::error::create_range_error(vm, "smallestUnit is required"));
    };
    let Some((unit_ns, units_per_day)) = instant_round_unit(&unit_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid smallest unit"));
    };
    if !increment_value.is_finite() {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment_value.trunc();
    if !(1.0..=1_000_000_000.0).contains(&increment) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment as i128;
    if units_per_day % increment != 0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "rounding increment must divide a day"));
    }
    let Some(quantum_ns) = unit_ns.checked_mul(increment) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    };
    let Some(rounded_ns) = round_instant_ns(epoch_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    };
    if rounded_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    make_instant(vm, rounded_ns)
}

fn instant_difference_unit(value: &str) -> Option<(usize, i128, i128)> {
    match value {
        "hour" | "hours" => Some((0, 3_600_000_000_000, 24)),
        "minute" | "minutes" => Some((1, 60_000_000_000, 60)),
        "second" | "seconds" => Some((2, 1_000_000_000, 60)),
        "millisecond" | "milliseconds" => Some((3, 1_000_000, 1_000)),
        "microsecond" | "microseconds" => Some((4, 1_000, 1_000)),
        "nanosecond" | "nanoseconds" => Some((5, 1, 1_000)),
        _ => None,
    }
}

/// 按普通有符号数语义舍入 Instant 差值。
fn round_instant_difference(value: i128, increment: i128, mode: InstantRoundingMode) -> Option<i128> {
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

fn balance_instant_difference(value: i128, largest_unit: usize) -> Option<[f64; 10]> {
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

fn instant_difference<H: VmHost>(vm: &mut H, args: &[u8], since: bool) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };

    // other 必须先完成 Instant 转换，之后才读取 options。
    let other = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other_ns = native_try!(instant_like_epoch_ns(vm, other));
    let options_value = if args.len() < 3 { JsValue::undefined() } else { vm.reg(args[2]) };

    let (largest_value, increment_value, mode_value, smallest_value) = if options_value.is_undefined() {
        (None, 1.0, "trunc".to_string(), "nanosecond".to_string())
    } else {
        if !options_value.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let largest_raw = native_try!(temporal_option_value(vm, options, options_value, "largestUnit"));
        let largest = if largest_raw.is_undefined() {
            None
        } else {
            Some(native_try!(temporal_option_string(vm, largest_raw)))
        };
        let increment_raw = native_try!(temporal_option_value(vm, options, options_value, "roundingIncrement"));
        let increment = if increment_raw.is_undefined() {
            1.0
        } else {
            native_try!(temporal_option_number(vm, increment_raw))
        };
        let mode_raw = native_try!(temporal_option_value(vm, options, options_value, "roundingMode"));
        let mode = if mode_raw.is_undefined() {
            "trunc".to_string()
        } else {
            native_try!(temporal_option_string(vm, mode_raw))
        };
        let smallest_raw = native_try!(temporal_option_value(vm, options, options_value, "smallestUnit"));
        let smallest = if smallest_raw.is_undefined() {
            "nanosecond".to_string()
        } else {
            native_try!(temporal_option_string(vm, smallest_raw))
        };
        (largest, increment, mode, smallest)
    };

    let Some((smallest_index, smallest_ns, increment_limit)) = instant_difference_unit(&smallest_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid smallest unit"));
    };
    let largest_index = match largest_value {
        Some(value) => match instant_difference_unit(&value) {
            Some((index, _, _)) => index,
            None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid largest unit")),
        },
        None => smallest_index.min(2),
    };
    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
    };
    if !increment_value.is_finite() {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment_value.trunc();
    if !(1.0..=1_000_000_000.0).contains(&increment) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment as i128;
    if increment >= increment_limit || increment_limit % increment != 0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    if largest_index > smallest_index {
        return NativeResult::Err(crate::error::create_range_error(vm, "smallest unit exceeds largest unit"));
    }

    let delta = if since { epoch_ns - other_ns } else { other_ns - epoch_ns };
    let Some(quantum_ns) = smallest_ns.checked_mul(increment) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    };
    let Some(rounded_ns) = round_instant_difference(delta, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant difference is out of range"));
    };
    let Some(values) = balance_instant_difference(rounded_ns, largest_index) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant difference is out of range"));
    };
    make_duration(vm, values)
}

/// `Temporal.Instant.prototype.until(other, options)`：返回从 receiver 到 other 的精确时长。
pub fn instant_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_difference(vm, args, false)
}

/// `Temporal.Instant.prototype.since(other, options)`：返回从 other 到 receiver 的精确时长。
pub fn instant_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_difference(vm, args, true)
}

enum FractionalSecondDigitsInput {
    Auto,
    Number(f64),
    String(String),
}

fn parse_fractional_second_digits<H: VmHost>(
    vm: &mut H, input: FractionalSecondDigitsInput,
) -> Result<Option<usize>, JsValue> {
    match input {
        FractionalSecondDigitsInput::Auto => Ok(None),
        FractionalSecondDigitsInput::String(value) if value == "auto" => Ok(None),
        FractionalSecondDigitsInput::String(_) => {
            Err(crate::error::create_range_error(vm, "invalid fractionalSecondDigits"))
        }
        FractionalSecondDigitsInput::Number(value) => {
            let digits = value.floor();
            if !digits.is_finite() || !(0.0..=9.0).contains(&digits) {
                return Err(crate::error::create_range_error(vm, "invalid fractionalSecondDigits"));
            }
            Ok(Some(digits as usize))
        }
    }
}

fn parse_offset_minutes(value: &str) -> Option<i32> {
    let bytes = value.as_bytes();
    if bytes.len() != 6 || !matches!(bytes[0], b'+' | b'-') || bytes[3] != b':' {
        return None;
    }
    if !bytes[1..3].iter().all(u8::is_ascii_digit) || !bytes[4..6].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let hours = i32::from(bytes[1] - b'0') * 10 + i32::from(bytes[2] - b'0');
    let minutes = i32::from(bytes[4] - b'0') * 10 + i32::from(bytes[5] - b'0');
    if hours > 23 || minutes > 59 {
        return None;
    }
    let magnitude = hours * 60 + minutes;
    Some(if bytes[0] == b'-' { -magnitude } else { magnitude })
}

fn canonical_time_zone(value: &str) -> Option<(String, i32)> {
    let input = value.trim();
    if input.eq_ignore_ascii_case("UTC") || input.eq_ignore_ascii_case("Z") {
        return Some(("UTC".to_string(), 0));
    }
    if let Some(offset) = parse_offset_minutes(input) {
        return Some((input.to_string(), offset));
    }
    if input.starts_with("-000000") {
        return None;
    }

    // 带 annotation 的日期时间以最后一个方括号内标识符为准。
    if let Some(open) = input.rfind('[') {
        if !input.ends_with(']') || open + 2 > input.len() {
            return None;
        }
        let annotation = &input[open + 1..input.len() - 1];
        if annotation.eq_ignore_ascii_case("UTC") {
            return Some(("UTC".to_string(), 0));
        }
        return parse_offset_minutes(annotation).map(|offset| (annotation.to_string(), offset));
    }

    let time_start = input.find(['T', 't', ' '])?;
    let time = &input[time_start + 1..];
    if time.ends_with(['Z', 'z']) {
        return Some(("UTC".to_string(), 0));
    }
    let offset_start = time
        .char_indices()
        .rev()
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index))?;
    let offset_id = &time[offset_start..];
    parse_offset_minutes(offset_id).map(|offset| (offset_id.to_string(), offset))
}

fn instant_time_zone_offset(value: &str) -> Option<i32> {
    canonical_time_zone(value).map(|(_, offset)| offset)
}

fn civil_from_days(days: i128) -> (i128, i128, i128) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i128::from(month <= 2);
    (year, month, day)
}

fn format_iso_year(year: i128) -> String {
    if (0..=9_999).contains(&year) {
        format!("{year:04}")
    } else if year >= 0 {
        format!("+{year:06}")
    } else {
        format!("-{:06}", -year)
    }
}

fn format_instant_iso(
    epoch_ns: i128, offset_minutes: Option<i32>, include_seconds: bool, fractional_digits: Option<usize>,
) -> Option<String> {
    const DAY_NS: i128 = 86_400_000_000_000;
    let offset_ns = i128::from(offset_minutes.unwrap_or(0)).checked_mul(60_000_000_000)?;
    let local_ns = epoch_ns.checked_add(offset_ns)?;
    let days = local_ns.div_euclid(DAY_NS);
    let mut time_ns = local_ns.rem_euclid(DAY_NS);
    let hour = time_ns / 3_600_000_000_000;
    time_ns %= 3_600_000_000_000;
    let minute = time_ns / 60_000_000_000;
    time_ns %= 60_000_000_000;
    let second = time_ns / 1_000_000_000;
    let subsecond = time_ns % 1_000_000_000;
    let (year, month, day) = civil_from_days(days);

    let mut output = format!("{}-{month:02}-{day:02}T{hour:02}:{minute:02}", format_iso_year(year));
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
                let fraction = format!("{subsecond:09}").trim_end_matches('0').to_string();
                output.push('.');
                output.push_str(&fraction);
            }
            None => {}
        }
    }

    if let Some(offset) = offset_minutes {
        let sign = if offset < 0 { '-' } else { '+' };
        let magnitude = offset.abs();
        output.push_str(&format!("{sign}{:02}:{:02}", magnitude / 60, magnitude % 60));
    } else {
        output.push('Z');
    }
    Some(output)
}

fn instant_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };
    match format_instant_iso(epoch_ns, None, true, None) {
        Some(output) => NativeResult::Ok(vm.new_string(&output)),
        None => NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    }
}

/// `Temporal.Instant.prototype.toString(options)`：按精度、舍入模式和时区输出 ISO 8601。
pub fn instant_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let epoch_ns = match get_instant_epoch_ns(obj) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    };
    let options_value = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (fractional_input, mode_value, smallest_value, time_zone_raw) = if options_value.is_undefined() {
        (FractionalSecondDigitsInput::Auto, "trunc".to_string(), None, JsValue::undefined())
    } else {
        if !options_value.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let fractional_raw = native_try!(temporal_option_value(vm, options, options_value, "fractionalSecondDigits"));
        let fractional = if fractional_raw.is_undefined() {
            FractionalSecondDigitsInput::Auto
        } else if fractional_raw.is_int() || fractional_raw.is_double() {
            FractionalSecondDigitsInput::Number(to_number(fractional_raw))
        } else {
            FractionalSecondDigitsInput::String(native_try!(temporal_option_string(vm, fractional_raw)))
        };
        let mode_raw = native_try!(temporal_option_value(vm, options, options_value, "roundingMode"));
        let mode = if mode_raw.is_undefined() {
            "trunc".to_string()
        } else {
            native_try!(temporal_option_string(vm, mode_raw))
        };
        let smallest_raw = native_try!(temporal_option_value(vm, options, options_value, "smallestUnit"));
        let smallest = if smallest_raw.is_undefined() {
            None
        } else {
            Some(native_try!(temporal_option_string(vm, smallest_raw)))
        };
        let time_zone = native_try!(temporal_option_value(vm, options, options_value, "timeZone"));
        (fractional, mode, smallest, time_zone)
    };

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
    let offset_minutes = if time_zone_raw.is_undefined() {
        None
    } else {
        if !time_zone_raw.is_string() {
            return NativeResult::Err(crate::error::create_type_error(vm, "invalid time zone"));
        }
        let time_zone = to_string(time_zone_raw);
        match instant_time_zone_offset(&time_zone) {
            Some(offset) => Some(offset),
            None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid time zone")),
        }
    };
    let Some(rounded_ns) = round_instant_ns(epoch_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    };
    if rounded_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "Instant outside supported range"));
    }
    match format_instant_iso(rounded_ns, offset_minutes, include_seconds, output_digits) {
        Some(output) => NativeResult::Ok(vm.new_string(&output)),
        None => NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant")),
    }
}

/// `Temporal.Instant.prototype.toJSON()`：输出默认 UTC ISO 字符串，忽略参数。
pub fn instant_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_default_string(vm, args)
}

/// `Temporal.Instant.prototype.toLocaleString()`：当前使用稳定的 UTC ISO 表示。
pub fn instant_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    instant_default_string(vm, args)
}

/// `Temporal.Instant.prototype.toZonedDateTimeISO(timeZone)`：以同一纪元纳秒创建 ISO 日历 ZonedDateTime。
pub fn instant_to_zoned_date_time_iso<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_instant(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid Instant"));
    };
    if args.len() < 2 {
        return NativeResult::Err(crate::error::create_type_error(vm, "timeZone is required"));
    }
    let raw = vm.reg(args[1]);
    if !raw.is_string() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid time zone"));
    }
    let input = to_string(raw);
    let Some((time_zone_id, _)) = canonical_time_zone(&input) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time zone"));
    };
    make_zoned_date_time(vm, epoch_ns, &time_zone_id)
}

/// `Temporal.Instant.prototype.valueOf()`：Temporal 对象禁止转原始值。
pub fn instant_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.Instant has no valueOf"))
}

/// `Temporal.ZonedDateTime` 构造器：保存纪元纳秒、固定偏移或 UTC 时区以及 ISO 日历。
pub fn zoned_date_time_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().zoned_date_time_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.ZonedDateTime cannot be invoked without 'new'",
        ));
    }
    let epoch_raw = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let epoch_ns = native_try!(primitive_to_bigint(vm, epoch_raw));
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    }
    if args.len() < 3 || !vm.reg(args[2]).is_string() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid time zone"));
    }
    let time_zone_input = to_string(vm.reg(args[2]));
    let Some((time_zone_id, _)) = canonical_time_zone(&time_zone_input) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time zone"));
    };
    if args.len() >= 4 && !vm.reg(args[3]).is_undefined() {
        let calendar = vm.reg(args[3]);
        if !calendar.is_string() {
            return NativeResult::Err(crate::error::create_type_error(vm, "invalid calendar"));
        }
        if !to_string(calendar).eq_ignore_ascii_case("iso8601") {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid calendar"));
        }
    }
    let epoch_value = vm.new_bigint(epoch_ns);
    let time_zone_value = vm.new_string(&time_zone_id);
    let calendar_value = vm.new_string("iso8601");
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_ZONED_DATE_TIME,
        [epoch_value, time_zone_value, calendar_value],
    )
}

/// `Temporal.ZonedDateTime.prototype.epochNanoseconds`：返回精确纪元纳秒。
pub fn zoned_date_time_epoch_nanoseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    NativeResult::Ok(obj.get_prop_at(0))
}

/// `Temporal.ZonedDateTime.prototype.timeZoneId`：返回规范化时区标识符。
pub fn zoned_date_time_time_zone_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    NativeResult::Ok(obj.get_prop_at(1))
}

/// `Temporal.ZonedDateTime.prototype.calendarId`：最小实现固定返回 ISO 8601 日历。
pub fn zoned_date_time_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    NativeResult::Ok(obj.get_prop_at(2))
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

fn make_plain_time<H: VmHost>(vm: &mut H, total_ns: f64) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_time_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_TIME;
    obj.set_prop_at(0, JsValue::float(total_ns));
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
    if bytes.get(cursor).copied().map(|byte| byte.to_ascii_uppercase()) != Some(b'P') {
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
        if bytes[cursor].eq_ignore_ascii_case(&b'T') {
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

        let unit = bytes.get(cursor)?.to_ascii_uppercase();
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
        if order <= last_order || fraction_seen {
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

const DURATION_FIELD_ORDER: [(&str, usize); 10] = [
    ("days", 3),
    ("hours", 4),
    ("microseconds", 8),
    ("milliseconds", 7),
    ("minutes", 5),
    ("months", 1),
    ("nanoseconds", 9),
    ("seconds", 6),
    ("weeks", 2),
    ("years", 0),
];

fn duration_field_number<H: VmHost>(
    vm: &mut H, obj: &JsObject, receiver: JsValue, name: &str,
) -> Result<Option<f64>, JsValue> {
    let key_val = vm.new_string(name);
    let si = vm.property_key_si(key_val);
    let raw = match vm.ordinary_get(obj, si, receiver) {
        Ok(value) => value,
        Err(error) => return Err(native_engine_error(vm, &error)),
    };
    if raw.is_undefined() {
        return Ok(None);
    }
    let primitive = match oxide_runtime_api::to_primitive(raw, oxide_runtime_api::ToPrimitiveHint::Number, vm) {
        Ok(value) => value,
        Err(error) => return Err(native_engine_error(vm, &error)),
    };
    if primitive.is_symbol() || primitive.is_bigint() {
        return Err(crate::error::create_type_error(vm, "cannot convert duration field to number"));
    }
    Ok(Some(to_number(primitive)))
}

fn duration_partial_values<H: VmHost>(vm: &mut H, val: JsValue) -> Result<[Option<f64>; 10], JsValue> {
    if !val.is_object() {
        return Err(crate::error::create_type_error(vm, "duration-like value must be an object"));
    }
    let ptr = val.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "duration-like value must be an object"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_duration_obj() {
        return Ok(std::array::from_fn(|index| Some(get_double_prop(obj, index))));
    }

    let mut values = [None; 10];
    for (name, index) in DURATION_FIELD_ORDER {
        values[index] = duration_field_number(vm, obj, val, name)?;
    }
    if values.iter().all(Option::is_none) {
        return Err(crate::error::create_type_error(vm, "duration-like object has no duration fields"));
    }
    Ok(values)
}

fn duration_component_integer(value: f64) -> Option<i128> {
    if !value.is_finite() || value.fract() != 0.0 || value.abs() >= i128::MAX as f64 {
        return None;
    }
    Some(value as i128)
}

fn duration_time_nanoseconds(values: &[f64; 10]) -> Option<i128> {
    const SCALES: [i128; 7] =
        [86_400_000_000_000, 3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    let mut total = 0_i128;
    for (value, scale) in values[3..].iter().zip(SCALES) {
        total = total.checked_add(duration_component_integer(*value)?.checked_mul(scale)?)?;
    }
    Some(total)
}

fn validate_duration_values<H: VmHost>(vm: &mut H, values: &[f64; 10]) -> Result<(), JsValue> {
    let mut sign = 0_i8;
    for value in values {
        if duration_component_integer(*value).is_none() {
            return Err(crate::error::create_range_error(vm, "invalid duration"));
        }
        if *value != 0.0 {
            let current = if value.is_sign_negative() { -1 } else { 1 };
            if sign != 0 && sign != current {
                return Err(crate::error::create_range_error(vm, "duration fields must have the same sign"));
            }
            sign = current;
        }
    }
    if values[..3].iter().any(|value| value.abs() > u32::MAX as f64) {
        return Err(crate::error::create_range_error(vm, "duration date field is out of range"));
    }
    const MAX_TIME_NANOSECONDS: i128 = (1_i128 << 53) * 1_000_000_000;
    let total = duration_time_nanoseconds(values)
        .ok_or_else(|| crate::error::create_range_error(vm, "duration time fields are out of range"))?;
    if total.abs() >= MAX_TIME_NANOSECONDS {
        return Err(crate::error::create_range_error(vm, "duration time fields are out of range"));
    }
    Ok(())
}

fn duration_like_values<H: VmHost>(vm: &mut H, val: JsValue) -> Result<[f64; 10], JsValue> {
    if val.is_string() {
        let values = parse_duration_string(&to_string(val))
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid duration string"))?;
        validate_duration_values(vm, &values)?;
        return Ok(values);
    }
    let partial = duration_partial_values(vm, val)?;
    let values = std::array::from_fn(|index| partial[index].unwrap_or(0.0));
    validate_duration_values(vm, &values)?;
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
    native_try!(validate_duration_values(vm, &values));
    initialize_temporal_receiver(vm, args, JsObject::OBJ_TYPE_DURATION, values.map(JsValue::float))
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
    let total = duration_component_integer(seconds)?
        .checked_mul(1_000_000_000)?
        .checked_add(duration_component_integer(milliseconds)?.checked_mul(1_000_000)?)?
        .checked_add(duration_component_integer(microseconds)?.checked_mul(1_000)?)?
        .checked_add(duration_component_integer(nanoseconds)?)?;
    if total == 0 {
        return None;
    }
    let negative = total < 0;
    let magnitude = total.abs();
    let whole = magnitude / 1_000_000_000;
    let fraction = magnitude % 1_000_000_000;
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

/// `Temporal.Duration.prototype.with(partial)`，以给定字段替换当前时长分量。
pub fn duration_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let partial_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let partial = native_try!(duration_partial_values(vm, partial_value));
    let mut values = duration_values(obj);
    for (index, value) in partial.into_iter().enumerate() {
        if let Some(value) = value {
            values[index] = value;
        }
    }
    native_try!(validate_duration_values(vm, &values));
    make_duration(vm, values)
}

/// `Temporal.Duration.prototype.total("seconds")`，汇总不含日历大单位的总秒数。
pub fn duration_total<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let unit_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !unit_value.is_string() || !to_string(unit_value).eq_ignore_ascii_case("seconds") {
        return NativeResult::Err(crate::error::create_range_error(vm, "only seconds total is supported"));
    }
    let values = duration_values(obj);
    if values[..3].iter().any(|value| *value != 0.0) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "a relativeTo option is required for calendar units",
        ));
    }
    let total = match duration_time_nanoseconds(&values) {
        Some(value) => value,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range")),
    };
    NativeResult::Ok(JsValue::float(total as f64 / 1_000_000_000.0))
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
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_PLAIN_DATE,
        [JsValue::float(year as f64), JsValue::float(month as f64), JsValue::float(day as f64)],
    )
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
    let total_ns = hour as f64 * 3.6e12
        + minute as f64 * 6e10
        + second as f64 * 1e9
        + ms as f64 * 1e6
        + us as f64 * 1e3
        + ns as f64;
    initialize_temporal_receiver(vm, args, JsObject::OBJ_TYPE_PLAIN_TIME, [JsValue::float(total_ns)])
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

    if args.len() > 10 && !vm.reg(args[10]).is_undefined() {
        let calendar = vm.reg(args[10]);
        let calendar = native_try!(temporal_option_string(vm, calendar));
        if !calendar.eq_ignore_ascii_case("iso8601") {
            return NativeResult::Err(crate::error::create_range_error(vm, "unsupported calendar"));
        }
    }

    let total_ns = hour as f64 * 3_600_000_000_000.0
        + minute as f64 * 60_000_000_000.0
        + second as f64 * 1_000_000_000.0
        + millisecond as f64 * 1_000_000.0
        + microsecond as f64 * 1_000.0
        + nanosecond as f64;
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_PLAIN_DATE_TIME,
        [
            JsValue::float(year as f64),
            JsValue::float(month as f64),
            JsValue::float(day as f64),
            JsValue::float(total_ns),
        ],
    )
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

/// `Temporal.PlainDateTime.prototype.calendarId` getter。
pub fn plain_date_time_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_date_time_parts(vm, args));
    NativeResult::Ok(vm.new_string("iso8601"))
}

/// 返回仅保留日期分量的新 `Temporal.PlainDate`。
pub fn plain_date_time_to_plain_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, month, day, _) = native_try!(plain_date_time_parts(vm, args));
    make_plain_date(vm, year, month, day)
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
