use chrono::{Datelike, Days, NaiveDate, Utc};

use num_traits::ToPrimitive;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

// Temporal 命名空间的最小实现子集：Temporal.Now / Temporal.Instant /
// Temporal.PlainDate / Temporal.PlainTime / Temporal.PlainDateTime / Temporal.ZonedDateTime。
// 内部数据按对象类型存入 prop 槽：
// Instant 存纪元纳秒（BigInt，prop 0）、PlainDate 存年/月/日/日历 ID（prop 0-3）、
// PlainTime 存午夜后纳秒（f64，prop 0）、PlainDateTime 存年/月/日/午夜后纳秒/日历 ID（prop 0-4），
// ZonedDateTime 存纪元纳秒、时区 ID、日历 ID（prop 0-2）。
// 日历 ID 未显式给定时统一取 "iso8601"。

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

/// 读对象日历槽：字符串槽直接返回，undefined/非 string 兜底 `"iso8601"`。
/// 只读 prop 槽，不触发属性 getter，getter 与日历传播共用。
fn get_calendar_id(obj: &JsObject, slot: usize) -> String {
    let value = obj.get_prop_at(slot);
    if value.is_string() {
        to_string(value)
    } else {
        "iso8601".to_string()
    }
}

fn get_instant_epoch_ns(obj: &JsObject) -> Option<i128> {
    let value = obj.get_prop_at(0);
    if value.is_bigint() {
        return Some(unsafe { oxide_runtime_api::bigint_data(value) }.to_i128().unwrap_or(i128::MAX));
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
    let epoch_value = vm.new_bigint(num_bigint::BigInt::from(epoch_ns));
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_INSTANT;
    obj.set_prop_at(0, epoch_value);
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn make_zoned_date_time<H: VmHost>(vm: &mut H, epoch_ns: i128, time_zone_id: &str, calendar: &str) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().zoned_date_time_proto.as_ptr() as *mut JsObject);
    let epoch_value = vm.new_bigint(num_bigint::BigInt::from(epoch_ns));
    let time_zone_value = vm.new_string(time_zone_id);
    let calendar_value = vm.new_string(calendar);
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
        return Ok(vm.bigint_value(primitive).to_i128().unwrap_or(i128::MAX));
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
        // 对象按规范走 ToString -> ParseTemporalInstantString（解析失败 RangeError）。
        let input = oxide_runtime_api::to_string_full(value, vm).map_err(|error| native_engine_error(vm, &error))?;
        return parse_instant_string(&input)
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid ISO 8601 string"));
    }
    // 非字符串原始值（undefined/null/boolean/number/bigint/symbol）按规范直接抛 TypeError。
    if !value.is_string() {
        return Err(crate::error::create_type_error(vm, "cannot convert to Instant"));
    }
    let input = to_string(value);
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
    let epoch_value = vm.new_bigint(num_bigint::BigInt::from(epoch_ns));
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
            Some(epoch_ns) => NativeResult::Ok(vm.new_bigint(num_bigint::BigInt::from(epoch_ns))),
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
        Some(output) => NativeResult::Ok(vm.new_string_owned(output)),
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
        Some(output) => NativeResult::Ok(vm.new_string_owned(output)),
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
    make_zoned_date_time(vm, epoch_ns, &time_zone_id, "iso8601")
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
    let calendar = if args.len() >= 4 && !vm.reg(args[3]).is_undefined() {
        native_try!(temporal_calendar_id_strict(vm, vm.reg(args[3]))).unwrap_or_else(|| "iso8601".to_string())
    } else {
        "iso8601".to_string()
    };
    let epoch_value = vm.new_bigint(num_bigint::BigInt::from(epoch_ns));
    let time_zone_value = vm.new_string(&time_zone_id);
    let calendar_value = vm.new_string(&calendar);
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

/// `Temporal.ZonedDateTime.prototype.calendarId`：读日历槽。
pub fn zoned_date_time_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    NativeResult::Ok(obj.get_prop_at(2))
}

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

/// `Temporal.Duration.prototype.total(totalOf)`：按单位汇总时长。
/// 日历单位（year/month/week）需要 relativeTo（PlainDateTime/PlainDate）。
pub fn duration_total<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let total_of = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if total_of.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "options argument is required"));
    }
    let (unit_raw, relative_raw) = if total_of.is_string() {
        (to_string(total_of), JsValue::undefined())
    } else {
        if !total_of.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = total_of.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let relative_raw = match temporal_option_value(vm, options, total_of, "relativeTo") {
            Ok(raw) => raw,
            Err(error) => return NativeResult::Err(error),
        };
        let unit_raw = match temporal_option_value(vm, options, total_of, "unit") {
            Ok(raw) => match temporal_option_string(vm, raw) {
                Ok(value) => value,
                Err(error) => return NativeResult::Err(error),
            },
            Err(error) => return NativeResult::Err(error),
        };
        (unit_raw, relative_raw)
    };
    let unit_index = match plain_date_time_unit_index(&unit_raw) {
        Some(index) => index,
        None => return NativeResult::Err(crate::error::create_range_error(vm, "invalid unit")),
    };
    let values = duration_values(obj);
    // 含日历单位（year/month/week）时，days 及以上的 total 必须走 relativeTo 日历路径。
    let has_calendar_units = values[0] != 0.0 || values[1] != 0.0 || values[2] != 0.0;

    // relativeTo：支持 PlainDateTime / PlainDate，取日期分量（时间按午夜计算，对齐 polyfill）。
    // relativeTo: supports PlainDateTime / PlainDate objects; strings follow the
    // ToRelativeTemporalObject path (PlainDateTime first, falling back to PlainDate),
    // invalid/out-of-range strings throw RangeError, and non-string primitives
    // (number/boolean/bigint/symbol/null) throw TypeError per test262.
    let relative_date = if relative_raw.is_string() {
        let text = to_string(relative_raw);
        match parse_plain_date_time_string(&text)
            .or_else(|_| parse_plain_date_string(&text).map(|(y, m, d)| (y, m, d, 0.0)))
        {
            Ok((year, month, day, _time_ns)) => Some((i128::from(year), i128::from(month), i128::from(day))),
            Err(_) => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid relativeTo string"));
            }
        }
    } else if relative_raw.is_object() {
        let rel_ptr = relative_raw.as_js_object_ptr();
        if rel_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "invalid relativeTo"));
        }
        let rel = unsafe { &*rel_ptr };
        if rel.is_plain_date_time_obj() || rel.is_plain_date_obj() {
            Some((
                i128::from(get_double_prop(rel, 0) as i32),
                i128::from(get_double_prop(rel, 1) as u32),
                i128::from(get_double_prop(rel, 2) as u32),
            ))
        } else {
            None
        }
    } else if relative_raw.is_undefined() {
        None
    } else {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid relativeTo"));
    };

    const UNIT_NS: [i128; 6] = [3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    if unit_index <= 2 || (unit_index == 3 && has_calendar_units) {
        // 日历单位：需要 relativeTo，用纪元纳秒窗口计算分数总量。
        let Some(rel_date) = relative_date else {
            return NativeResult::Err(crate::error::create_range_error(
                vm,
                &format!("a starting point is required for {} total", unit_raw),
            ));
        };
        let Some(time_ns_base) = duration_time_nanoseconds(&values) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        // days 并入时间（24 小时/天），整日回填到日期分量。
        // duration_time_nanoseconds 已包含 days 分量（24 小时/天），直接取整日。
        let time_ns_total = time_ns_base;
        let delta_days = time_ns_total / DAY_NS;
        let target_time = time_ns_total % DAY_NS;
        let mut date_parts = [0.0; 10];
        date_parts[0] = values[0];
        date_parts[1] = values[1];
        date_parts[2] = values[2];
        date_parts[3] = delta_days as f64;
        let target_date = add_date_duration(rel_date, &date_parts);

        // DifferenceISODateTime：date1=relativeTo（午夜）、date2=target。
        let date1 = rel_date;
        let mut date2 = target_date;
        let mut time_ns = target_time;
        let time_sign = if time_ns > 0 {
            1_i128
        } else if time_ns < 0 {
            -1_i128
        } else {
            0
        };
        let date_sign = compare_iso_date(date1, date2);
        if date_sign != 0 && date_sign == time_sign {
            date2 = add_days_iso(date2, time_sign);
            time_ns -= time_sign * DAY_NS;
        }
        let date_values = date_until_iso(date1, date2, unit_index);
        let sign = {
            let ds = date_duration_sign(&date_values);
            if ds != 0 {
                ds
            } else if time_ns > 0 {
                1
            } else if time_ns < 0 {
                -1
            } else {
                1
            }
        };
        let origin_epoch = days_from_civil(date1.0, date1.1, date1.2) * DAY_NS;
        let dest_epoch = days_from_civil(target_date.0, target_date.1, target_date.2) * DAY_NS + target_time;
        let (r1, _, start_dur, end_dur) = nudge_window(sign, &date_values, date1, 1, unit_index, false);
        let epoch_of = |dur: &[f64; 10]| -> Option<i128> {
            if date_duration_sign(dur) == 0 {
                return Some(origin_epoch);
            }
            let date = add_date_duration(date1, dur);
            let days = days_from_civil(date.0, date.1, date.2);
            if days.abs() > MAX_ISO_DAY {
                return None;
            }
            Some(days * DAY_NS)
        };
        let Some(start_epoch) = epoch_of(&start_dur) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        let Some(end_epoch) = epoch_of(&end_dur) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        let numerator = dest_epoch - start_epoch;
        let denominator = end_epoch - start_epoch;
        if denominator == 0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        }
        let total = (denominator as f64 * r1 as f64 + numerator as f64 * sign as f64) / denominator as f64;
        return NativeResult::Ok(JsValue::float(total));
    }

    // 均匀长度单位（day..nanosecond）：直接按纳秒汇总。
    let Some(time_ns_base) = duration_time_nanoseconds(&values) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
    };
    // duration_time_nanoseconds 已包含 days 分量。
    let total_ns = time_ns_base;
    let scale = if unit_index == 3 { DAY_NS } else { UNIT_NS[unit_index - 4] };
    NativeResult::Ok(JsValue::float(total_ns as f64 / scale as f64))
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
    NativeResult::Ok(vm.new_string_owned(output))
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

/// `Temporal.Duration.prototype.add(other)`：分量相加并按规范平衡时间单位；
/// 任一侧含日历单位（years/months/weeks）时抛 RangeError（无 relativeTo 支持）。
pub fn duration_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let other_val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other = native_try!(duration_like_values(vm, other_val));
    let receiver = duration_values(obj);
    if receiver[..3].iter().any(|value| *value != 0.0) || other[..3].iter().any(|value| *value != 0.0) {
        return NativeResult::Err(crate::error::create_range_error(vm, "cannot add durations with calendar units"));
    }
    // 最大单位取 receiver 与参数中最大的非零时间单位（days=3 最大，ns=9 最小）。
    let mut largest = 9_usize;
    for index in 3..10 {
        if receiver[index] != 0.0 || other[index] != 0.0 {
            largest = index;
            break;
        }
    }
    if largest == 9 && receiver[9] + other[9] == 0.0 {
        return make_duration(vm, [0.0; 10]);
    }
    // 分量按精确整数求和（f64 分量是精确整数；超过 2^53 的和需在 i128 上保持精确，规范按数学值计算）。
    let mut total_ns = 0_i128;
    const SUM_SCALES: [i128; 7] =
        [86_400_000_000_000, 3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    for (index, scale) in (3..10).zip(SUM_SCALES) {
        let Some(a) = duration_component_integer(receiver[index]) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid duration"));
        };
        let Some(b) = duration_component_integer(other[index]) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid duration"));
        };
        let Some(component) = a.checked_add(b).and_then(|value| value.checked_mul(scale)) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        let Some(updated) = total_ns.checked_add(component) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        total_ns = updated;
    }
    // 从总纳秒向下按截断除法分解，进位到 largest 为止（largest 单位可无界）。
    let mut values = [0.0; 10];
    let mut rem = total_ns;
    let mut unit = 9_usize;
    loop {
        if unit == largest {
            values[unit] = rem as f64;
            break;
        }
        let base = match unit {
            7..=9 => 1_000,
            6 | 5 => 60,
            _ => 24,
        };
        values[unit] = (rem % base) as f64;
        rem /= base;
        unit -= 1;
    }
    // 范围校验：对 𝔽 舍入后的每个分量按其纳秒刻度检查是否达到 2^53 秒上限。
    const MAX_TIME_NANOSECONDS: f64 = (1_i128 << 53) as f64 * 1_000_000_000.0;
    const UNIT_SCALES: [f64; 7] = [
        86_400_000_000_000.0,
        3_600_000_000_000.0,
        60_000_000_000.0,
        1_000_000_000.0,
        1_000_000.0,
        1_000.0,
        1.0,
    ];
    for (index, scale) in (3..10).zip(UNIT_SCALES) {
        if values[index] != 0.0 && values[index].abs() * scale >= MAX_TIME_NANOSECONDS {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration time fields are out of range"));
        }
    }
    make_duration(vm, values)
}
/// `Temporal.Duration.prototype.round(roundTo)`锛氭寜鏈€灏忓崟浣嶈垗鍏ュ苟鎸夋渶澶у崟浣嶅钩琛°€?
/// 绗竴鐗堟敮鎸佹棤鏃ュ巻鍗曚綅锛坹ear/month/week 闈為浂鎴栫洰鏍囦负鏃ュ巻鍗曚綅鏃惰姹?relativeTo锛屾殏鎶?RangeError锛夛紱
/// 绾弒鏃堕棿璺緞鎸?24 灏忔椂/澶╁鐞嗭紝涓?polyfill 鐨?24h-day 璇箟涓€鑷淬€?
pub fn duration_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let round_to = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    if round_to.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "options parameter is required"));
    }

    let values = duration_values(obj);
    // 鐜版湁鏈€澶у崟浣嶏細棣栦釜闈為浂鍒嗛噺锛涘叏闆舵椂瑙嗕负 nanosecond銆?
    let mut existing_largest = 9_usize;
    for (index, value) in values.iter().enumerate() {
        if *value != 0.0 {
            existing_largest = index;
            break;
        }
    }

    // roundTo 瀛楃涓?=> { smallestUnit: 瀛楃涓?}锛涘惁鍒欏繀椤讳负瀵硅薄銆?
    let (largest_raw, _relative_raw, increment_raw, mode_raw, smallest_raw) = if round_to.is_string() {
        (
            JsValue::undefined(),
            JsValue::undefined(),
            JsValue::undefined(),
            JsValue::undefined(),
            round_to,
        )
    } else {
        if !round_to.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "roundTo must be an object"));
        }
        let options_ptr = round_to.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "roundTo must be an object"));
        }
        let options = unsafe { &*options_ptr };
        let largest_raw = native_try!(temporal_option_value(vm, options, round_to, "largestUnit"));
        let relative_raw = native_try!(temporal_option_value(vm, options, round_to, "relativeTo"));
        let increment_raw = native_try!(temporal_option_value(vm, options, round_to, "roundingIncrement"));
        let mode_raw = native_try!(temporal_option_value(vm, options, round_to, "roundingMode"));
        let smallest_raw = native_try!(temporal_option_value(vm, options, round_to, "smallestUnit"));
        (largest_raw, relative_raw, increment_raw, mode_raw, smallest_raw)
    };

    // largestUnit锛氬厑璁?auto"锛涚己鐪?/undefined 瑙嗕负鏈彁渚涖€?
    let largest_provided = !largest_raw.is_undefined();
    let largest_index = if largest_raw.is_undefined() {
        None
    } else {
        let unit = native_try!(temporal_option_string(vm, largest_raw));
        if unit == "auto" {
            None
        } else {
            match plain_date_time_unit_index(&unit) {
                Some(index) => Some(index),
                None => {
                    return NativeResult::Err(crate::error::create_range_error(vm, "invalid largestUnit"));
                }
            }
        }
    };

    // roundingIncrement锛歍oIntegerOrInfinity 鑸嶅叆鍚庨渶鍦?[1, 10^9] 鍐呫€?
    let increment_value = if increment_raw.is_undefined() {
        1.0
    } else {
        native_try!(temporal_option_number(vm, increment_raw))
    };
    let increment = increment_value.trunc();
    if !increment.is_finite() || !(1.0..=1_000_000_000.0).contains(&increment) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    let increment = increment as i128;

    // roundingMode锛氱己鐪?halfExpand銆?
    let mode = if mode_raw.is_undefined() {
        InstantRoundingMode::HalfExpand
    } else {
        let mode_string = native_try!(temporal_option_string(vm, mode_raw));
        match instant_rounding_mode(&mode_string) {
            Some(mode) => mode,
            None => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
            }
        }
    };

    // smallestUnit锛氱己鐪?nanosecond銆?
    let smallest_provided = !smallest_raw.is_undefined();
    let smallest_index = if smallest_raw.is_undefined() {
        9
    } else {
        let unit = native_try!(temporal_option_string(vm, smallest_raw));
        match plain_date_time_unit_index(&unit) {
            Some(index) => index,
            None => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid smallestUnit"));
            }
        }
    };

    // 榛樿鏈€澶у崟浣嶏細鐜版湁鏈€澶у崟浣嶄笌鏈€灏忓崟浣嶄腑杈冨ぇ鐨勯偅涓€€?
    let default_largest = if existing_largest < smallest_index {
        existing_largest
    } else {
        smallest_index
    };
    let largest = largest_index.unwrap_or(default_largest);

    // 鑷冲皯涓€涓崟浣嶉渶瑕佹樉寮忔彁渚涳紱largest 涓嶈兘灏忎簬 smallest銆?
    if !smallest_provided && !largest_provided {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "at least one of smallestUnit or largestUnit is required",
        ));
    }
    if smallest_index < largest {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "largestUnit cannot be smaller than smallestUnit",
        ));
    }

    // 鑸嶅叆澧為噺涓婇檺锛氬崟浣嶈秺灏忓彲闄ら櫎涓婁竴绾э紱鏃ュ巻鍗曚綅锛坹ear/month/week/day锛夋棤鏁撮櫎绾︽潫銆?
    const MAX_INCREMENT: [i128; 10] = [0, 0, 0, 0, 24, 60, 60, 1000, 1000, 1000];
    let max_increment = MAX_INCREMENT[smallest_index];
    if max_increment != 0 && (increment >= max_increment || max_increment % increment != 0) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }

    // 鏃ュ巻鍗曚綅锛坹ear/month/week锛夛細鏈増瑕佹眰 relativeTo锛屾殏鎶?RangeError銆?
    if values[..3].iter().any(|value| *value != 0.0) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "a starting point is required for balancing calendar units",
        ));
    }
    if largest < 3 || smallest_index < 3 {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "a starting point is required for calendar units",
        ));
    }
    if increment > 1 && smallest_index == 3 && largest != smallest_index {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "rounding increments of calendar units require largestUnit to equal smallestUnit",
        ));
    }

    // 绾弒鏃堕棿璺緞锛氭寜 24 灏忔椂/澶╁皢 days..nanoseconds 姹囨€讳负绾崇锛屾寜 smallest 鑸嶅叆鍚庡钩琛″埌 largest銆?
    let Some(time_ns) = duration_time_nanoseconds(&values) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
    };
    const UNIT_NS: [i128; 7] =
        [86_400_000_000_000, 3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    let quantum = if smallest_index == 3 {
        increment * UNIT_NS[0]
    } else {
        increment * UNIT_NS[smallest_index - 3]
    };
    let Some(rounded) = round_instant_difference(time_ns, quantum, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
    };
    let mut result = [0.0; 10];
    if smallest_index == 3 {
        result[3] = (rounded / UNIT_NS[0]) as f64;
    } else {
        // 涓?duration_add 鐩稿悓鐨勫悓鍙锋ā鍒嗚В锛氫粠 ns 鍚?largest 杩涗綅銆?
        let mut rem = rounded;
        let mut unit = 9_usize;
        loop {
            if unit == largest {
                result[unit] = rem as f64;
                break;
            }
            let base = match unit {
                7..=9 => 1_000,
                6 | 5 => 60,
                _ => 24,
            };
            result[unit] = (rem % base) as f64;
            rem /= base;
            unit -= 1;
        }
    }
    // 鑼冨洿鏍￠獙锛氬姣忎釜鏃堕棿鍒嗛噺鎸夊叾绾崇鍒诲害妫€鏌ユ槸鍚﹁揪鍒?2^53 绉掍笂闄愩€?
    const MAX_TIME_NANOSECONDS: f64 = (1_i128 << 53) as f64 * 1_000_000_000.0;
    const UNIT_SCALES: [f64; 7] = [
        86_400_000_000_000.0,
        3_600_000_000_000.0,
        60_000_000_000.0,
        1_000_000_000.0,
        1_000_000.0,
        1_000.0,
        1.0,
    ];
    for (index, scale) in (3..10).zip(UNIT_SCALES) {
        if result[index] != 0.0 && result[index].abs() * scale >= MAX_TIME_NANOSECONDS {
            return NativeResult::Err(crate::error::create_range_error(vm, "duration time fields are out of range"));
        }
    }
    make_duration(vm, result)
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

/// 校验 ISO 日期分量（month 1-12、day 按月份与闰年，不依赖 chrono 年份范围）。
fn valid_iso_date(year: i32, month: u32, day: u32) -> bool {
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
fn valid_plain_date_time_range(year: i32, month: u32, day: u32, time_ns: f64) -> bool {
    const MAX_ISO_DAY: i128 = 100_000_000;
    let limit = (MAX_ISO_DAY + 1) * 86_400_000_000_000;
    match iso_date_time_epoch_ns(year, month, day, time_ns) {
        Some(epoch_ns) => epoch_ns > -limit && epoch_ns < limit,
        None => false,
    }
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
/// 带符号分解午夜后纳秒为时/分/秒/毫秒/微秒/纳秒（各分量向零截断，保持同号）。
fn plain_time_components_signed(total_ns: i128) -> [i128; 6] {
    const SCALES: [i128; 5] = [3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000];
    let mut values = [0_i128; 6];
    let mut remainder = total_ns;
    for (index, scale) in SCALES.iter().enumerate() {
        values[index] = remainder / scale;
        remainder %= scale;
    }
    values[5] = remainder;
    values
}

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

fn validate_temporal_annotation_suffix(mut suffix: &str) -> Result<(), String> {
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
                } else if is_builtin_calendar_id(value).is_none() {
                    return Err("invalid calendar annotation".into());
                } else {
                    calendar_seen = true;
                    saw_critical_calendar = critical;
                }
            } else if critical {
                return Err("unknown critical annotation".into());
            }
        } else {
            if time_zone_seen {
                return Err("invalid time-zone annotation".into());
            }
            time_zone_seen = true;
        }
        suffix = &suffix[end + 1..];
    }
    Ok(())
}

/// 剥离 ISO 偏移（±HH、±HHMM、±HH:MM、±HHMMSS、±HH:MM:SS，秒可带小数），返回剩余部分。
fn strip_iso_offset(input: &str) -> Result<&str, String> {
    let bytes = input.as_bytes();
    if bytes.len() < 3 || !matches!(bytes[0], b'+' | b'-') {
        return Err("invalid ISO offset".into());
    }
    let colon = bytes.len() > 3 && bytes[3] == b':';
    let mut i = 1usize;
    let take_digits = |count: usize, cursor: &mut usize| -> Result<(), String> {
        if *cursor + count > bytes.len() || !bytes[*cursor..*cursor + count].iter().all(u8::is_ascii_digit) {
            return Err("invalid ISO offset".into());
        }
        *cursor += count;
        Ok(())
    };
    take_digits(2, &mut i)?;
    if i < bytes.len() {
        if colon {
            if bytes[i] != b':' {
                return Err("invalid ISO offset".into());
            }
            i += 1;
        }
        if i + 2 <= bytes.len() && bytes[i..i + 2].iter().all(u8::is_ascii_digit) {
            i += 2;
            if i < bytes.len() {
                if colon {
                    if bytes[i] != b':' {
                        return Err("invalid ISO offset".into());
                    }
                    i += 1;
                }
                if i + 2 <= bytes.len() && bytes[i..i + 2].iter().all(u8::is_ascii_digit) {
                    i += 2;
                    if i < bytes.len() && matches!(bytes[i], b'.' | b',') {
                        i += 1;
                        let start = i;
                        while i < bytes.len() && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                        if i == start {
                            return Err("invalid ISO offset fraction".into());
                        }
                        if i - start > 9 {
                            return Err("invalid ISO offset fraction".into());
                        }
                    }
                }
            }
        }
    }
    let digits: String = input[1..i].chars().filter(|ch| ch.is_ascii_digit()).take(6).collect();
    if digits.len() < 2 {
        return Err("invalid ISO offset".into());
    }
    let hour: u32 = digits[..2].parse().map_err(|_| "invalid ISO offset".to_string())?;
    let minute: u32 = if digits.len() >= 4 {
        digits[2..4].parse().map_err(|_| "invalid ISO offset".to_string())?
    } else {
        0
    };
    let second: u32 = if digits.len() >= 6 {
        digits[4..6].parse().map_err(|_| "invalid ISO offset".to_string())?
    } else {
        0
    };
    if hour > 23 || minute > 59 || second > 59 {
        return Err("invalid ISO offset".into());
    }
    Ok(&input[i..])
}

fn parse_plain_date_time_string(input: &str) -> Result<(i32, u32, u32, f64), String> {
    parse_temporal_string_impl(input, true)
}

/// ParseTemporalDateString：PlainDate 字符串（时间部分可选且被忽略，仅按日期做范围校验）。
fn parse_plain_date_string(input: &str) -> Result<(i32, u32, u32), String> {
    parse_temporal_string_impl(input, false).map(|(year, month, day, _)| (year, month, day))
}

fn parse_temporal_string_impl(input: &str, enforce_date_time_range: bool) -> Result<(i32, u32, u32, f64), String> {
    let trimmed = input.trim();
    if trimmed.contains('\u{2212}') {
        return Err("variant minus sign is not valid for PlainDateTime".into());
    }
    let text = trimmed.to_owned();
    let annotation_start = text.find('[').unwrap_or(text.len());
    validate_temporal_annotation_suffix(&text[annotation_start..])?;
    let text = &text[..annotation_start];
    if text.contains('Z') || text.contains('z') {
        return Err("UTC designator is not valid for PlainDateTime".into());
    }
    let separator = text.find(['T', 't', ' ']);
    let date_part = match separator {
        Some(index) => &text[..index],
        None => text,
    };
    let (year, month, day) = parse_iso_date(date_part)?;
    let Some(separator) = separator else {
        if !valid_iso_date(year, month, day)
            || (enforce_date_time_range && !valid_plain_date_time_range(year, month, day, 0.0))
        {
            return Err("invalid ISO date".into());
        }
        return Ok((year, month, day, 0.0));
    };

    let time_and_suffix = &text[separator + 1..];
    let time_end = time_and_suffix
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index))
        .unwrap_or(time_and_suffix.len());
    if time_end < time_and_suffix.len() && !strip_iso_offset(&time_and_suffix[time_end..])?.is_empty() {
        return Err("invalid ISO offset".into());
    }
    let time = &time_and_suffix[..time_end];
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
    let millisecond = subsecond / 1_000_000;
    let microsecond = subsecond / 1_000 % 1_000;
    let nanosecond = subsecond % 1_000;
    if !valid_plain_time(hour, minute, second, millisecond, microsecond, nanosecond) {
        return Err("invalid ISO time".into());
    }
    let total_ns = hour as f64 * 3_600_000_000_000.0
        + minute as f64 * 60_000_000_000.0
        + second as f64 * 1_000_000_000.0
        + subsecond as f64;
    if !valid_iso_date(year, month, day)
        || (enforce_date_time_range && !valid_plain_date_time_range(year, month, day, total_ns))
    {
        return Err("invalid ISO date".into());
    }
    Ok((year, month, day, total_ns))
}

/// 规范 BuiltinCalendarID 全集（18 项）。匹配用 eq_ignore_ascii_case（ASCII 折叠，
/// 禁 to_lowercase：U+0130 点 I 会被 Unicode 折叠成 i+组合符，误判为 iso8601）。
const CALENDAR_ID_WHITELIST: [&str; 18] = [
    "buddhist",
    "chinese",
    "coptic",
    "dangi",
    "ethioaa",
    "ethiopic",
    "gregory",
    "hebrew",
    "indian",
    "islamic",
    "islamic-civil",
    "islamic-rcy",
    "islamic-tbla",
    "islamic-umalqura",
    "iso8601",
    "japanese",
    "persian",
    "roc",
];

/// 严格白名单匹配：命中返回规范小写形式，未命中返回 None。
fn is_builtin_calendar_id(value: &str) -> Option<&'static str> {
    CALENDAR_ID_WHITELIST.iter().find(|id| value.eq_ignore_ascii_case(id)).copied()
}

/// 严格日历 ID 解析：只走 18 项白名单，拒绝 ISO 串（含注解/compact/extended/空串）。
/// 供构造器日历参数与注解值判定使用。
fn parse_temporal_calendar_id_strict(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    is_builtin_calendar_id(trimmed)
        .map(str::to_string)
        .ok_or_else(|| "invalid calendar".to_string())
}

/// ParseTemporalCalendarString：日历标识符 = 18 项内置日历 ID（ASCII 大小写不敏感）
/// 或合法 ISO 日期(-时间)字符串（含部分日期 YYYY-MM / MM-DD，可选时间、偏移、注解）。
/// 返回规范化日历 ID：白名单命中返回规范小写，ISO 串路径恒为 "iso8601"。
fn parse_temporal_calendar_string(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("invalid calendar".into());
    }
    if let Some(id) = is_builtin_calendar_id(trimmed) {
        return Ok(id.to_string());
    }
    if trimmed.contains('\u{2212}') {
        return Err("variant minus sign is not valid for calendar".into());
    }
    // 完整日期时间字符串（含注解校验），例如 2020-01-01T00:00:00.000000000[u-ca=iso8601]
    if parse_plain_date_time_string(trimmed).is_ok() {
        return Ok("iso8601".to_string());
    }
    // 部分日期：YYYY-MM 或 MM-DD（可带注解）
    let text = trimmed.to_owned();
    let annotation_start = text.find('[').unwrap_or(text.len());
    validate_temporal_annotation_suffix(&text[annotation_start..])?;
    let text = &text[..annotation_start];
    if text.contains(['Z', 'z']) {
        return Err("UTC designator is not valid for calendar".into());
    }
    parse_partial_calendar_date(text)?;
    Ok("iso8601".to_string())
}

/// 部分 ISO 日期（无时间）：YYYY[-MM[-DD]] 或 MM-DD；校验月份/日期基本范围并拒绝负零年。
fn parse_partial_calendar_date(input: &str) -> Result<(), String> {
    let bytes = input.as_bytes();
    let mut i = 0usize;
    let signed = i < bytes.len() && matches!(bytes[i], b'+' | b'-');
    let negative = signed && bytes[i] == b'-';
    if signed {
        i += 1;
    }
    let start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let digits = &input[start..i];
    let rest = &input[i..];
    if digits.is_empty() {
        return Err("invalid calendar date".into());
    }
    // MM-DD 形式（无年份）
    if digits.len() == 2 {
        if !rest.starts_with('-') {
            return Err("invalid calendar date".into());
        }
        let day_part = &rest[1..];
        if day_part.len() != 2 || !day_part.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("invalid calendar day".into());
        }
        let month: u32 = digits.parse().map_err(|_| "invalid calendar month".to_string())?;
        let day: u32 = day_part.parse().map_err(|_| "invalid calendar day".to_string())?;
        if month == 0 || month > 12 || day == 0 || day > 31 {
            return Err("invalid calendar date".into());
        }
        return Ok(());
    }
    if !(4..=6).contains(&digits.len()) {
        return Err("invalid calendar year".into());
    }
    if negative && digits.bytes().all(|byte| byte == b'0') {
        return Err("invalid calendar negative zero year".into());
    }
    let mut rest2 = rest;
    let mut month: Option<u32> = None;
    let mut day: Option<u32> = None;
    if rest2.starts_with('-') {
        rest2 = &rest2[1..];
        if rest2.len() < 2 || !rest2.as_bytes()[..2].iter().all(u8::is_ascii_digit) {
            return Err("invalid calendar month".into());
        }
        month = Some(rest2[..2].parse().map_err(|_| "invalid calendar month".to_string())?);
        rest2 = &rest2[2..];
        if rest2.starts_with('-') {
            rest2 = &rest2[1..];
            if rest2.len() != 2 || !rest2.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("invalid calendar day".into());
            }
            day = Some(rest2.parse().map_err(|_| "invalid calendar day".to_string())?);
            rest2 = "";
        }
    }
    if !rest2.is_empty() {
        return Err("invalid trailing calendar content".into());
    }
    if month.is_some_and(|m| m == 0 || m > 12) || day.is_some_and(|d| d == 0 || d > 31) {
        return Err("invalid calendar date".into());
    }
    Ok(())
}

/// ToTemporalCalendar（宽松版，property bag 路径用）：undefined → None；
/// string → 宽松解析（白名单 ID 或 ISO 串）；Temporal 实例 → 读日历槽（不触发属性 getter）；
/// 其他类型 → TypeError。
fn temporal_calendar_id<H: VmHost>(vm: &mut H, value: JsValue) -> Result<Option<String>, JsValue> {
    if value.is_undefined() {
        return Ok(None);
    }
    if value.is_string() {
        return parse_temporal_calendar_string(&to_string(value))
            .map(Some)
            .map_err(|_| crate::error::create_range_error(vm, "invalid calendar"));
    }
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_plain_date_obj() {
                return Ok(Some(get_calendar_id(obj, 3)));
            }
            if obj.is_plain_date_time_obj() {
                return Ok(Some(get_calendar_id(obj, 4)));
            }
            if obj.is_zoned_date_time_obj() {
                return Ok(Some(get_calendar_id(obj, 2)));
            }
        }
    }
    Err(crate::error::create_type_error(vm, "invalid calendar"))
}

/// ToTemporalCalendar 严格版（构造器日历参数用）：字符串只走 18 项白名单，拒绝 ISO 串；
/// undefined / 其他分支与宽松版一致。
fn temporal_calendar_id_strict<H: VmHost>(vm: &mut H, value: JsValue) -> Result<Option<String>, JsValue> {
    if value.is_string() {
        return parse_temporal_calendar_id_strict(&to_string(value))
            .map(Some)
            .map_err(|_| crate::error::create_range_error(vm, "invalid calendar"));
    }
    temporal_calendar_id(vm, value)
}

fn plain_date_time_object_parts<H: VmHost>(
    vm: &mut H, value: JsValue, obj: &JsObject, constrain: bool, ignore_time: bool,
) -> Result<(i32, u32, u32, f64, Option<String>), JsValue> {
    let calendar_raw = temporal_option_value(vm, obj, value, "calendar")?;
    let calendar = temporal_calendar_id(vm, calendar_raw)?;

    // 先按规范顺序读取全部原始字段，暂不转换类型；PlainDate 路径忽略时间字段。
    let (
        day_raw,
        hour_raw,
        microsecond_raw,
        millisecond_raw,
        minute_raw,
        month_raw,
        month_code_raw,
        nanosecond_raw,
        second_raw,
        year_raw,
    ) = if ignore_time {
        (
            temporal_option_value(vm, obj, value, "day")?,
            JsValue::undefined(),
            JsValue::undefined(),
            JsValue::undefined(),
            JsValue::undefined(),
            temporal_option_value(vm, obj, value, "month")?,
            temporal_option_value(vm, obj, value, "monthCode")?,
            JsValue::undefined(),
            JsValue::undefined(),
            temporal_option_value(vm, obj, value, "year")?,
        )
    } else {
        (
            temporal_option_value(vm, obj, value, "day")?,
            temporal_option_value(vm, obj, value, "hour")?,
            temporal_option_value(vm, obj, value, "microsecond")?,
            temporal_option_value(vm, obj, value, "millisecond")?,
            temporal_option_value(vm, obj, value, "minute")?,
            temporal_option_value(vm, obj, value, "month")?,
            temporal_option_value(vm, obj, value, "monthCode")?,
            temporal_option_value(vm, obj, value, "nanosecond")?,
            temporal_option_value(vm, obj, value, "second")?,
            temporal_option_value(vm, obj, value, "year")?,
        )
    };

    // 缺失必填字段先抛 TypeError（先于任何 RangeError 值校验）。
    if day_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "day is required"));
    }
    if year_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "year is required"));
    }
    if month_raw.is_undefined() && month_code_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "month is required"));
    }

    // monthCode 必须是 string 类型，否则 TypeError。
    let month_code = if month_code_raw.is_undefined() {
        None
    } else {
        if !month_code_raw.is_string() {
            return Err(crate::error::create_type_error(vm, "monthCode must be a string"));
        }
        let code = to_string(month_code_raw);
        // 语法（well-formed）：M 后两位数字，可选 L 后缀。
        let digits_ok =
            code.len() >= 3 && code.starts_with('M') && code.as_bytes()[1..3].iter().all(u8::is_ascii_digit);
        let well_formed = digits_ok && (code.len() == 3 || (code.len() == 4 && code.ends_with('L')));
        if !well_formed {
            return Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
        Some((code.clone(), code.ends_with('L')))
    };

    // 数值字段类型转换：Symbol/BigInt 抛 TypeError。
    let mut convert_number = |_name: &str, raw: JsValue| -> Result<Option<f64>, JsValue> {
        if raw.is_undefined() {
            Ok(None)
        } else {
            temporal_option_number(vm, raw).map(|number| Some(number.trunc()))
        }
    };
    let (day, hour, microsecond, millisecond, minute, month, nanosecond, second, year) = (
        convert_number("day", day_raw)?,
        convert_number("hour", hour_raw)?.unwrap_or(0.0),
        convert_number("microsecond", microsecond_raw)?.unwrap_or(0.0),
        convert_number("millisecond", millisecond_raw)?.unwrap_or(0.0),
        convert_number("minute", minute_raw)?.unwrap_or(0.0),
        convert_number("month", month_raw)?,
        convert_number("nanosecond", nanosecond_raw)?.unwrap_or(0.0),
        convert_number("second", second_raw)?.unwrap_or(0.0),
        convert_number("year", year_raw)?,
    );

    // monthCode 适配 ISO 日历：仅 M01-M12，L 后缀不支持；无效抛 RangeError。
    let month_code = match month_code {
        Some((code, leap)) => {
            if leap {
                return Err(crate::error::create_range_error(vm, "monthCode is not valid for ISO calendar"));
            }
            let number = code[1..3]
                .parse::<f64>()
                .map_err(|_| crate::error::create_range_error(vm, "invalid monthCode"))?;
            if !(1.0..=12.0).contains(&number) {
                return Err(crate::error::create_range_error(vm, "monthCode is not valid for ISO calendar"));
            }
            Some(number)
        }
        None => None,
    };
    let month = match (month, month_code) {
        (Some(month), Some(code)) if month != code => {
            return Err(crate::error::create_range_error(vm, "month and monthCode disagree"));
        }
        (Some(month), _) => month,
        (None, Some(code)) => code,
        (None, None) => unreachable!(),
    };
    let day = day.ok_or_else(|| crate::error::create_type_error(vm, "day is required"))?;
    let year = year.ok_or_else(|| crate::error::create_type_error(vm, "year is required"))?;
    let mut values = [year, month, day, hour, minute, second, millisecond, microsecond, nanosecond];
    if values.iter().any(|number| !number.is_finite()) || values[1..].iter().any(|number| *number < 0.0) {
        return Err(crate::error::create_range_error(vm, "invalid date-time component"));
    }
    if constrain {
        if values[1] < 1.0 || values[2] < 1.0 {
            return Err(crate::error::create_range_error(vm, "invalid date-time component"));
        }
        values[1] = values[1].clamp(1.0, 12.0);
        values[2] = values[2].clamp(1.0, days_in_month(values[0] as i128, values[1] as i128).unwrap_or(31) as f64);
        values[3] = values[3].clamp(0.0, 23.0);
        values[4] = values[4].clamp(0.0, 59.0);
        values[5] = values[5].clamp(0.0, 59.0);
        values[6] = values[6].clamp(0.0, 999.0);
        values[7] = values[7].clamp(0.0, 999.0);
        values[8] = values[8].clamp(0.0, 999.0);
    }
    let [year, month, day, hour, minute, second, millisecond, microsecond, nanosecond] = values;
    let (year, month, day) = (year as i32, month as u32, day as u32);
    let (hour, minute, second) = (hour as u32, minute as u32, second as u32);
    let (millisecond, microsecond, nanosecond) = (millisecond as u32, microsecond as u32, nanosecond as u32);
    if !valid_iso_date(year, month, day)
        || (!ignore_time && !valid_plain_time(hour, minute, second, millisecond, microsecond, nanosecond))
    {
        return Err(crate::error::create_range_error(vm, "invalid date-time component"));
    }
    let total_ns = hour as f64 * 3_600_000_000_000.0
        + minute as f64 * 60_000_000_000.0
        + second as f64 * 1_000_000_000.0
        + millisecond as f64 * 1_000_000.0
        + microsecond as f64 * 1_000.0
        + nanosecond as f64;
    if !ignore_time && !valid_plain_date_time_range(year, month, day, total_ns) {
        return Err(crate::error::create_range_error(vm, "invalid date-time component"));
    }
    Ok((year, month, day, total_ns, calendar))
}

/// 将 ZonedDateTime 按时区偏移转换为本地 PlainDateTime 分量。
fn zoned_date_time_plain_parts<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(i32, u32, u32, f64), JsValue> {
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return Err(crate::error::create_range_error(vm, "invalid ZonedDateTime"));
    };
    let time_zone_id = to_string(obj.get_prop_at(1));
    let Some(offset_minutes) = instant_time_zone_offset(&time_zone_id) else {
        return Err(crate::error::create_range_error(vm, "invalid time zone"));
    };
    const DAY_NS: i128 = 86_400_000_000_000;
    let local_ns = epoch_ns + i128::from(offset_minutes) * 60_000_000_000;
    let days = local_ns.div_euclid(DAY_NS);
    let time_ns = local_ns.rem_euclid(DAY_NS) as f64;
    let (year, month, day) = civil_from_days(days);
    if !valid_plain_date_time_range(year as i32, month as u32, day as u32, time_ns) {
        return Err(crate::error::create_range_error(vm, "invalid date-time"));
    }
    Ok((year as i32, month as u32, day as u32, time_ns))
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

fn temporal_overflow<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<bool, JsValue> {
    let options = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    if options.is_undefined() {
        return Ok(true);
    }
    if !options.is_object() {
        return Err(crate::error::create_type_error(vm, "options must be an object"));
    }
    let ptr = options.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "options must be an object"));
    }
    let raw = temporal_option_value(vm, unsafe { &*ptr }, options, "overflow")?;
    if raw.is_undefined() {
        return Ok(true);
    }
    let value = temporal_option_string(vm, raw)?;
    match value.as_str() {
        "constrain" => Ok(true),
        "reject" => Ok(false),
        _ => Err(crate::error::create_range_error(vm, "invalid overflow")),
    }
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

/// PlainDateTime 差值单位层级：year=0 … nanosecond=9；"auto" 仅限 largestUnit。
fn plain_date_time_unit_index(value: &str) -> Option<usize> {
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

const DAY_NS: i128 = 86_400_000_000_000;

const MAX_ISO_DAY: i128 = 100_000_000;

/// 比较两个 ISO 日期，返回 -1/0/+1。
fn compare_iso_date(a: (i128, i128, i128), b: (i128, i128, i128)) -> i128 {
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
fn add_days_iso(date: (i128, i128, i128), days: i128) -> (i128, i128, i128) {
    let (y, m, d) = civil_from_days(days_from_civil(date.0, date.1, date.2) + days);
    (y, m, d)
}

/// ISO8601 日期差分解，对齐 polyfill 的 dateUntil/untilCalendar。
/// largest：0=year 1=month 2=week 3=day。
fn date_until_iso(start: (i128, i128, i128), end: (i128, i128, i128), largest: usize) -> [f64; 10] {
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
fn date_duration_sign(values: &[f64; 10]) -> i128 {
    for value in values[0..4].iter().copied() {
        if value != 0.0 {
            return if value > 0.0 { 1 } else { -1 };
        }
    }
    0
}

/// 在日期上叠加时长（年/月/周/日）。
fn add_date_duration(date: (i128, i128, i128), values: &[f64; 10]) -> (i128, i128, i128) {
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
fn nudge_window(
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
struct DifferenceSettings {
    largest_index: usize,
    smallest_index: usize,
    increment: i128,
    mode: InstantRoundingMode,
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

/// 解析差值选项（对齐 GetDifferenceSettings）：读取顺序 largestUnit →
/// roundingIncrement → roundingMode → smallestUnit。date_only 时单位限定
/// year/month/week/day，smallestUnit 缺省 "day"（含时间时缺省 "nanosecond"）。
fn parse_difference_settings<H: VmHost>(
    vm: &mut H, options_value: JsValue, date_only: bool,
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
    // auto/缺省：LargerOfTwoTemporalUnits('day', smallestUnit)。
    // 索引 0=year…3=day…9=nanosecond，更大单位取更小索引，故为 min(3, smallest)。
    let largest_index = match largest_raw {
        Some(value) if value == "auto" => smallest_index.min(3),
        Some(value) => match unit_index(&value) {
            Some(index) => index,
            None => return Err(crate::error::create_range_error(vm, "invalid largestUnit")),
        },
        None => smallest_index.min(3),
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

/// 差值核心：internal = end - start，按设置取整；since 用 NegateRoundingMode
/// 的舍入模式并在最后整体取反（不调换两端，调换会改变 0.5 边界所在的年长）。
#[allow(clippy::too_many_arguments)]
fn difference_core<H: VmHost>(
    vm: &mut H, start: (i128, i128, i128), start_time_ns: i128, end: (i128, i128, i128), end_time_ns: i128,
    settings: DifferenceSettings, since: bool,
) -> NativeResult {
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

    let mut values = if smallest_index == 9 && increment == 1 {
        // 不要求舍入：时间按最大单位拆回。
        let mut values = date_values;
        if largest_index >= 4 {
            let Some(time_values) = balance_instant_difference(time_ns, largest_index - 4) else {
                return NativeResult::Err(crate::error::create_range_error(vm, "difference is out of range"));
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
            return NativeResult::Err(crate::error::create_range_error(vm, "difference is out of range"));
        };
        let whole_days = rounded_ns / DAY_NS;
        let rem = rounded_ns % DAY_NS;
        let old_whole = total_ns / DAY_NS;
        let did_expand_days = (whole_days - old_whole).signum() == total_ns.signum();
        let mut values = date_values;
        if largest_index >= 4 {
            let Some(time_values) = balance_instant_difference(rounded_ns, largest_index - 4) else {
                return NativeResult::Err(crate::error::create_range_error(vm, "difference is out of range"));
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
                Err(()) => {
                    return NativeResult::Err(crate::error::create_range_error(vm, "difference is out of range"))
                }
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
            Err(()) => return NativeResult::Err(crate::error::create_range_error(vm, "difference is out of range")),
        };
        if did_expand && smallest_index != 2 {
            match bubble_relative_duration(sign, values, nudged_epoch, date1, time1_ns, largest_index, smallest_index) {
                Ok(bubbled) => values = bubbled,
                Err(()) => {
                    return NativeResult::Err(crate::error::create_range_error(vm, "difference is out of range"))
                }
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
    make_duration(vm, values)
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
    let settings = match parse_difference_settings(vm, options_value, false) {
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
    let _ = native_try!(plain_date_ymd(vm, args));
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
    let settings = match parse_difference_settings(vm, options_value, true) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    difference_core(
        vm,
        (i128::from(start.0), i128::from(start.1), i128::from(start.2)),
        0,
        (i128::from(other.0), i128::from(other.1), i128::from(other.2)),
        0,
        settings,
        false,
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
    let settings = match parse_difference_settings(vm, options_value, true) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    difference_core(
        vm,
        (i128::from(start.0), i128::from(start.1), i128::from(start.2)),
        0,
        (i128::from(other.0), i128::from(other.1), i128::from(other.2)),
        0,
        settings,
        true,
    )
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
}
