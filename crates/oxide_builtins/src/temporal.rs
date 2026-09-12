use chrono::{Datelike, Days, NaiveDate, Utc};

use num_traits::ToPrimitive;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

// Temporal 命名空间的最小实现子集：Temporal.Now / Temporal.Instant /
// Temporal.PlainDate / Temporal.PlainTime / Temporal.PlainDateTime /
// Temporal.PlainMonthDay / Temporal.PlainYearMonth / Temporal.ZonedDateTime。
// 内部数据按对象类型存入 prop 槽：
// Instant 存纪元纳秒（BigInt，prop 0）、PlainDate 存年/月/日/日历 ID（prop 0-3）、
// PlainTime 存午夜后纳秒（f64，prop 0）、PlainDateTime 存年/月/日/午夜后纳秒/日历 ID（prop 0-4）、
// PlainMonthDay 存月/日/参考年/日历 ID（prop 0-3）、
// PlainYearMonth 存年/月/参考日/日历 ID（prop 0-3）、
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

    // ±HH 基本偏移（"+01" / "-05"）：3 字符，hour ≤ 23，分钟恒 0。仅放宽构造器时区接受面。
    if input.len() == 3
        && matches!(input.as_bytes()[0], b'+' | b'-')
        && input.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
    {
        let hour = (input.as_bytes()[1] - b'0') as i32 * 10 + (input.as_bytes()[2] - b'0') as i32;
        if hour <= 23 {
            let sign = if input.as_bytes()[0] == b'-' { -1 } else { 1 };
            return Some((input.to_string(), sign * hour * 60));
        }
        return None;
    }

    // ±HHMM 基本偏移（"+0000" / "-0530"）：5 字符，无冒号，分钟位可选非零。4 位无冒号归一形式。
    if input.len() == 5
        && matches!(input.as_bytes()[0], b'+' | b'-')
        && input.as_bytes()[1..5].iter().all(u8::is_ascii_digit)
    {
        let hour = (input.as_bytes()[1] - b'0') as i32 * 10 + (input.as_bytes()[2] - b'0') as i32;
        let minute = (input.as_bytes()[3] - b'0') as i32 * 10 + (input.as_bytes()[4] - b'0') as i32;
        if hour <= 23 && minute <= 59 {
            let sign = if input.as_bytes()[0] == b'-' { -1 } else { 1 };
            return Some((input.to_string(), sign * (hour * 60 + minute)));
        }
        return None;
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

/// 与 instant_string_without_annotations 同一次扫描并行提取末尾时区注解（不剥离）。
///
/// # 步骤
/// 1. 仿 instant_string_without_annotations 的注解扫描循环，跳过带 `=` 的 key 注解。
/// 2. 首个无 `=` 的注解按时间区形式校验（UTC 大小写 / ±HH / ±HH:MM / ±HHMM），命中即返回。
///
/// # 边界与前提
/// - 无时区注解或注解形式非法返回 None；critical（`!` 前缀）注解同样返回（带标记）。
/// - 不修改 instant_string_without_annotations 的返回；剥离与提取各自独立扫描。
/// - IANA 命名区（如 America/New_York）通过本函数校验，由调用方 canonical_time_zone 裁决。
fn extract_time_zone_annotation(input: &str) -> Option<(String, bool)> {
    let first_annotation = input.find('[')?;
    let mut rest = &input[first_annotation..];
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

        // 带 `=` 的是 key 注解（u-ca 等），跳过继续扫描。
        if annotation.contains('=') {
            continue;
        }
        if !is_time_zone_annotation_value(annotation) {
            return None;
        }
        return Some((annotation.to_string(), critical));
    }
    None
}

/// 校验时区注解值：UTC/Z 大小写折叠、±HH / ±HH:MM / ±HHMM 数值偏移。
/// 非数值形式（IANA 命名区）原样放行，交由 canonical_time_zone 裁决。
fn is_time_zone_annotation_value(annotation: &str) -> bool {
    if annotation.eq_ignore_ascii_case("UTC") || annotation.eq_ignore_ascii_case("Z") {
        return true;
    }
    if !matches!(annotation.as_bytes().first(), Some(b'+' | b'-')) {
        return true;
    }
    let bytes = annotation.as_bytes();
    let hour_only = bytes.len() == 3 && bytes[1..3].iter().all(u8::is_ascii_digit);
    let hour_minute = bytes.len() == 6
        && bytes[3] == b':'
        && bytes[1..3].iter().all(u8::is_ascii_digit)
        && bytes[4..6].iter().all(u8::is_ascii_digit);
    let hour_minute_compact = bytes.len() == 5 && bytes[1..5].iter().all(u8::is_ascii_digit);
    if hour_only {
        return (bytes[1] - b'0') * 10 + bytes[2] - b'0' <= 23;
    }
    if hour_minute {
        let hour = (bytes[1] - b'0') * 10 + bytes[2] - b'0';
        let minute = (bytes[4] - b'0') * 10 + bytes[5] - b'0';
        return hour <= 23 && minute <= 59;
    }
    if hour_minute_compact {
        let hour = (bytes[1] - b'0') * 10 + bytes[2] - b'0';
        let minute = (bytes[3] - b'0') * 10 + bytes[4] - b'0';
        return hour <= 23 && minute <= 59;
    }
    false
}

/// 读 ZonedDateTime 的 options（disambiguation → offset 顺序），逐项 Get/转换/白名单校验。
///
/// # 步骤
/// 1. options 为 undefined 时返回默认（offset=default_offset，disambiguation=compatible）。
/// 2. 先 Get disambiguation 并立即转换 + 校验，再 Get offset 并立即转换 + 校验。
///
/// # 边界与前提
/// - default_offset 由调用方决定（from 用 reject，with 用 prefer）。
/// - options 为非对象原始值抛 TypeError；选项值不在白名单抛 RangeError。
fn zoned_date_time_options<H: VmHost>(
    vm: &mut H, args: &[u8], default_offset: &str,
) -> Result<(String, String), JsValue> {
    let options = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    if options.is_undefined() {
        return Ok((default_offset.to_string(), "compatible".to_string()));
    }
    if !options.is_object() {
        return Err(crate::error::create_type_error(vm, "options must be an object"));
    }
    let ptr = options.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "options must be an object"));
    }
    let options_obj = unsafe { &*ptr };

    // 逐个选项处理：Get → ToString → 白名单，保证用户代码可观察的读序为
    // disambiguation 完整处理完后再处理 offset。
    let disambiguation_raw = temporal_option_value(vm, options_obj, options, "disambiguation")?;
    let disambiguation = if disambiguation_raw.is_undefined() {
        "compatible".to_string()
    } else {
        let value = temporal_option_string(vm, disambiguation_raw)?;
        if !matches!(value.as_str(), "compatible" | "earlier" | "later" | "reject") {
            return Err(crate::error::create_range_error(vm, "invalid disambiguation"));
        }
        value
    };
    let offset_raw = temporal_option_value(vm, options_obj, options, "offset")?;
    let offset = if offset_raw.is_undefined() {
        default_offset.to_string()
    } else {
        let value = temporal_option_string(vm, offset_raw)?;
        if !matches!(value.as_str(), "prefer" | "use" | "ignore" | "reject") {
            return Err(crate::error::create_range_error(vm, "invalid offset"));
        }
        value
    };
    Ok((offset, disambiguation))
}

/// 从剥注解后的 Instant 主体提取字符串内数值偏移分钟数（±HH / ±HHMM / ±HH:MM / 亚秒形式）。
/// Z/z 结尾视为 0 分钟；无偏移或偏移不可提取返回 None。
fn extract_string_offset_minutes(body: &str) -> Option<i32> {
    let time_start = body.find(['T', 't', ' '])?;
    let time = &body[time_start + 1..];
    let offset_start = time
        .char_indices()
        .rev()
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index))?;
    parse_any_offset_minutes(&time[offset_start..])
}

/// 解析数值偏移串（±HH / ±HHMM / ±HH:MM / ±HHMMSS / ±HH:MM:SS，可带小数秒）为分钟数。
fn parse_any_offset_minutes(value: &str) -> Option<i32> {
    let bytes = value.as_bytes();
    let sign = match bytes.first() {
        Some(b'+') => 1_i32,
        Some(b'-') => -1_i32,
        _ => return None,
    };
    let mut cursor = 1usize;
    let hour = parse_digits(bytes, &mut cursor, 2)?;
    let colon_format = bytes.get(cursor) == Some(&b':');
    let minute = if colon_format {
        cursor += 1;
        parse_digits(bytes, &mut cursor, 2)?
    } else if matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
        parse_digits(bytes, &mut cursor, 2)?
    } else {
        0
    };
    let second = if colon_format && bytes.get(cursor) == Some(&b':') {
        cursor += 1;
        parse_digits(bytes, &mut cursor, 2)?
    } else if !colon_format && matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
        parse_digits(bytes, &mut cursor, 2)?
    } else {
        0
    };
    if matches!(bytes.get(cursor), Some(b'.' | b',')) {
        cursor += 1;
        while matches!(bytes.get(cursor), Some(byte) if byte.is_ascii_digit()) {
            cursor += 1;
        }
    }
    if cursor != bytes.len() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(sign * (hour * 60 + minute) as i32)
}

/// 从 ZDT 对象 / ISO 字符串 / property bag 解析 (epoch_ns, time_zone_id, calendar_id) 三元组。
///
/// # 步骤
/// 1. ZDT 对象直拷三槽；字符串按 instant 解析 + 时区注解提取 + offset 选项决策。
/// 2. 其余对象按字段 bag 解析：timeZone 必填、offset 可选，冲突按 offset 选项处理。
/// 3. 非字符串原始值抛 TypeError。
///
/// # 边界与前提
/// - 字符串解析失败抛 RangeError；epoch 越 Instant 界抛 RangeError。
/// - offset_mode 为 prefer/use/ignore/reject；disambiguation 本批仅校验不参与算法。
fn zoned_date_time_like_epoch_ns<H: VmHost>(
    vm: &mut H, value: JsValue, offset_mode: &str, _disambiguation: &str,
) -> Result<(i128, String, String), JsValue> {
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_zoned_date_time_obj() {
                let epoch_ns = get_instant_epoch_ns(obj)
                    .ok_or_else(|| crate::error::create_range_error(vm, "invalid ZonedDateTime"))?;
                let time_zone_id = to_string(obj.get_prop_at(1));
                let calendar_id = get_calendar_id(obj, 2);
                return Ok((epoch_ns, time_zone_id, calendar_id));
            }
            return zoned_date_time_bag_parts(vm, value, obj, offset_mode);
        }
    }
    if value.is_string() {
        return zoned_date_time_string_parts(vm, &to_string(value), offset_mode);
    }
    Err(crate::error::create_type_error(vm, "cannot convert to ZonedDateTime"))
}

/// 字符串分支：instant 解析 + 时区注解提取，按 offset 选项决定最终 epoch。
///
/// # 步骤
/// 1. extract_time_zone_annotation 取注解 → canonical_time_zone 得时区 ID 与偏移。
/// 2. 主体剥注解后按 Z / 数值偏移 / 无偏移分路：Z 定 exact time，有偏移经 parse_instant_string
///    反推墙钟，无偏移直接解析墙钟。
/// 3. 按 offset 选项 use/ignore/prefer/reject 决策 epoch，并做 Instant 范围校验。
///
/// # 边界与前提
/// - 字符串缺时区注解或注解无法规范化抛 RangeError；主体非法抛 RangeError。
/// - Z 时区标识使 offsetBehaviour 为 exact：字符串偏移被忽略，epoch 恒为墙钟时刻。
fn zoned_date_time_string_parts<H: VmHost>(
    vm: &mut H, input: &str, offset_mode: &str,
) -> Result<(i128, String, String), JsValue> {
    let Some((tz_annotation, _critical)) = extract_time_zone_annotation(input) else {
        return Err(crate::error::create_range_error(vm, "invalid time zone"));
    };
    let Some((time_zone_id, time_zone_offset)) = canonical_time_zone(&tz_annotation) else {
        return Err(crate::error::create_range_error(vm, "invalid time zone"));
    };
    let Some(body) = instant_string_without_annotations(input) else {
        return Err(crate::error::create_range_error(vm, "invalid ISO 8601 date-time"));
    };
    let has_utc_designator = body.ends_with(['Z', 'z']);
    let string_offset_minutes = if has_utc_designator { Some(0) } else { extract_string_offset_minutes(body) };

    // 有偏移/Z 时经 parse_instant_string 得 epoch 并反推墙钟；否则直接解析墙钟。
    const DAY_NS: i128 = 86_400_000_000_000;
    let (epoch_from_string, wall_parts) = if string_offset_minutes.is_some() {
        let epoch_ns = parse_instant_string(input)
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid ISO 8601 date-time"))?;
        let offset_minutes = string_offset_minutes.unwrap_or(0);
        let wall_ns = epoch_ns + i128::from(offset_minutes) * 60_000_000_000;
        let days = wall_ns.div_euclid(DAY_NS);
        let (year, month, day) = civil_from_days(days);
        (Some(epoch_ns), (year as i32, month as u32, day as u32, wall_ns.rem_euclid(DAY_NS) as f64))
    } else {
        let (year, month, day, total_ns) = parse_temporal_string_impl(body, true)
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 date-time"))?;
        (None, (year, month, day, total_ns))
    };
    // 墙钟日范围校验（CheckISODaysRange）：本地日期超出 ±10^8 天直接拒绝，
    // 即使换算回 epoch 仍在 Instant 界内（offset 能把越界墙钟拉回界内）。
    if days_from_civil(i128::from(wall_parts.0), i128::from(wall_parts.1), i128::from(wall_parts.2)).abs() > MAX_ISO_DAY
    {
        return Err(crate::error::create_range_error(vm, "date-time out of range"));
    }
    let wall_epoch =
        |offset_minutes: i32| local_to_epoch_ns(wall_parts.0, wall_parts.1, wall_parts.2, wall_parts.3, offset_minutes);

    // 按 offsetBehaviour 决策：Z → exact（墙钟 epoch）；无偏移 → wall（墙钟 + 时区偏移）；
    // 有偏移 → 按 offset 选项在字符串偏移与时区偏移之间选择。
    let epoch_ns = if has_utc_designator {
        epoch_from_string
    } else {
        match string_offset_minutes {
            None => wall_epoch(time_zone_offset),
            Some(offset) => match offset_mode {
                "use" => epoch_from_string,
                "ignore" => wall_epoch(time_zone_offset),
                "prefer" => {
                    if offset == time_zone_offset {
                        epoch_from_string
                    } else {
                        wall_epoch(time_zone_offset)
                    }
                }
                "reject" => {
                    if offset == time_zone_offset {
                        epoch_from_string
                    } else {
                        return Err(crate::error::create_range_error(vm, "offset and time zone disagree"));
                    }
                }
                _ => unreachable!(),
            },
        }
    }
    .ok_or_else(|| crate::error::create_range_error(vm, "invalid date-time"))?;
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    }
    Ok((epoch_ns, time_zone_id, "iso8601".to_string()))
}

/// property bag 分支：读 timeZone/offset 与年月日字段，按 timeZone 偏移换算 epoch。
///
/// # 步骤
/// 1. timeZone 必填：缺失 TypeError，经 canonical_time_zone 规范化。
/// 2. offset 可选：先做语法校验（RangeError），再按 offset 选项与 timeZone 偏移比对。
/// 3. plain_date_time_object_parts 读年月日与日历，local_to_epoch_ns 换算并校验 Instant 范围。
///
/// # 边界与前提
/// - timeZone/offset 在字段读取前取用（读序对齐 order-of-operations 的前置约定）。
/// - offset 字段语法校验先于 year 等数值字段类型校验；匹配校验在其后。
fn zoned_date_time_bag_parts<H: VmHost>(
    vm: &mut H, value: JsValue, obj: &JsObject, offset_mode: &str,
) -> Result<(i128, String, String), JsValue> {
    let time_zone_raw = temporal_option_value(vm, obj, value, "timeZone")?;
    if time_zone_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "timeZone is required"));
    }
    let time_zone_input = temporal_option_string(vm, time_zone_raw)?;
    let Some((time_zone_id, time_zone_offset)) = canonical_time_zone(&time_zone_input) else {
        return Err(crate::error::create_range_error(vm, "invalid time zone"));
    };

    // offset 可选：语法校验（ToOffsetString 语义）先于数值字段转换。
    let offset_raw = temporal_option_value(vm, obj, value, "offset")?;
    let bag_offset_minutes = if offset_raw.is_undefined() {
        None
    } else {
        let offset_input = temporal_option_string(vm, offset_raw)?;
        Some(
            instant_time_zone_offset(&offset_input)
                .ok_or_else(|| crate::error::create_range_error(vm, "invalid offset"))?,
        )
    };

    // 年月日字段与 calendar：内部先读 calendar 再按字母序读字段并做类型转换校验。
    let (year, month, day, total_ns, calendar) = plain_date_time_object_parts(vm, value, obj, false, false)?;

    // 按 offset 选项决定 epoch（InterpretISODateTimeOffset 固定偏移简化：候选恒唯一）。
    let epoch_ns = match (bag_offset_minutes, offset_mode) {
        (None, _) | (Some(_), "ignore") => local_to_epoch_ns(year, month, day, total_ns, time_zone_offset),
        (Some(offset), "use") => local_to_epoch_ns(year, month, day, total_ns, offset),
        (Some(offset), "prefer") => {
            if offset == time_zone_offset {
                local_to_epoch_ns(year, month, day, total_ns, offset)
            } else {
                local_to_epoch_ns(year, month, day, total_ns, time_zone_offset)
            }
        }
        (Some(offset), "reject") => {
            if offset == time_zone_offset {
                local_to_epoch_ns(year, month, day, total_ns, offset)
            } else {
                return Err(crate::error::create_range_error(vm, "offset and time zone disagree"));
            }
        }
        _ => unreachable!(),
    }
    .ok_or_else(|| crate::error::create_range_error(vm, "invalid date-time"))?;
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    }
    Ok((epoch_ns, time_zone_id, calendar.unwrap_or_else(|| "iso8601".to_string())))
}

/// `Temporal.ZonedDateTime.from(item, options)`：从 ZDT 对象、ISO 字符串或 property bag 创建副本。
///
/// # 步骤
/// 1. 读 options（disambiguation → offset 顺序），先 Get 后统一白名单校验。
/// 2. ZDT 对象直拷三槽；字符串走 instant 解析 + 时区注解；其他对象走字段 bag。
/// 3. make_zoned_date_time 组装新对象。
///
/// # 边界与前提
/// - offset 默认 reject：字符串内偏移与注解时区不一致抛 RangeError；bag 内 offset 冲突同理。
/// - 字符串缺时区注解、bag 缺 timeZone 字段均抛错；number 等原始值抛 TypeError。
/// - disambiguation 本批仅做选项值校验，固定偏移时区下四个取值算法等价。
pub fn zoned_date_time_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (offset_mode, disambiguation) = native_try!(zoned_date_time_options(vm, args, "reject"));
    let (epoch_ns, time_zone_id, calendar_id) =
        native_try!(zoned_date_time_like_epoch_ns(vm, value, &offset_mode, &disambiguation));
    make_zoned_date_time(vm, epoch_ns, &time_zone_id, &calendar_id)
}

/// `Temporal.ZonedDateTime.compare(one, two)`：按纪元纳秒比较两 ZDT，忽略时区 ID 与日历。
///
/// # 步骤
/// 1. 两参数各经 zoned_date_time_like_epoch_ns 解析（默认 offset=reject + disambiguation=compatible）。
/// 2. 仅比较 epoch 纳秒，返回 -1/0/1。
///
/// # 边界与前提
/// - 参数可为 ZDT 对象 / ISO 字符串 / property bag；解析失败抛错。
/// - 同 epoch 不同时区或日历恒相等（比较不读时区/日历槽）。
pub fn zoned_date_time_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let one = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let two = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let one = native_try!(zoned_date_time_like_epoch_ns(vm, one, "reject", "compatible"));
    let two = native_try!(zoned_date_time_like_epoch_ns(vm, two, "reject", "compatible"));
    let ordering = one.0.cmp(&two.0);
    NativeResult::Ok(JsValue::int(match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }))
}

/// 时区注解文本：命名区原样返回，偏移区规范化为 `±HH:MM` 带冒号；critical 时 `!` 置于括号内。
fn format_time_zone_annotation(time_zone_id: &str, critical: bool) -> String {
    let inner = if matches!(time_zone_id.as_bytes().first(), Some(b'+') | Some(b'-')) {
        let offset_minutes = instant_time_zone_offset(time_zone_id).unwrap_or(0);
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let magnitude = offset_minutes.abs();
        format!("{sign}{:02}:{:02}", magnitude / 60, magnitude % 60)
    } else {
        time_zone_id.to_string()
    };
    if critical {
        format!("[!{inner}]")
    } else {
        format!("[{inner}]")
    }
}

/// ZDT 字符串化核心：epoch 域已舍入后按偏移与注解选项拼 `{date}T{time}.fff{offset}[{tz}][{ca}]`。
///
/// # 边界与前提
/// - epoch_ns 须已按 quantum 舍入（调用方保证），此处仅做本地墙钟分解。
/// - offset_name/time_zone_name 取 auto/never/critical，calendar_name 取 auto/never/always/critical。
/// - 偏移段恒 `±HH:MM` 带冒号；时区注解命名区原样、偏移区规范化。
#[expect(clippy::too_many_arguments)]
fn format_zoned_date_time_iso(
    epoch_ns: i128, offset_minutes: i32, time_zone_id: &str, calendar_id: &str, include_seconds: bool,
    output_digits: Option<usize>, offset_name: &str, time_zone_name: &str, calendar_name: &str,
) -> Option<String> {
    const DAY_NS: i128 = 86_400_000_000_000;
    let offset_ns = i128::from(offset_minutes).checked_mul(60_000_000_000)?;
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
        match output_digits {
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

    // offset 段：auto/always 显示，never 省略，critical 段前加 !。
    if offset_name != "never" {
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let magnitude = offset_minutes.abs();
        if offset_name == "critical" {
            output.push('!');
        }
        output.push_str(&format!("{sign}{:02}:{:02}", magnitude / 60, magnitude % 60));
    }

    // timeZoneName 注解：auto 显示，never 省略，critical 时 `!` 置于括号内。
    if time_zone_name != "never" {
        output.push_str(&format_time_zone_annotation(time_zone_id, time_zone_name == "critical"));
    }

    // calendarName 注解：always/critical 显示，auto/never 省略，critical 时 `!` 置于括号内。
    if calendar_name == "always" || calendar_name == "critical" {
        if calendar_name == "critical" {
            output.push_str(&format!("[!u-ca={calendar_id}]"));
        } else {
            output.push_str(&format!("[u-ca={calendar_id}]"));
        }
    }
    Some(output)
}

/// `Temporal.ZonedDateTime.prototype.toString(options)`：epoch 域舍入后按六项 options 输出 ISO 8601。
pub fn zoned_date_time_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ZonedDateTime"));
    };
    let time_zone_id = to_string(obj.get_prop_at(1));
    let offset_minutes = native_try!(zoned_date_time_offset_minutes(vm, obj));
    let calendar_id = get_calendar_id(obj, 2);
    let options_value = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };

    // 六项 options 按规范顺序先全部 Get：calendarName → timeZoneName → offset →
    // fractionalSecondDigits → roundingMode → smallestUnit。
    let (calendar_name, time_zone_name, offset_name, fractional_input, mode_value, smallest_value) = if options_value
        .is_undefined()
    {
        (
            "auto".to_string(),
            "auto".to_string(),
            "auto".to_string(),
            FractionalSecondDigitsInput::Auto,
            "trunc".to_string(),
            None,
        )
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
        let calendar = if calendar_raw.is_undefined() {
            "auto".to_string()
        } else {
            native_try!(temporal_option_string(vm, calendar_raw))
        };
        let tz_raw = native_try!(temporal_option_value(vm, options, options_value, "timeZoneName"));
        let tz_name = if tz_raw.is_undefined() {
            "auto".to_string()
        } else {
            native_try!(temporal_option_string(vm, tz_raw))
        };
        let offset_raw = native_try!(temporal_option_value(vm, options, options_value, "offset"));
        let offset_name = if offset_raw.is_undefined() {
            "auto".to_string()
        } else {
            native_try!(temporal_option_string(vm, offset_raw))
        };
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
        (calendar, tz_name, offset_name, fractional, mode, smallest)
    };

    // 白名单校验（全部 Get 完成后统一校验）。
    if !matches!(offset_name.as_str(), "auto" | "never" | "always" | "critical") {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid offset option"));
    }
    if !matches!(time_zone_name.as_str(), "auto" | "never" | "critical") {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid timeZoneName option"));
    }
    if !matches!(calendar_name.as_str(), "auto" | "never" | "always" | "critical") {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid calendarName option"));
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
    let Some(rounded_ns) = round_instant_ns(epoch_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    };
    if rounded_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    }
    match format_zoned_date_time_iso(
        rounded_ns,
        offset_minutes,
        &time_zone_id,
        &calendar_id,
        include_seconds,
        output_digits,
        &offset_name,
        &time_zone_name,
        &calendar_name,
    ) {
        Some(output) => NativeResult::Ok(vm.new_string_owned(output)),
        None => NativeResult::Err(crate::error::create_range_error(vm, "invalid ZonedDateTime")),
    }
}

/// `Temporal.ZonedDateTime.prototype.toJSON()`：输出默认 toString 字符串，忽略参数。
pub fn zoned_date_time_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let receiver = args.first().copied().unwrap_or(0);
    zoned_date_time_to_string(vm, &[receiver])
}

/// `Temporal.ZonedDateTime.prototype.toLocaleString()`：当前使用稳定的默认 ISO 表示。
pub fn zoned_date_time_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let receiver = args.first().copied().unwrap_or(0);
    zoned_date_time_to_string(vm, &[receiver])
}

/// `Temporal.ZonedDateTime.prototype.valueOf()`：Temporal 对象禁止转原始值。
pub fn zoned_date_time_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.ZonedDateTime has no valueOf"))
}

/// `Temporal.ZonedDateTime.prototype.withTimeZone(timeZone)`：返回同一 instant 换时区槽的新 ZDT。
///
/// # 步骤
/// 1. branding 校验 receiver 为 ZDT。
/// 2. 读槽 0 epoch 与槽 2 calendar（保不变），参数经 canonical_time_zone 解析新时区。
/// 3. make_zoned_date_time 重建对象，仅替换时区槽。
///
/// # 边界与前提
/// - 参数须为字符串；非字符串抛 TypeError。无法解析的时区串抛 RangeError。
/// - epoch 与 calendar 槽原样保留，仅时区 ID 变化。
pub fn zoned_date_time_with_time_zone<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ZonedDateTime"));
    };
    let calendar_id = get_calendar_id(obj, 2);
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
    make_zoned_date_time(vm, epoch_ns, &time_zone_id, &calendar_id)
}

/// `Temporal.ZonedDateTime.prototype.equals(other)`：与另一 ZDT 按 epoch/时区/日历三槽比较。
///
/// # 边界与前提
/// - receiver 须为 ZDT，否则 TypeError。
/// - 参数仅支持 ZDT 对象：三槽（epoch/时区/日历）全等才返回 true。
/// - 非 ZDT 对象或非对象参数：S3 基础范围外，返回 false（完整比较语义待后续）。
pub fn zoned_date_time_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));

    let other = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    if !other.is_object() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let other_ptr = other.as_js_object_ptr();
    if other_ptr.is_null() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let other_obj = unsafe { &*other_ptr };
    if !other_obj.is_zoned_date_time_obj() {
        return NativeResult::Ok(JsValue::bool(false));
    }
    let epoch_equal = get_instant_epoch_ns(obj) == get_instant_epoch_ns(other_obj);
    let zone_equal = to_string(obj.get_prop_at(1)) == to_string(other_obj.get_prop_at(1));
    let calendar_equal = get_calendar_id(obj, 2) == get_calendar_id(other_obj, 2);
    NativeResult::Ok(JsValue::bool(epoch_equal && zone_equal && calendar_equal))
}

/// with 字段合并的中间结果：未钳制的合并分量 + partial month/monthCode + bag offset。
struct ZdtMergedFields {
    year: i32,
    month: Option<f64>,
    day: f64,
    hour: f64,
    minute: f64,
    second: f64,
    millisecond: f64,
    microsecond: f64,
    nanosecond: f64,
    month_code: Option<(f64, bool)>,
    bag_offset: Option<i32>,
}

/// ToPrimitive(String) 后要求结果为字符串，否则 TypeError（ParseMonthCode/ToOffsetString 语义）。
fn temporal_string_strict<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
    let primitive = oxide_runtime_api::to_primitive(value, oxide_runtime_api::ToPrimitiveHint::String, vm)
        .map_err(|error| native_engine_error(vm, &error))?;
    if !primitive.is_string() {
        return Err(crate::error::create_type_error(vm, "expected a string"));
    }
    Ok(to_string(primitive))
}

/// RejectObjectWithCalendarOrTimeZone：非对象 / Temporal 实例 / calendar、timeZone 非 undefined
/// 均返回 TypeError（with 的 partial 参数校验）。
///
/// # 边界与前提
/// - Temporal 内部槽检查先于 calendar/timeZone 的 Get（IsPartialTemporalObject 步骤序）。
/// - 参数校验通过后调用方才能读字段。
fn reject_partial_object_with_calendar_or_time_zone<H: VmHost>(vm: &mut H, value: JsValue) -> Result<(), JsValue> {
    if !value.is_object() {
        return Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_plain_date_obj()
        || obj.is_plain_date_time_obj()
        || obj.is_plain_time_obj()
        || obj.is_zoned_date_time_obj()
        || obj.is_plain_month_day_obj()
        || obj.is_plain_year_month_obj()
    {
        return Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let calendar = temporal_option_value(vm, obj, value, "calendar")?;
    if !calendar.is_undefined() {
        return Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let time_zone = temporal_option_value(vm, obj, value, "timeZone")?;
    if !time_zone.is_undefined() {
        return Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    Ok(())
}

/// with 的部分字段读取与合并：按字典序读 bag 字段，与 receiver 默认分量合并。
///
/// # 步骤
/// 1. 数值字段 ToNumber→trunc（NaN/±Inf RangeError，day 额外拒绝 <1）；monthCode/offset 走 ToString。
/// 2. undefined 不覆盖；至少一个字段有定义否则 TypeError。
/// 3. 返回合并后的原始分量（未钳制）+ monthCode 解析 + bag offset 分钟。
///
/// # 边界与前提
/// - 调用方须先完成 RejectObjectWithCalendarOrTimeZone（calendar/timeZone 已拒绝）。
/// - monthCode 的闰月/超界/与 month 冲突校验延迟到选项解析后（对齐 spec 读序）。
/// - offset 经 ToOffsetString：非字符串 TypeError、坏格式 RangeError。
fn zoned_date_time_with_fields<H: VmHost>(
    vm: &mut H, value: JsValue, defaults: (i32, u32, u32, f64),
) -> Result<ZdtMergedFields, JsValue> {
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let obj = unsafe { &*ptr };

    // 读序（字典序）：day → hour → microsecond → millisecond → minute → month →
    // monthCode → nanosecond → offset → second → year。
    let day_raw = temporal_option_value(vm, obj, value, "day")?;
    let hour_raw = temporal_option_value(vm, obj, value, "hour")?;
    let microsecond_raw = temporal_option_value(vm, obj, value, "microsecond")?;
    let millisecond_raw = temporal_option_value(vm, obj, value, "millisecond")?;
    let minute_raw = temporal_option_value(vm, obj, value, "minute")?;
    let month_raw = temporal_option_value(vm, obj, value, "month")?;
    let month_code_raw = temporal_option_value(vm, obj, value, "monthCode")?;
    let nanosecond_raw = temporal_option_value(vm, obj, value, "nanosecond")?;
    let offset_raw = temporal_option_value(vm, obj, value, "offset")?;
    let second_raw = temporal_option_value(vm, obj, value, "second")?;
    let year_raw = temporal_option_value(vm, obj, value, "year")?;

    // 至少一个字段有定义，否则 TypeError（object-must-contain-at-least-one-property）。
    if [
        day_raw,
        hour_raw,
        microsecond_raw,
        millisecond_raw,
        minute_raw,
        month_raw,
        month_code_raw,
        nanosecond_raw,
        offset_raw,
        second_raw,
        year_raw,
    ]
    .iter()
    .all(|raw| raw.is_undefined())
    {
        return Err(crate::error::create_type_error(vm, "no properties present"));
    }

    // 数值字段：ToNumber→trunc；NaN/±Inf RangeError；day 额外要求 ≥1。
    let convert_integer = |vm: &mut H, raw: JsValue| -> Result<Option<f64>, JsValue> {
        if raw.is_undefined() {
            Ok(None)
        } else {
            let number = temporal_option_number(vm, raw)?;
            if !number.is_finite() {
                return Err(crate::error::create_range_error(vm, "invalid date-time component"));
            }
            Ok(Some(number.trunc()))
        }
    };
    let day = convert_integer(vm, day_raw)?;
    let hour = convert_integer(vm, hour_raw)?;
    let microsecond = convert_integer(vm, microsecond_raw)?;
    let millisecond = convert_integer(vm, millisecond_raw)?;
    let minute = convert_integer(vm, minute_raw)?;
    let month = convert_integer(vm, month_raw)?;
    let nanosecond = convert_integer(vm, nanosecond_raw)?;
    let second = convert_integer(vm, second_raw)?;
    let year = convert_integer(vm, year_raw)?;
    if let Some(day) = day {
        if day < 1.0 {
            return Err(crate::error::create_range_error(vm, "invalid date-time component"));
        }
    }

    // monthCode：ToString 后格式校验（M + 两位数字 + 可选 L），闰月/超界留到解析阶段。
    let month_code = if month_code_raw.is_undefined() {
        None
    } else {
        let code = temporal_string_strict(vm, month_code_raw)?;
        let digits_ok =
            code.len() >= 3 && code.starts_with('M') && code.as_bytes()[1..3].iter().all(u8::is_ascii_digit);
        let well_formed = digits_ok && (code.len() == 3 || (code.len() == 4 && code.ends_with('L')));
        if !well_formed {
            return Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
        let number = code[1..3]
            .parse::<f64>()
            .map_err(|_| crate::error::create_range_error(vm, "invalid monthCode"))?;
        Some((number, code.ends_with('L')))
    };

    // offset：ToString → 字符串语法校验（小数秒 ≤9 位），分钟数供 offset 选项决策。
    let bag_offset = if offset_raw.is_undefined() {
        None
    } else {
        let offset_input = temporal_string_strict(vm, offset_raw)?;
        let minutes = parse_any_offset_minutes(&offset_input)
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid offset"))?;
        if !valid_offset_fraction(&offset_input) {
            return Err(crate::error::create_range_error(vm, "invalid offset"));
        }
        Some(minutes)
    };

    let (receiver_year, _receiver_month, receiver_day, receiver_time_ns) = defaults;
    let (rh, rm, rs, rms, rus, rns) = plain_time_components(receiver_time_ns);
    Ok(ZdtMergedFields {
        year: year.unwrap_or(receiver_year as f64) as i32,
        month,
        day: day.unwrap_or(receiver_day as f64),
        hour: hour.unwrap_or(rh as f64),
        minute: minute.unwrap_or(rm as f64),
        second: second.unwrap_or(rs as f64),
        millisecond: millisecond.unwrap_or(rms as f64),
        microsecond: microsecond.unwrap_or(rus as f64),
        nanosecond: nanosecond.unwrap_or(rns as f64),
        month_code,
        bag_offset,
    })
}

/// `Temporal.ZonedDateTime.prototype.with(temporalZonedDateTimeLike, options)`。
///
/// # 步骤
/// 1. branding + IsPartialTemporalObject（calendar/timeZone/Temporal 实例 → TypeError）。
/// 2. receiver 本地分量与偏移；zoned_date_time_with_fields 读字段合并。
/// 3. options：disambiguation → offset（默认 prefer）→ overflow（逐项 Get/校验）。
/// 4. monthCode 闰月/超界/冲突校验 + constrain/reject 钳制 + PlainDateTime 范围校验。
/// 5. offset 选项决策 → local_to_epoch_ns → Instant 范围校验 → make_zoned_date_time。
///
/// # 边界与前提
/// - 字段读取先于 options 解析（options-wrong-type 先报字段错误）。
/// - 选项解析先于 monthCode 算法校验（options-read-before-algorithmic-validation）。
/// - disambiguation 在固定偏移时区下四取值算法等价，仅做白名单校验。
pub fn zoned_date_time_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    native_try!(reject_partial_object_with_calendar_or_time_zone(vm, value));

    let time_zone_id = to_string(obj.get_prop_at(1));
    let offset_minutes = native_try!(zoned_date_time_offset_minutes(vm, obj));
    let (year, month, day, time_ns) = native_try!(zoned_date_time_plain_parts(vm, obj));
    let merged = native_try!(zoned_date_time_with_fields(vm, value, (year, month, day, time_ns)));

    // options：字段读取完成后解析（disambiguation → offset → overflow）。
    let (offset_mode, _disambiguation) = native_try!(zoned_date_time_options(vm, args, "prefer"));
    let constrain = native_try!(temporal_overflow(vm, args));

    // CalendarResolveFields（ISO）：partial 的 monthCode 闰月 / 超 12 拒绝；month 与
    // monthCode 冲突仅在两者都来自 partial 时校验（partial 有 monthCode 时 receiver
    // 的 month 被覆盖，不参与比较）。
    let receiver_month_f = month as f64;
    let merged_month = match (merged.month, merged.month_code) {
        (_, Some((_, true))) => {
            return NativeResult::Err(crate::error::create_range_error(vm, "monthCode is not valid for ISO calendar"));
        }
        (Some(month), Some((code, false))) => {
            if code > 12.0 {
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "monthCode is not valid for ISO calendar",
                ));
            }
            if month != code {
                return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode disagree"));
            }
            code
        }
        (None, Some((code, false))) => {
            if code > 12.0 {
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "monthCode is not valid for ISO calendar",
                ));
            }
            code
        }
        (Some(month), None) => month,
        (None, None) => receiver_month_f,
    };

    // RegulateISODate：constrain 钳制月/日，reject 直接校验。
    let (year, month, day) = if constrain {
        let month = merged_month.clamp(1.0, 12.0);
        let max_day = days_in_month(i128::from(merged.year), month as i128).unwrap_or(31) as f64;
        (merged.year, month, merged.day.clamp(1.0, max_day))
    } else {
        if !valid_iso_date(merged.year, merged_month as u32, merged.day as u32) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time component"));
        }
        (merged.year, merged_month, merged.day)
    };

    // RegulateTime：constrain 钳制时/分/秒/亚秒，reject 直接校验。
    let (hour, minute, second, millisecond, microsecond, nanosecond) = if constrain {
        (
            merged.hour.clamp(0.0, 23.0),
            merged.minute.clamp(0.0, 59.0),
            merged.second.clamp(0.0, 59.0),
            merged.millisecond.clamp(0.0, 999.0),
            merged.microsecond.clamp(0.0, 999.0),
            merged.nanosecond.clamp(0.0, 999.0),
        )
    } else {
        let values = [
            merged.hour,
            merged.minute,
            merged.second,
            merged.millisecond,
            merged.microsecond,
            merged.nanosecond,
        ];
        if !valid_plain_time(
            values[0] as u32,
            values[1] as u32,
            values[2] as u32,
            values[3] as u32,
            values[4] as u32,
            values[5] as u32,
        ) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid time component"));
        }
        (values[0], values[1], values[2], values[3], values[4], values[5])
    };
    let (year, month, day) = (year, month as u32, day as u32);
    let total_ns = hour * 3_600_000_000_000.0
        + minute * 60_000_000_000.0
        + second * 1_000_000_000.0
        + millisecond * 1_000_000.0
        + microsecond * 1_000.0
        + nanosecond;
    if !valid_plain_date_time_range(year, month, day, total_ns) {
        return NativeResult::Err(crate::error::create_range_error(vm, "date-time out of range"));
    }

    // InterpretISODateTimeOffset（固定偏移简化）：按 offset 选项在 bag 偏移与时区偏移间选择。
    let epoch_ns = match (merged.bag_offset, offset_mode.as_str()) {
        (None, _) => local_to_epoch_ns(year, month, day, total_ns, offset_minutes),
        (Some(offset), "use") => local_to_epoch_ns(year, month, day, total_ns, offset),
        (Some(_), "ignore") => local_to_epoch_ns(year, month, day, total_ns, offset_minutes),
        (Some(offset), "prefer") => {
            if offset == offset_minutes {
                local_to_epoch_ns(year, month, day, total_ns, offset)
            } else {
                local_to_epoch_ns(year, month, day, total_ns, offset_minutes)
            }
        }
        (Some(offset), "reject") => {
            if offset == offset_minutes {
                local_to_epoch_ns(year, month, day, total_ns, offset)
            } else {
                return NativeResult::Err(crate::error::create_range_error(vm, "offset and time zone disagree"));
            }
        }
        _ => unreachable!(),
    }
    .ok_or_else(|| crate::error::create_range_error(vm, "invalid date-time"));
    let epoch_ns = native_try!(epoch_ns);
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    }
    let calendar_id = get_calendar_id(obj, 2);
    make_zoned_date_time(vm, epoch_ns, &time_zone_id, &calendar_id)
}

/// `Temporal.ZonedDateTime.prototype.withCalendar(calendar)`：换日历槽，epoch/时区不变。
///
/// # 步骤
/// 1. branding 校验 receiver 为 ZDT。
/// 2. 参数经 temporal_calendar_id（宽松版）解析：字符串白名单/ISO 串 → ID，
///    PlainDate/PDT/ZDT 对象读日历槽（不触发 getter）。
/// 3. make_zoned_date_time 重建对象，仅替换日历槽。
///
/// # 边界与前提
/// - 缺参 / undefined → TypeError；非字符串非对象（number/null 等）→ TypeError。
/// - 非法日历串 → RangeError；日历 ID 大小写不敏感。
pub fn zoned_date_time_with_calendar<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let calendar_value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if calendar_value.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "calendar is required"));
    }
    let calendar = native_try!(temporal_calendar_id(vm, calendar_value)).unwrap_or_else(|| "iso8601".to_string());
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ZonedDateTime"));
    };
    let time_zone_id = to_string(obj.get_prop_at(1));
    make_zoned_date_time(vm, epoch_ns, &time_zone_id, &calendar)
}

/// 校验 offset 串的小数秒位数 ≤9（parse_any_offset_minutes 不限制位数，此处补查）。
fn valid_offset_fraction(value: &str) -> bool {
    let Some(dot) = value.find(['.', ',']) else {
        return true;
    };
    let fraction = &value[dot + 1..];
    fraction.len() <= 9 && fraction.bytes().all(|byte| byte.is_ascii_digit())
}

/// 时间串的日期歧义判定：形如 YYYY-MM / MMDD / YYYYMM / MM-DD 且作为日期合法 → 歧义。
///
/// # 边界与前提
/// - 2 月按闰年处理（0229 判歧义、0230 不判）。
/// - 月/日非法（13、00、2 月 30 日等）不算歧义，按时间解析。
fn is_ambiguous_date_string(s: &str) -> bool {
    let bytes = s.as_bytes();
    let all_digits = |range: std::ops::Range<usize>| {
        bytes.get(range.clone()).is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    let two_digits = |at: usize| -> Option<i32> {
        if at + 1 >= bytes.len() {
            None
        } else {
            Some((bytes[at] - b'0') as i32 * 10 + (bytes[at + 1] - b'0') as i32)
        }
    };
    let day_in_month = |month: i32, day: i32| -> bool {
        // 2 月按闰年（29 天）判定歧义。
        let max = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => 29,
            _ => return false,
        };
        day >= 1 && day <= max
    };
    match s.len() {
        4 if all_digits(0..4) => {
            let (month, day) = (two_digits(0).unwrap(), two_digits(2).unwrap());
            day_in_month(month, day)
        }
        6 if all_digits(0..6) => (1..=12).contains(&two_digits(4).unwrap()),
        5 if bytes[2] == b'-' && all_digits(0..2) && all_digits(3..5) => {
            let (month, day) = (two_digits(0).unwrap(), two_digits(3).unwrap());
            day_in_month(month, day)
        }
        7 if bytes[4] == b'-' && all_digits(0..4) && all_digits(5..7) => (1..=12).contains(&two_digits(5).unwrap()),
        _ => false,
    }
}

/// 解析时间主体（时[:分[:秒[.小数]]] 或 HHMM / HHMMSS），闰秒按前一秒。
fn parse_plain_clock(clock: &str) -> Option<f64> {
    let (clock, fraction) = match clock.find(['.', ',']) {
        Some(index) => (&clock[..index], Some(&clock[index + 1..])),
        None => (clock, None),
    };
    let subsecond = match fraction {
        Some(digits) => {
            if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let mut padded = digits.to_string();
            padded.extend(std::iter::repeat('0').take(9 - digits.len()));
            padded.parse::<u32>().ok()?
        }
        None => 0,
    };
    let (hour, minute, second) = if clock.contains(':') {
        let fields: Vec<&str> = clock.split(':').collect();
        if fields.is_empty()
            || fields.len() > 3
            || fields
                .iter()
                .any(|field| field.len() != 2 || !field.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
        if fraction.is_some() && fields.len() < 3 {
            return None;
        }
        (
            fields[0].parse::<u32>().ok()?,
            fields.get(1).map_or(Ok(0), |field| field.parse()).ok()?,
            fields.get(2).map_or(Ok(0), |field| field.parse()).ok()?,
        )
    } else {
        if !clock.bytes().all(|byte| byte.is_ascii_digit())
            || clock.len() % 2 != 0
            || clock.len() > 6
            || clock.is_empty()
        {
            return None;
        }
        if fraction.is_some() && clock.len() < 6 {
            return None;
        }
        let read2 = |at: usize| (clock.as_bytes()[at] - b'0') as u32 * 10 + (clock.as_bytes()[at + 1] - b'0') as u32;
        (
            read2(0),
            if clock.len() >= 4 { read2(2) } else { 0 },
            if clock.len() >= 6 { read2(4) } else { 0 },
        )
    };
    let second = if second == 60 { 59 } else { second };
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(
        hour as f64 * 3_600_000_000_000.0
            + minute as f64 * 60_000_000_000.0
            + second as f64 * 1_000_000_000.0
            + subsecond as f64,
    )
}

/// ParseTemporalTimeString：时间串 → 当日纳秒（offset/注解被忽略、Z 拒绝）。
///
/// # 边界与前提
/// - Z/z designator → None；纯日期串 → None（不隐式午夜）。
/// - 日期+时间（"1976-11-18T12:34..."）→ 取 T 后时间部分；空格分隔需带日期部分。
/// - 无 T 的数字/连字符形式先做日期歧义判定：日期合法 → None（须 T 前缀）；
///   日期非法 → 按时间解析（HHMM-UU 形式，offset 被忽略）。
/// - 小数秒 ≤9 位；offset 小数秒 ≤9 位；闰秒按前一秒。
fn parse_plain_time_string(input: &str) -> Option<f64> {
    let text = input.trim();
    if text.contains('\u{2212}') {
        return None;
    }
    let body = instant_string_without_annotations(text)?;
    if body.contains(['Z', 'z']) {
        return None;
    }
    let sep = body.find(['T', 't', ' ']);
    let (date_part, time_part, has_t) = match sep {
        Some(0) => ("", &body[1..], matches!(body.as_bytes().first(), Some(b'T' | b't'))),
        Some(index) => (&body[..index], &body[index + 1..], matches!(body.as_bytes()[index], b'T' | b't')),
        None => ("", body, false),
    };
    if !has_t && sep == Some(0) {
        // 前导空格不能替代 T 前缀（无日期部分的时间串）。
        return None;
    }
    if !date_part.is_empty() {
        // 完整日期部分必须可解析（负零年等由 parse_iso_date 拒绝）。
        if parse_iso_date(date_part).is_err() {
            return None;
        }
        return parse_plain_time_spec(time_part);
    }
    if !has_t && !time_part.contains(':') && is_ambiguous_date_string(time_part) {
        return None;
    }
    parse_plain_time_spec(time_part)
}

/// 时间主体 + 尾部 offset（剥离并忽略）：offset 语法非法返回 None。
fn parse_plain_time_spec(s: &str) -> Option<f64> {
    let offset_start = s
        .char_indices()
        .rev()
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index));
    let (clock, offset) = match offset_start {
        Some(index) => (&s[..index], &s[index..]),
        None => (s, ""),
    };
    if !offset.is_empty() && (parse_any_offset_minutes(offset).is_none() || !valid_offset_fraction(offset)) {
        return None;
    }
    parse_plain_clock(clock)
}

/// ToTemporalTime：PlainTime 对象 / ZDT / PlainDateTime / 字符串 / property bag → 当日纳秒。
///
/// # 步骤
/// 1. undefined → 0（午夜）；字符串走 parse_plain_time_string（失败 RangeError）。
/// 2. PlainTime 读槽 0；ZDT 用其自身时区取本地时间；PlainDateTime 读时间槽。
/// 3. 其余对象按 ToTemporalTimeRecord 读时间字段（无字段 TypeError）。
///
/// # 边界与前提
/// - 非字符串原始值（number/bigint/null/boolean）→ TypeError；Symbol → TypeError。
/// - bag 字段缺省 0（完整模式），越界值按 constrain 钳制（second=60 → 59）。
fn plain_time_like_ns<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    if value.is_undefined() {
        return Ok(0.0);
    }
    if value.is_string() {
        return parse_plain_time_string(&to_string(value))
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid ISO 8601 time"));
    }
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_plain_time_obj() {
                return Ok(get_double_prop(obj, 0));
            }
            if obj.is_zoned_date_time_obj() {
                return zoned_date_time_plain_parts(vm, obj).map(|(_, _, _, time_ns)| time_ns);
            }
            if obj.is_plain_date_time_obj() {
                return Ok(get_double_prop(obj, 3));
            }
            return plain_time_bag_ns(vm, value, obj);
        }
    }
    Err(crate::error::create_type_error(vm, "cannot convert to PlainTime"))
}

/// ToTemporalTimeRecord（完整模式）：按字母序读时间字段，缺省 0，无字段 TypeError。
fn plain_time_bag_ns<H: VmHost>(vm: &mut H, value: JsValue, obj: &JsObject) -> Result<f64, JsValue> {
    let hour_raw = temporal_option_value(vm, obj, value, "hour")?;
    let microsecond_raw = temporal_option_value(vm, obj, value, "microsecond")?;
    let millisecond_raw = temporal_option_value(vm, obj, value, "millisecond")?;
    let minute_raw = temporal_option_value(vm, obj, value, "minute")?;
    let nanosecond_raw = temporal_option_value(vm, obj, value, "nanosecond")?;
    let second_raw = temporal_option_value(vm, obj, value, "second")?;
    if [hour_raw, microsecond_raw, millisecond_raw, minute_raw, nanosecond_raw, second_raw]
        .iter()
        .all(|raw| raw.is_undefined())
    {
        return Err(crate::error::create_type_error(vm, "no time units present"));
    }
    let mut convert = |raw: JsValue| -> Result<f64, JsValue> {
        if raw.is_undefined() {
            Ok(0.0)
        } else {
            let number = temporal_option_number(vm, raw)?;
            if !number.is_finite() {
                return Err(crate::error::create_range_error(vm, "invalid time component"));
            }
            Ok(number.trunc())
        }
    };
    let hour = convert(hour_raw)?;
    let microsecond = convert(microsecond_raw)?;
    let millisecond = convert(millisecond_raw)?;
    let minute = convert(minute_raw)?;
    let nanosecond = convert(nanosecond_raw)?;
    let second = convert(second_raw)?;
    // RegulateTime（constrain）：越界钳制（second=60 → 59）。
    let hour = hour.clamp(0.0, 23.0);
    let minute = minute.clamp(0.0, 59.0);
    let second = second.clamp(0.0, 59.0);
    let millisecond = millisecond.clamp(0.0, 999.0);
    let microsecond = microsecond.clamp(0.0, 999.0);
    let nanosecond = nanosecond.clamp(0.0, 999.0);
    Ok(hour * 3_600_000_000_000.0
        + minute * 60_000_000_000.0
        + second * 1_000_000_000.0
        + millisecond * 1_000_000.0
        + microsecond * 1_000.0
        + nanosecond)
}

/// `Temporal.ZonedDateTime.prototype.withPlainTime(plainTimeLike)`。
///
/// # 步骤
/// 1. branding + receiver 本地分量与偏移。
/// 2. plain_time_like_ns 取当日纳秒（undefined → 午夜）。
/// 3. local_to_epoch_ns 换算 + Instant 范围校验 → make_zoned_date_time（保时区/日历槽）。
///
/// # 边界与前提
/// - 本地分量越 PlainDateTime 范围 / epoch 越 Instant 界 → RangeError。
/// - ZDT 参数用其自身时区取本地时间（不用 receiver 时区）。
pub fn zoned_date_time_with_plain_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let (year, month, day, _) = native_try!(zoned_date_time_plain_parts(vm, obj));
    let offset_minutes = native_try!(zoned_date_time_offset_minutes(vm, obj));
    let plain_time_like = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let time_ns = native_try!(plain_time_like_ns(vm, plain_time_like));
    let epoch_ns = local_to_epoch_ns(year, month, day, time_ns, offset_minutes)
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid date-time"));
    let epoch_ns = native_try!(epoch_ns);
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    }
    let time_zone_id = to_string(obj.get_prop_at(1));
    let calendar_id = get_calendar_id(obj, 2);
    make_zoned_date_time(vm, epoch_ns, &time_zone_id, &calendar_id)
}

/// until/since 的 other 转换（ToTemporalZonedDateTime 语义，仅取 epoch 参与差值）。
///
/// # 步骤
/// 1. ZDT 对象直读槽 0 epoch；Instant 对象读 epoch（忽略自身时区/日历，仅 epoch 参与差值）。
/// 2. 字符串须带时区注解：注解时区 + 字符串内偏移按 reject 决策（Z→exact、无偏移→wall）。
/// 3. PlainDate/PlainDateTime/PlainTime 对象读字段，按 receiver 时区解释为墙钟。
/// 4. 其他对象按字段 bag 解析：timeZone 缺省 receiver 时区，offset 与 timeZone 冲突 RangeError。
/// 5. 其他原始值（undefined/null/boolean/number/bigint/symbol）TypeError。
///
/// # 边界与前提
/// - 字符串无注解 / epoch 越 Instant 界 / offset 冲突均抛 RangeError。
/// - bag 的 timeZone 非字符串抛 TypeError、解析失败抛 RangeError；offset 同理。
/// - PlainTime 无日期字段，以其墙钟时间落在 receiver 本地日期上。
fn zoned_date_time_other_epoch_ns<H: VmHost>(
    vm: &mut H, value: JsValue, default_tz_id: &str, default_wall: (i32, u32, u32),
) -> Result<i128, JsValue> {
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_zoned_date_time_obj() {
                return get_instant_epoch_ns(obj)
                    .ok_or_else(|| crate::error::create_range_error(vm, "invalid ZonedDateTime"));
            }
            if obj.is_instant_obj() {
                return get_instant_epoch_ns(obj)
                    .ok_or_else(|| crate::error::create_range_error(vm, "invalid Instant"));
            }
            if obj.is_plain_date_time_obj() {
                return zoned_date_time_other_wall_epoch_ns(
                    vm,
                    get_double_prop(obj, 0) as i32,
                    get_double_prop(obj, 1) as u32,
                    get_double_prop(obj, 2) as u32,
                    get_double_prop(obj, 3),
                    default_tz_id,
                );
            }
            if obj.is_plain_date_obj() {
                return zoned_date_time_other_wall_epoch_ns(
                    vm,
                    get_double_prop(obj, 0) as i32,
                    get_double_prop(obj, 1) as u32,
                    get_double_prop(obj, 2) as u32,
                    0.0,
                    default_tz_id,
                );
            }
            if obj.is_plain_time_obj() {
                return zoned_date_time_other_wall_epoch_ns(
                    vm,
                    default_wall.0,
                    default_wall.1,
                    default_wall.2,
                    get_double_prop(obj, 0),
                    default_tz_id,
                );
            }
            return zoned_date_time_other_bag_epoch_ns(vm, value, obj, default_tz_id);
        }
    }
    if value.is_string() {
        return zoned_date_time_string_parts(vm, &to_string(value), "reject").map(|(epoch_ns, _, _)| epoch_ns);
    }
    Err(crate::error::create_type_error(vm, "cannot convert to ZonedDateTime"))
}

/// 本地墙钟分量 + receiver 时区偏移 → epoch（PlainDate/PlainTime 系对象的 other 路径）。
fn zoned_date_time_other_wall_epoch_ns<H: VmHost>(
    vm: &mut H, year: i32, month: u32, day: u32, total_ns: f64, default_tz_id: &str,
) -> Result<i128, JsValue> {
    if !valid_iso_date(year, month, day) || !total_ns.is_finite() || !(0.0..86_400_000_000_000.0).contains(&total_ns) {
        return Err(crate::error::create_range_error(vm, "invalid date-time component"));
    }
    let offset_minutes = instant_time_zone_offset(default_tz_id)
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid time zone"))?;
    local_to_epoch_ns(year, month, day, total_ns, offset_minutes)
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid date-time"))
}

/// property bag 路径：timeZone 缺省 receiver 时区，offset 与 timeZone 冲突 RangeError。
///
/// # 步骤
/// 1. timeZone 可选：缺省取 receiver 时区；显式时非字符串 TypeError、解析失败 RangeError。
/// 2. offset 可选：非字符串 TypeError、坏格式 RangeError。
/// 3. plain_date_time_object_parts 读年月日时分秒字段，offset 与 timeZone 偏移比对（冲突 RangeError）。
/// 4. local_to_epoch_ns 换算并校验 Instant 范围。
fn zoned_date_time_other_bag_epoch_ns<H: VmHost>(
    vm: &mut H, value: JsValue, obj: &JsObject, default_tz_id: &str,
) -> Result<i128, JsValue> {
    let time_zone_raw = temporal_option_value(vm, obj, value, "timeZone")?;
    let time_zone_offset = if time_zone_raw.is_undefined() {
        instant_time_zone_offset(default_tz_id)
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid time zone"))?
    } else {
        if !time_zone_raw.is_string() {
            return Err(crate::error::create_type_error(vm, "invalid time zone"));
        }
        let input = to_string(time_zone_raw);
        instant_time_zone_offset(&input).ok_or_else(|| crate::error::create_range_error(vm, "invalid time zone"))?
    };

    let offset_raw = temporal_option_value(vm, obj, value, "offset")?;
    let bag_offset_minutes = if offset_raw.is_undefined() {
        None
    } else {
        if !offset_raw.is_string() {
            return Err(crate::error::create_type_error(vm, "invalid offset"));
        }
        let offset_input = to_string(offset_raw);
        Some(
            instant_time_zone_offset(&offset_input)
                .ok_or_else(|| crate::error::create_range_error(vm, "invalid offset"))?,
        )
    };

    let (year, month, day, total_ns, _calendar) = plain_date_time_object_parts(vm, value, obj, false, false)?;
    if let Some(offset) = bag_offset_minutes {
        if offset != time_zone_offset {
            return Err(crate::error::create_range_error(vm, "offset and time zone disagree"));
        }
    }
    let epoch_ns = local_to_epoch_ns(year, month, day, total_ns, time_zone_offset)
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid date-time"))?;
    if epoch_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    }
    Ok(epoch_ns)
}

/// ZDT 差值核心：两 ZDT 的 epoch 差 → receiver 时区下的本地墙钟分量差 → difference_core。
///
/// # 步骤
/// 1. receiver 三槽读取：epoch、offset 分钟、时区 ID，本地分量经 zoned_date_time_plain_parts。
/// 2. other → epoch（zoned_date_time_other_epoch_ns，receiver 时区作 bag 默认）。
/// 3. epoch 相等快速路径：先于任何分量计算返回全零时长。
/// 4. other 本地分量：epoch + offset → div_euclid/rem_euclid 拆墙钟（receiver 时区）。
/// 5. settings = parse_difference_settings(…, default_largest = 4)（ZDT 默认 largest 为 hour）。
/// 6. difference_core(vm, receiver, other, settings, since)。
fn zoned_date_time_difference<H: VmHost>(vm: &mut H, args: &[u8], since: bool) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let Some(epoch_r) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ZonedDateTime"));
    };
    let offset_min_r = native_try!(zoned_date_time_offset_minutes(vm, obj));
    let time_zone_id = to_string(obj.get_prop_at(1));
    let (y_r, m_r, d_r, time_ns_r) = native_try!(zoned_date_time_plain_parts(vm, obj));

    let other = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let epoch_o = native_try!(zoned_date_time_other_epoch_ns(vm, other, &time_zone_id, (y_r, m_r, d_r)));

    // epoch 相等 → 空时长：先于任何分量计算（同 epoch 不同 tz 本地分量不同，规范要求空结果）。
    if epoch_r == epoch_o {
        return make_duration(vm, [0.0; 10]);
    }

    // other 按 receiver 时区偏移拆本地墙钟（负 epoch 用 div_euclid/rem_euclid 保持非负余数）。
    const DAY_NS: i128 = 86_400_000_000_000;
    let local_ns_o = epoch_o + i128::from(offset_min_r) * 60_000_000_000;
    let days_o = local_ns_o.div_euclid(DAY_NS);
    let (y_o, m_o, d_o) = civil_from_days(days_o);
    let time_ns_o = local_ns_o.rem_euclid(DAY_NS);

    let options_value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let settings = native_try!(parse_difference_settings(vm, options_value, false, 4));
    difference_core(
        vm,
        (i128::from(y_r), i128::from(m_r), i128::from(d_r)),
        time_ns_r as i128,
        (y_o, m_o, d_o),
        time_ns_o,
        settings,
        since,
    )
}

/// `Temporal.ZonedDateTime.prototype.until(other, options)`：other 减 receiver 的差值时长。
pub fn zoned_date_time_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    zoned_date_time_difference(vm, args, false)
}

/// `Temporal.ZonedDateTime.prototype.since(other, options)`：receiver 减 other 的差值时长。
pub fn zoned_date_time_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    zoned_date_time_difference(vm, args, true)
}

/// ZDT round 的最小单位表：day + 6 个时间单位（不含 year/month/week）。
/// 返回 (单位纳秒, 每更高一级单位的数量)；day 的"更高一级数量"= 1（增量仅 1 合法）。
fn zoned_date_time_round_unit(value: &str) -> Option<(i128, i128)> {
    match value {
        "day" | "days" => Some((86_400_000_000_000, 1)),
        "hour" | "hours" => Some((3_600_000_000_000, 24)),
        "minute" | "minutes" => Some((60_000_000_000, 60)),
        "second" | "seconds" => Some((1_000_000_000, 60)),
        "millisecond" | "milliseconds" => Some((1_000_000, 1_000)),
        "microsecond" | "microseconds" => Some((1_000, 1_000)),
        "nanosecond" | "nanoseconds" => Some((1, 1_000)),
        _ => None,
    }
}

/// `Temporal.ZonedDateTime.prototype.round(roundTo)`：按 smallestUnit 在"本地日"域内舍入。
///
/// # 步骤
/// 1. roundTo 解析：undefined → TypeError；字符串 → {smallestUnit: 串}；对象 → 依次 Get
///    roundingIncrement → roundingMode → smallestUnit，全部读完再统一校验。
/// 2. receiver：ensure_zoned_date_time → epoch_r；instant_time_zone_offset(tz) 得 offset_min；
///    zoned_date_time_plain_parts 得 (y, m, d, time_ns)。
/// 3. 单位表查 smallestUnit（None → RangeError "invalid smallest unit"）。
/// 4. roundingIncrement 校验：1..=1e9 且真因子（increment < units_per_day 且
///    units_per_day % increment == 0，否则 RangeError）。day 仅 increment=1 合法。
/// 5. mode = instant_rounding_mode（None → RangeError）；缺省 "halfExpand"。
/// 6. day 路径（smallestUnit = day）：startNs/endNs 双算（越界 → RangeError），
///    dayProgress = epoch_r − startNs，rounded = round_instant_ns(dayProgress, DAY*increment)，
///    result = startNs + rounded。
/// 7. else 路径（时间单位）：rounded_time = round_instant_ns(time_ns, unit_ns*increment)
///    （可进位到 DAY → 本地墙钟自动跨日），result = local_to_epoch_ns(y, m, d, rounded_time)。
/// 8. 范围校验：|result| > MAX_INSTANT_NS → RangeError。
/// 9. make_zoned_date_time(vm, result, tz_id, cal_id)（保时区/日历槽）。
pub fn zoned_date_time_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let Some(epoch_r) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ZonedDateTime"));
    };
    let time_zone_id = to_string(obj.get_prop_at(1));
    let Some(offset_min) = instant_time_zone_offset(&time_zone_id) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time zone"));
    };
    let calendar_id = get_calendar_id(obj, 2);
    let (y, m, d, time_ns) = native_try!(zoned_date_time_plain_parts(vm, obj));

    // roundTo 解析：字符串简写或对象选项，读序对齐 order-of-operations。
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
    let Some((unit_ns, units_per_day)) = zoned_date_time_round_unit(&unit_value) else {
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
    // 真因子校验：时间单位增量须小于更高一级单位数量且整除（hour/24、minute/60、second/60 拒绝）；
    // day 特殊：更高一级数量=1，仅 increment==1 合法。
    let increment_valid = if unit_ns == DAY_NS {
        increment == 1
    } else {
        increment < units_per_day && units_per_day % increment == 0
    };
    if !increment_valid {
        return NativeResult::Err(crate::error::create_range_error(vm, "rounding increment must divide a day"));
    }

    const DAY_NS: i128 = 86_400_000_000_000;
    let result_ns = if unit_ns == DAY_NS {
        // day 路径：startNs/endNs 双算（越界 → RangeError），dayProgress 固定偏移下 ∈ [0, DAY)。
        let start_ns = native_try!(start_of_day_epoch_ns(y, m, d, offset_min)
            .ok_or_else(|| crate::error::create_range_error(vm, "invalid start of day")));
        native_try!(start_of_day_epoch_ns_by_days(
            days_from_civil(i128::from(y), i128::from(m), i128::from(d)) + 1,
            offset_min,
        )
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid start of day")));
        let day_progress = epoch_r - start_ns;
        let Some(quantum_ns) = DAY_NS.checked_mul(increment) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
        };
        let Some(rounded) = round_instant_ns(day_progress, quantum_ns, mode) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
        };
        start_ns + rounded
    } else {
        // else 路径：本地墙钟时间舍入（可进位到 DAY → 自动跨日），再换算回 epoch。
        let Some(quantum_ns) = unit_ns.checked_mul(increment) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
        };
        let Some(rounded_time) = round_instant_ns(time_ns as i128, quantum_ns, mode) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
        };
        let Some(result) = local_to_epoch_ns(y, m, d, rounded_time as f64, offset_min) else {
            return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
        };
        result
    };

    if result_ns.unsigned_abs() > MAX_INSTANT_NS as u128 {
        return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range"));
    }
    make_zoned_date_time(vm, result_ns, &time_zone_id, &calendar_id)
}

/// ZDT 字段 getter 宏：branding 后读本地分量，按选择函数取字段。
macro_rules! zoned_date_time_parts_getter {
    ($name:ident, $select:expr) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let ptr = native_try!(receiver_obj(vm, args));
            let obj = unsafe { &*ptr };
            native_try!(ensure_zoned_date_time(vm, obj));
            let (year, month, day, time_ns) = native_try!(zoned_date_time_plain_parts(vm, obj));
            NativeResult::Ok(JsValue::float($select(year, month, day, time_ns) as f64))
        }
    };
}

/// ZDT epoch 除法 getter 宏：槽 0 按除数向下取整（负值 floor，仿 instant_epoch_*）。
macro_rules! zoned_date_time_epoch_getter {
    ($name:ident, $divisor:expr) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let ptr = native_try!(receiver_obj(vm, args));
            let obj = unsafe { &*ptr };
            native_try!(ensure_zoned_date_time(vm, obj));
            let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid ZonedDateTime"));
            };
            NativeResult::Ok(JsValue::float(epoch_ns.div_euclid($divisor) as f64))
        }
    };
}

/// ZDT 日期派生 getter 宏：本地日期转 chrono NaiveDate 后按闭包取值。
macro_rules! zoned_date_time_naive_getter {
    ($name:ident, $body:expr) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let ptr = native_try!(receiver_obj(vm, args));
            let obj = unsafe { &*ptr };
            native_try!(ensure_zoned_date_time(vm, obj));
            let (year, month, day, _) = native_try!(zoned_date_time_plain_parts(vm, obj));
            let Some(date) = NaiveDate::from_ymd_opt(year, month, day) else {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid date"));
            };
            NativeResult::Ok($body(&date))
        }
    };
}

/// ZDT 常量 getter 宏：仅做 branding，返回固定值。
macro_rules! zoned_date_time_brand_only_getter {
    ($name:ident, $value:expr) => {
        pub fn $name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
            let ptr = native_try!(receiver_obj(vm, args));
            let obj = unsafe { &*ptr };
            native_try!(ensure_zoned_date_time(vm, obj));
            NativeResult::Ok($value)
        }
    };
}

// 墙钟字段 getter：本地日期分量直读。
zoned_date_time_parts_getter!(zoned_date_time_year, |y, _, _, _| y as f64);
zoned_date_time_parts_getter!(zoned_date_time_month, |_, m, _, _| m as f64);
zoned_date_time_parts_getter!(zoned_date_time_day, |_, _, d, _| d as f64);
zoned_date_time_parts_getter!(zoned_date_time_hour, |_, _, _, t| plain_time_components(t).0 as f64);
zoned_date_time_parts_getter!(zoned_date_time_minute, |_, _, _, t| plain_time_components(t).1 as f64);
zoned_date_time_parts_getter!(zoned_date_time_second, |_, _, _, t| plain_time_components(t).2 as f64);
zoned_date_time_parts_getter!(zoned_date_time_millisecond, |_, _, _, t| plain_time_components(t).3 as f64);
zoned_date_time_parts_getter!(zoned_date_time_microsecond, |_, _, _, t| plain_time_components(t).4 as f64);
zoned_date_time_parts_getter!(zoned_date_time_nanosecond, |_, _, _, t| plain_time_components(t).5 as f64);

// epoch 除法 getter：BigInt 除以秒/毫秒/微秒，负值向下取整。
zoned_date_time_epoch_getter!(zoned_date_time_epoch_seconds, 1_000_000_000);
zoned_date_time_epoch_getter!(zoned_date_time_epoch_milliseconds, 1_000_000);
zoned_date_time_epoch_getter!(zoned_date_time_epoch_microseconds, 1_000);

// 日期派生 getter：本地日期经 chrono NaiveDate 取周/年/月属性。
zoned_date_time_naive_getter!(zoned_date_time_day_of_week, |d: &NaiveDate| {
    JsValue::float((d.weekday().num_days_from_monday() + 1) as f64)
});
zoned_date_time_naive_getter!(zoned_date_time_day_of_year, |d: &NaiveDate| { JsValue::float(d.ordinal() as f64) });
zoned_date_time_naive_getter!(zoned_date_time_week_of_year, |d: &NaiveDate| {
    JsValue::float(d.iso_week().week() as f64)
});
zoned_date_time_naive_getter!(zoned_date_time_year_of_week, |d: &NaiveDate| {
    JsValue::float(d.iso_week().year() as f64)
});
zoned_date_time_naive_getter!(zoned_date_time_days_in_month, |d: &NaiveDate| {
    JsValue::float(days_in_month_iso(d.year(), d.month()) as f64)
});
zoned_date_time_naive_getter!(zoned_date_time_days_in_year, |d: &NaiveDate| {
    JsValue::float(if is_leap_year_iso(d.year()) { 366.0 } else { 365.0 })
});
zoned_date_time_naive_getter!(zoned_date_time_in_leap_year, |d: &NaiveDate| {
    JsValue::bool(is_leap_year_iso(d.year()))
});

// 常量 getter：ISO 日历下固定值，仅做 branding。
zoned_date_time_brand_only_getter!(zoned_date_time_days_in_week, JsValue::float(7.0));
zoned_date_time_brand_only_getter!(zoned_date_time_months_in_year, JsValue::float(12.0));
zoned_date_time_brand_only_getter!(zoned_date_time_era, JsValue::undefined());
zoned_date_time_brand_only_getter!(zoned_date_time_era_year, JsValue::undefined());

/// 读 ZDT 时区偏移（分钟）；槽 1 解析失败返回 RangeError。调用方须先 ensure_zoned_date_time。
fn zoned_date_time_offset_minutes<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<i32, JsValue> {
    let time_zone_id = to_string(obj.get_prop_at(1));
    instant_time_zone_offset(&time_zone_id).ok_or_else(|| crate::error::create_range_error(vm, "invalid time zone"))
}

/// `Temporal.ZonedDateTime.prototype.offset`：由偏移分钟数规范化为 ±HH:MM 字符串。
pub fn zoned_date_time_offset<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let offset_minutes = native_try!(zoned_date_time_offset_minutes(vm, obj));
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let magnitude = offset_minutes.abs();
    NativeResult::Ok(vm.new_string(&format!("{sign}{:02}:{:02}", magnitude / 60, magnitude % 60)))
}

/// `Temporal.ZonedDateTime.prototype.offsetNanoseconds`：偏移分钟数换算纳秒（f64 精确域内）。
pub fn zoned_date_time_offset_nanoseconds<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let offset_minutes = native_try!(zoned_date_time_offset_minutes(vm, obj));
    NativeResult::Ok(JsValue::float(offset_minutes as f64 * 60_000_000_000.0))
}

/// `Temporal.ZonedDateTime.prototype.monthCode`：ISO 日历下恒为 `M{month:02}` 补零格式。
pub fn zoned_date_time_month_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let (_, month, _, _) = native_try!(zoned_date_time_plain_parts(vm, obj));
    NativeResult::Ok(vm.new_string(&format!("M{month:02}")))
}

/// `Temporal.ZonedDateTime.prototype.hoursInDay`：当日与次日当地午夜差 / 小时，含 Instant 范围校验。
pub fn zoned_date_time_hours_in_day<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let (year, month, day, _) = native_try!(zoned_date_time_plain_parts(vm, obj));
    let offset_minutes = native_try!(zoned_date_time_offset_minutes(vm, obj));
    let today = native_try!(start_of_day_epoch_ns(year, month, day, offset_minutes)
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid date")));
    let days = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    let tomorrow = native_try!(start_of_day_epoch_ns_by_days(days + 1, offset_minutes)
        .ok_or_else(|| crate::error::create_range_error(vm, "invalid date")));
    NativeResult::Ok(JsValue::float((tomorrow - today) as f64 / 3_600_000_000_000.0))
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

/// 归一 options.relativeTo 为相对日期 `(y, m, d)`。
///
/// # 步骤
/// 1. string → parse_plain_date_time_string，失败回退 parse_plain_date_string（PDT 优先、PD 回退）；失败 RangeError。
/// 2. PlainDateTime / PlainDate 对象 → 直读 prop 0/1/2。
/// 3. ZonedDateTime → zoned_date_time_plain_parts 取本地 (y,m,d)。
/// 4. Instant → epoch 纳秒按 UTC 分解（div_euclid(DAY_NS) → civil_from_days）。
/// 5. 其他对象按 property bag 解析（ToTemporalDateTime，constrain）。
/// 6. undefined → None；其余原始值 → TypeError。
///
/// # 边界与前提
/// - 调用方须已把 raw 从 options 取出（读序由调用方保证）。
/// - 本函数只取日期分量；时间分量由调用方的 duration 分量另行加。
fn duration_relative_to_date<H: VmHost>(
    vm: &mut H, relative_raw: JsValue,
) -> Result<Option<(i128, i128, i128)>, JsValue> {
    if relative_raw.is_undefined() {
        return Ok(None);
    }
    if relative_raw.is_string() {
        let text = to_string(relative_raw);
        // 先按 PlainDateTime/PlainDate 解析（纯日期、日期时间、带 offset/注解的 plain 串）。
        if let Ok((year, month, day, _time_ns)) = parse_plain_date_time_string(&text) {
            return Ok(Some((i128::from(year), i128::from(month), i128::from(day))));
        }
        // ZonedDateTime-like（含 Z 或时区注解）：按 instant 解析 + 注解时区反推墙钟日期。
        let (epoch_ns, time_zone_id, _calendar) = zoned_date_time_string_parts(vm, &text, "reject")?;
        let offset_minutes = instant_time_zone_offset(&time_zone_id).unwrap_or(0);
        let wall_ns = epoch_ns + i128::from(offset_minutes) * 60_000_000_000;
        let (year, month, day) = civil_from_days(wall_ns.div_euclid(DAY_NS));
        return Ok(Some((year, month, day)));
    }
    if !relative_raw.is_object() {
        return Err(crate::error::create_type_error(vm, "invalid relativeTo"));
    }
    let ptr = relative_raw.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "invalid relativeTo"));
    }
    let obj = unsafe { &*ptr };
    if obj.is_plain_date_time_obj() || obj.is_plain_date_obj() {
        return Ok(Some((
            i128::from(get_double_prop(obj, 0) as i32),
            i128::from(get_double_prop(obj, 1) as u32),
            i128::from(get_double_prop(obj, 2) as u32),
        )));
    }
    if obj.is_zoned_date_time_obj() {
        let (year, month, day, _time_ns) = zoned_date_time_plain_parts(vm, obj)?;
        return Ok(Some((i128::from(year), i128::from(month), i128::from(day))));
    }
    if obj.is_instant_obj() {
        let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
            return Err(crate::error::create_range_error(vm, "invalid Instant"));
        };
        let (year, month, day) = civil_from_days(epoch_ns.div_euclid(DAY_NS));
        return Ok(Some((year, month, day)));
    }
    // property bag：按 ToRelativeTemporalObject 依次校验 calendar/timeZone/offset 再读字段。
    let calendar_raw = temporal_option_value(vm, obj, relative_raw, "calendar")?;
    if !calendar_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "invalid relativeTo"));
    }
    let time_zone_raw = temporal_option_value(vm, obj, relative_raw, "timeZone")?;
    if !time_zone_raw.is_undefined() {
        // 非字符串（对象/符号/null/数字）→ TypeError；字符串非合法时区 → RangeError。
        if !time_zone_raw.is_string() {
            return Err(crate::error::create_type_error(vm, "invalid time zone"));
        }
        if canonical_time_zone(&to_string(time_zone_raw)).is_none() {
            return Err(crate::error::create_range_error(vm, "invalid time zone"));
        }
    }
    let offset_raw = temporal_option_value(vm, obj, relative_raw, "offset")?;
    if !offset_raw.is_undefined() {
        // 非字符串 → TypeError；字符串格式非法（含亚秒偏移）→ RangeError。
        if !offset_raw.is_string() {
            return Err(crate::error::create_type_error(vm, "invalid offset"));
        }
        let offset_input = to_string(offset_raw);
        if parse_any_offset_minutes(&offset_input).is_none() || !valid_offset_fraction(&offset_input) {
            return Err(crate::error::create_range_error(vm, "invalid offset"));
        }
    }
    let (year, month, day, _time_ns, _calendar) = plain_date_time_object_parts(vm, relative_raw, obj, true, false)?;
    Ok(Some((i128::from(year), i128::from(month), i128::from(day))))
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

    // relativeTo：统一经 duration_relative_to_date 归一（支持 string/PD/PDT/ZDT/Instant/bag）。
    let relative_date = match duration_relative_to_date(vm, relative_raw) {
        Ok(relative) => relative,
        Err(error) => return NativeResult::Err(error),
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
    let values = native_try!(add_duration_values(vm, &receiver, &other));
    make_duration(vm, values)
}

/// 两个纯时间 duration（无 year/month/week）按分量精确求和并按最大非零单位平衡。
///
/// # 步骤
/// 1. 最大单位取两侧最大的非零时间单位（days=3 最大，ns=9 最小）。
/// 2. 分量在 i128 上按纳秒精确求和（f64 分量是精确整数，数学值不受 2^53 截断）。
/// 3. 从总纳秒向下按同号截断分解，进位到最大单位为止。
/// 4. 逐分量按纳秒刻度校验 2^53 秒上限。
///
/// # 边界与前提
/// - 任一侧含日历单位抛 RangeError（本批无 relativeTo 支持）。
/// - 全零与 nanosecond 分量和为 0 时直接返回零时长。
fn add_duration_values<H: VmHost>(vm: &mut H, receiver: &[f64; 10], other: &[f64; 10]) -> Result<[f64; 10], JsValue> {
    if receiver[..3].iter().any(|value| *value != 0.0) || other[..3].iter().any(|value| *value != 0.0) {
        return Err(crate::error::create_range_error(vm, "cannot add durations with calendar units"));
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
        return Ok([0.0; 10]);
    }
    // 分量按精确整数求和（f64 分量是精确整数；超过 2^53 的和需在 i128 上保持精确，规范按数学值计算）。
    let mut total_ns = 0_i128;
    const SUM_SCALES: [i128; 7] =
        [86_400_000_000_000, 3_600_000_000_000, 60_000_000_000, 1_000_000_000, 1_000_000, 1_000, 1];
    for (index, scale) in (3..10).zip(SUM_SCALES) {
        let Some(a) = duration_component_integer(receiver[index]) else {
            return Err(crate::error::create_range_error(vm, "invalid duration"));
        };
        let Some(b) = duration_component_integer(other[index]) else {
            return Err(crate::error::create_range_error(vm, "invalid duration"));
        };
        let Some(component) = a.checked_add(b).and_then(|value| value.checked_mul(scale)) else {
            return Err(crate::error::create_range_error(vm, "duration is out of range"));
        };
        let Some(updated) = total_ns.checked_add(component) else {
            return Err(crate::error::create_range_error(vm, "duration is out of range"));
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
            return Err(crate::error::create_range_error(vm, "duration time fields are out of range"));
        }
    }
    Ok(values)
}

/// `Temporal.Duration.prototype.subtract(other)`：等于 add(other.negated())。
///
/// # 边界与前提
/// - 继承 add 的无日历单位限制：任一侧含 year/month/week 抛 RangeError。
/// - 参数归一与 add 一致（字符串/对象/负值均支持）。
pub fn duration_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let other_val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other = native_try!(duration_like_values(vm, other_val));
    let receiver = duration_values(obj);
    let negated = std::array::from_fn(|index| if other[index] != 0.0 { -other[index] } else { 0.0 });
    let values = native_try!(add_duration_values(vm, &receiver, &negated));
    make_duration(vm, values)
}

/// `Temporal.Duration.prototype.equals(other)`：逐分量比较两个时长。
///
/// # 边界与前提
/// - receiver 须为 Duration（branding TypeError）。
/// - 参数经 duration_like_values 归一（无效字符串/混合符号抛 RangeError）。
/// - 全分量相等返回 true；仅数值相等比较，不涉及日历语义。
pub fn duration_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let other_val = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let other = native_try!(duration_like_values(vm, other_val));
    let receiver = duration_values(obj);
    let equal = (0..10).all(|index| receiver[index] == other[index]);
    NativeResult::Ok(JsValue::bool(equal))
}

/// `Temporal.Duration.prototype.sign`：首个非零分量的符号（-1/0/+1）。
pub fn duration_sign<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    for value in duration_values(obj) {
        if value != 0.0 {
            return NativeResult::Ok(JsValue::int(if value.is_sign_negative() { -1 } else { 1 }));
        }
    }
    NativeResult::Ok(JsValue::int(0))
}

/// `Temporal.Duration.prototype.blank`：所有分量是否全零。
pub fn duration_blank<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    NativeResult::Ok(JsValue::bool(duration_values(obj).iter().all(|value| *value == 0.0)))
}

/// `Temporal.Duration.prototype.toJSON()`：输出默认 ISO 字符串，忽略参数。
pub fn duration_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let receiver = args.first().copied().unwrap_or(0);
    duration_to_string(vm, &[receiver])
}

/// `Temporal.Duration.prototype.toLocaleString()`：默认 locale 下返回 ISO 字符串（无 Intl 依赖）。
pub fn duration_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let receiver = args.first().copied().unwrap_or(0);
    duration_to_string(vm, &[receiver])
}

/// 把 duration 应用相对点，返回目标 `(date, time_ns)`。
///
/// # 步骤
/// 1. days 及时间分量按 24h/day 汇总为纳秒，跨天部分拆成 delta_days 与一天内 target_time。
/// 2. 日历分量（year/month/week）与 delta_days 经 add_date_duration 加到相对点。
///
/// # 边界与前提
/// - 时间分量超出 i128 纳秒表示范围时抛 RangeError。
/// - 返回的 time_ns 带符号（负 duration 时可为负）。
fn duration_apply_to_relative<H: VmHost>(
    vm: &mut H, rel: (i128, i128, i128), values: &[f64; 10],
) -> Result<((i128, i128, i128), i128), JsValue> {
    let Some(time_ns_total) = duration_time_nanoseconds(values) else {
        return Err(crate::error::create_range_error(vm, "duration is out of range"));
    };
    let delta_days = time_ns_total / DAY_NS;
    let target_time = time_ns_total % DAY_NS;
    let mut date_parts = [0.0; 10];
    date_parts[0] = values[0];
    date_parts[1] = values[1];
    date_parts[2] = values[2];
    date_parts[3] = delta_days as f64;
    let end = add_date_duration(rel, &date_parts);
    // 目标日期须落在 ISO 日期范围内（约 ±10^8 天），否则抛 RangeError。
    if days_from_civil(end.0, end.1, end.2).abs() > MAX_ISO_DAY {
        return Err(crate::error::create_range_error(vm, "relativeTo plus duration is out of range"));
    }
    Ok((end, target_time))
}

/// `Temporal.Duration.compare(one, two, options)`：按相对点比较两个时长。
///
/// # 步骤
/// 1. one/two 各自 duration_like_values 归一（读序 one → two → options）。
/// 2. options.relativeTo 经 duration_relative_to_date 归一（含 ZDT/Instant/bag 分支）。
/// 3. 无 relativeTo 且无日历单位：按 duration_time_nanoseconds 直接比大小（fast path）。
/// 4. 无 relativeTo 但含日历单位：RangeError（calendar-possibly-required 语义）。
/// 5. 有 relativeTo：每个 duration 应用相对点得 (date, time)，先比日期再比时间（字典序）。
///
/// # 边界与前提
/// - 含日历单位必须提供 relativeTo，否则 RangeError。
/// - 时间分量按 24h/day 折算；比较的是应用后目标时刻，非分量数值。
pub fn duration_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let one_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let two_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let one = native_try!(duration_like_values(vm, one_val));
    let two = native_try!(duration_like_values(vm, two_val));
    // 全分量相等直接返回 0（含日历单位时也无需 relativeTo，对齐 instances-identical）。
    if one == two {
        return NativeResult::Ok(JsValue::int(0));
    }
    let options_value = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };
    let relative_raw = if options_value.is_undefined() {
        JsValue::undefined()
    } else {
        if !options_value.is_object() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options_ptr = options_value.as_js_object_ptr();
        if options_ptr.is_null() {
            return NativeResult::Err(crate::error::create_type_error(vm, "options must be an object"));
        }
        let options = unsafe { &*options_ptr };
        match temporal_option_value(vm, options, options_value, "relativeTo") {
            Ok(raw) => raw,
            Err(error) => return NativeResult::Err(error),
        }
    };
    let relative = native_try!(duration_relative_to_date(vm, relative_raw));

    let ordering = match relative {
        None => {
            // 无 relativeTo：含日历单位抛 RangeError，否则按归一纳秒比大小。
            if one[..3].iter().any(|value| *value != 0.0) || two[..3].iter().any(|value| *value != 0.0) {
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "relativeTo is required for calendar units",
                ));
            }
            let one_ns = native_try!(duration_time_nanoseconds(&one)
                .ok_or_else(|| { crate::error::create_range_error(vm, "duration is out of range") }));
            let two_ns = native_try!(duration_time_nanoseconds(&two)
                .ok_or_else(|| { crate::error::create_range_error(vm, "duration is out of range") }));
            one_ns.cmp(&two_ns)
        }
        Some(rel) => {
            // 有 relativeTo：各 duration 应用后先比日期再比时间。
            let (date1, time1) = native_try!(duration_apply_to_relative(vm, rel, &one));
            let (date2, time2) = native_try!(duration_apply_to_relative(vm, rel, &two));
            let date_cmp = compare_iso_date(date1, date2);
            if date_cmp != 0 {
                if date_cmp > 0 {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                }
            } else {
                time1.cmp(&time2)
            }
        }
    };
    NativeResult::Ok(JsValue::int(match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }))
}
/// `Temporal.Duration.prototype.round(roundTo)`：按最小单位舍入并按最大单位平衡。
///
/// # 步骤
/// 1. 解析 roundTo（字符串简写或对象）→ largestUnit/smallestUnit/roundingIncrement/roundingMode/relativeTo。
/// 2. relativeTo 经 duration_relative_to_date 归一；仅日历单位路径需要它。
/// 3. 纯时间路径（无 year/month/week 且 largest/smallest ≥ day）按 24h/day 汇总舍入。
/// 4. 日历单位路径把 duration 应用 relativeTo 后走 nudge_iso_difference 平衡舍入。
///
/// # 边界与前提
/// - 含日历单位（year/month/week 非零）或目标单位为日历单位时 relativeTo 缺失抛 RangeError。
/// - 时间分量按 24 小时/天折算，与 polyfill 的 24h-day 语义一致。
pub fn duration_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_duration(vm, obj));
    let round_to = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    if round_to.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "options parameter is required"));
    }

    let values = duration_values(obj);
    // 现有最大单位：首个非零分量；全零时视为 nanosecond。
    let mut existing_largest = 9_usize;
    for (index, value) in values.iter().enumerate() {
        if *value != 0.0 {
            existing_largest = index;
            break;
        }
    }

    // roundTo 字符串 => { smallestUnit: 字符串 }；否则必须为对象。
    let (largest_raw, relative_raw, increment_raw, mode_raw, smallest_raw) = if round_to.is_string() {
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

    // largestUnit：允许 "auto"；缺失/undefined 视为未提供。
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

    // roundingIncrement：ToIntegerOrInfinity 舍入后需在 [1, 10^9] 内。
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

    // roundingMode：缺失时 halfExpand。
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

    // smallestUnit：缺失时 nanosecond。
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

    // 默认最大单位：现有最大单位与最小单位中较大（索引较小）的那个。
    let default_largest = if existing_largest < smallest_index {
        existing_largest
    } else {
        smallest_index
    };
    let largest = largest_index.unwrap_or(default_largest);

    // 至少一个单位需要显式提供；largest 不能小于 smallest。
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

    // 舍入增量上限：单位越小可除以上一级；日历单位（year/month/week/day）无整除约束。
    const MAX_INCREMENT: [i128; 10] = [0, 0, 0, 0, 24, 60, 60, 1000, 1000, 1000];
    let max_increment = MAX_INCREMENT[smallest_index];
    if max_increment != 0 && (increment >= max_increment || max_increment % increment != 0) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    }
    // 日历单位的 increment 要求 largestUnit 与 smallestUnit 相同（防止窗口舍入歧义）。
    if increment > 1 && smallest_index == 3 && largest != smallest_index {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "rounding increments of calendar units require largestUnit to equal smallestUnit",
        ));
    }

    // relativeTo 仅在日历路径需要时读取（解析失败直接抛错）。
    let relative_date = match duration_relative_to_date(vm, relative_raw) {
        Ok(relative) => relative,
        Err(error) => return NativeResult::Err(error),
    };

    // 日历单位存在或目标为日历单位时走 relativeTo 路径，否则走纯时间路径。
    let needs_relative = values[..3].iter().any(|value| *value != 0.0) || largest < 3 || smallest_index < 3;
    if !needs_relative {
        // 纯时间路径：按 24 小时/天将 days..nanoseconds 汇总为纳秒，按 smallest 舍入后平衡到 largest。
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
            // 与 duration_add 相同的同号模分解：从 ns 向 largest 进位。
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
        // 范围校验：对每个时间分量按纳秒刻度检查是否达到 2^53 秒上限。
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
                return NativeResult::Err(crate::error::create_range_error(
                    vm,
                    "duration time fields are out of range",
                ));
            }
        }
        return make_duration(vm, result);
    }

    // 日历路径：需要 relativeTo，把 duration 应用后按 nudge_iso_difference 舍入平衡。
    let Some(rel) = relative_date else {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "a starting point is required for rounding calendar units",
        ));
    };
    let Some(time_ns_total) = duration_time_nanoseconds(&values) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "duration is out of range"));
    };
    let delta_days = time_ns_total / DAY_NS;
    let target_time = time_ns_total % DAY_NS;
    let mut date_parts = [0.0; 10];
    date_parts[0] = values[0];
    date_parts[1] = values[1];
    date_parts[2] = values[2];
    date_parts[3] = delta_days as f64;
    let end = add_date_duration(rel, &date_parts);
    let settings = DifferenceSettings {
        largest_index: largest,
        smallest_index,
        increment,
        mode,
    };
    let result = match nudge_iso_difference(vm, rel, 0, end, target_time, settings, false) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
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

/// 按是否含秒与小数位格式化当日纳秒为 `HH:MM:SS[.frac]`。
///
/// # 边界与前提
/// - `include_seconds=false` 时忽略小数位，仅输出 `HH:MM`。
/// - `Some(0)` 不输出小数；`Some(digits)` 固定补足位数；`None` 按实际非零亚秒去尾零。
fn format_plain_time_iso(time_ns: i128, include_seconds: bool, fractional_digits: Option<usize>) -> String {
    let hour = time_ns / 3_600_000_000_000;
    let minute = time_ns / 60_000_000_000 % 60;
    let second = time_ns / 1_000_000_000 % 60;
    let subsecond = time_ns % 1_000_000_000;
    let mut output = format!("{hour:02}:{minute:02}");
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
    output
}

/// `Temporal.PlainTime.prototype.toString(options)`：按精度、舍入模式与最小单位输出 ISO 8601 时间。
pub fn plain_time_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_time(vm, obj));
    let total_ns = get_double_prop(obj, 0) as i128;
    let options_value = if args.len() < 2 { JsValue::undefined() } else { vm.reg(args[1]) };
    let (fractional_input, mode_value, smallest_value) = if options_value.is_undefined() {
        (FractionalSecondDigitsInput::Auto, "trunc".to_string(), None)
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
        (fractional, mode, smallest)
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
    // 舍入后可能跨到次日（rounding-cross-midnight），取模保持 0-24 域。
    const DAY_NS: i128 = 86_400_000_000_000;
    let Some(rounded_ns) = round_instant_ns(total_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time"));
    };
    let time_ns = rounded_ns.rem_euclid(DAY_NS);
    let output = format_plain_time_iso(time_ns, include_seconds, output_digits);
    NativeResult::Ok(vm.new_string_owned(output))
}

/// `Temporal.PlainTime.prototype.toJSON()`：输出默认 ISO 时间字符串，忽略参数。
pub fn plain_time_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_default_string(vm, args)
}

/// `Temporal.PlainTime.prototype.toLocaleString()`：当前使用稳定的默认 ISO 时间表示。
pub fn plain_time_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_default_string(vm, args)
}

/// 输出无 options 的默认 PlainTime 串（全秒 + 非零亚秒）。
fn plain_time_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_time(vm, obj));
    let total_ns = get_double_prop(obj, 0) as i128;
    NativeResult::Ok(vm.new_string_owned(format_plain_time_iso(total_ns, true, None)))
}

/// `Temporal.PlainTime.prototype.valueOf()`：Temporal 对象禁止转原始值。
pub fn plain_time_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.PlainTime has no valueOf"))
}

/// `Temporal.PlainTime.from(item, options)`：从字符串、PlainTime/ZDT/PlainDateTime 实例或 property bag 构造。
pub fn plain_time_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if value.is_string() {
        // 规范顺序：先 ParseTemporalTimeString，再 ToTemporalOverflow(options)。
        let total_ns = match parse_plain_time_string(&to_string(value)) {
            Some(ns) => ns,
            None => {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO 8601 time"));
            }
        };
        native_try!(temporal_overflow(vm, args));
        make_plain_time(vm, total_ns)
    } else {
        // 对象分支：实例复制 / property bag 统一走 plain_time_like_ns（含缺省 0 与约束钳制）。
        native_try!(temporal_overflow(vm, args));
        let total_ns = native_try!(plain_time_like_ns(vm, value));
        make_plain_time(vm, total_ns)
    }
}

/// `Temporal.PlainTime.prototype.equals(other)`：各分量全等返回 true，否则 false。
///
/// # 边界与前提
/// - other 经 `plain_time_like_ns` 归一（实例/字符串/property bag），无法转换抛 TypeError/RangeError。
pub fn plain_time_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(error) => return NativeResult::Err(error),
    };
    let obj = unsafe { &*ptr };
    if let Err(error) = ensure_plain_time(vm, obj) {
        return NativeResult::Err(error);
    }
    let receiver_ns = get_double_prop(obj, 0);
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let other_ns = match plain_time_like_ns(vm, other_val) {
        Ok(ns) => ns,
        Err(error) => return NativeResult::Err(error),
    };
    NativeResult::Ok(JsValue::bool(receiver_ns as i128 == other_ns as i128))
}

/// `Temporal.PlainTime.compare(one, two)`：静态比较，按午夜后纳秒返回 -1、0 或 1。
pub fn plain_time_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let one = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let two = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let one_ns = match plain_time_like_ns(vm, one) {
        Ok(ns) => ns,
        Err(error) => return NativeResult::Err(error),
    };
    let two_ns = match plain_time_like_ns(vm, two) {
        Ok(ns) => ns,
        Err(error) => return NativeResult::Err(error),
    };
    let result = match (one_ns as i128).cmp(&(two_ns as i128)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    NativeResult::Ok(JsValue::int(result))
}

// ───────────────────── PlainTime until / since ─────────────────────

/// until/since 核心：receiver 与 other 归一为午夜后纳秒，经 `difference_core` 计算差值。
///
/// # 步骤
/// 1. branding receiver 取槽 0 ns，other 经 `plain_time_like_ns` 归一。
/// 2. `parse_difference_settings` 解析 options（default_largest=4=hour）。
/// 3. 日期固定为 epoch（0,0,0），调 `difference_core` 输出 Duration。
///
/// # 边界与前提
/// - since 复用 same 语义并整体取反（difference_core 内部处理），不调换两端。
fn plain_time_difference<H: VmHost>(vm: &mut H, args: &[u8], since: bool) -> NativeResult {
    let ptr = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(error) => return NativeResult::Err(error),
    };
    let obj = unsafe { &*ptr };
    if let Err(error) = ensure_plain_time(vm, obj) {
        return NativeResult::Err(error);
    }
    let receiver_ns = get_double_prop(obj, 0);
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let other_ns = match plain_time_like_ns(vm, other_val) {
        Ok(ns) => ns,
        Err(error) => return NativeResult::Err(error),
    };
    let options_value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let settings = match parse_difference_settings(vm, options_value, false, 4) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    difference_core(vm, (0, 0, 0), receiver_ns as i128, (0, 0, 0), other_ns as i128, settings, since)
}

/// `Temporal.PlainTime.prototype.until(other, options)`。
pub fn plain_time_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_difference(vm, args, false)
}

/// `Temporal.PlainTime.prototype.since(other, options)`。
pub fn plain_time_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_difference(vm, args, true)
}

// ───────────────────── PlainTime add / subtract ─────────────────────

/// add/subtract 核心：receiver 午夜后纳秒叠加时长的"时间域"增量。
///
/// # 步骤
/// 1. branding receiver 取槽 0 ns。
/// 2. `duration_like_values` 归一 duration，`temporal_overflow` 解析 options（读序：duration → options）。
/// 3. 仅取 hours 起的时间字段换算纳秒增量（日期字段 days 及以上规范忽略）。
/// 4. 按 sign 叠加后 `rem_euclid(DAY_NS)` 保持 0-24 域 → `make_plain_time`。
///
/// # 边界与前提
/// - duration 全 0 → 值不变的新对象（blank-duration 语义）。
/// - duration 含日期字段（y/m/w/d）不报错，规范对 PlainTime 直接忽略。
fn plain_time_apply_duration<H: VmHost>(vm: &mut H, args: &[u8], sign: i64) -> NativeResult {
    let ptr = match receiver_obj(vm, args) {
        Ok(p) => p,
        Err(error) => return NativeResult::Err(error),
    };
    let obj = unsafe { &*ptr };
    if let Err(error) = ensure_plain_time(vm, obj) {
        return NativeResult::Err(error);
    }
    let time_ns = get_double_prop(obj, 0) as i128;
    let duration_like = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = match duration_like_values(vm, duration_like) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
    if let Err(error) = temporal_overflow(vm, args) {
        return NativeResult::Err(error);
    }
    const DAY_NS: i128 = 86_400_000_000_000;
    // 时间字段（hours 起）单独换算纳秒；日期字段 days 及以上对 PlainTime 无意义直接忽略。
    let [_, _, _, _, h, min, s, ms, us, ns] = values;
    let time_delta = duration_component_integer(h).unwrap_or(0) * 3_600_000_000_000
        + duration_component_integer(min).unwrap_or(0) * 60_000_000_000
        + duration_component_integer(s).unwrap_or(0) * 1_000_000_000
        + duration_component_integer(ms).unwrap_or(0) * 1_000_000
        + duration_component_integer(us).unwrap_or(0) * 1_000
        + duration_component_integer(ns).unwrap_or(0);
    // 时间溢出跨午夜：rem_euclid 保持 0-24 域。
    let new_time_ns = (time_ns + time_delta * sign as i128).rem_euclid(DAY_NS);
    make_plain_time(vm, new_time_ns as f64)
}

/// `Temporal.PlainTime.prototype.add(durationLike, options)`。
pub fn plain_time_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_apply_duration(vm, args, 1)
}

/// `Temporal.PlainTime.prototype.subtract(durationLike, options)`。
pub fn plain_time_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_time_apply_duration(vm, args, -1)
}

// ───────────────────── PlainTime round ─────────────────────

/// PlainTime round 的最小单位表：小时..纳秒（不含 day）。
/// 返回 (单位纳秒, 最大增量)。最大增量 = MaximumTemporalDurationRoundingIncrement：
/// hour→24、minute→60、second→60、millisecond→1000、microsecond→1000、nanosecond→1000。
fn plain_time_round_unit(value: &str) -> Option<(i128, i128)> {
    match value {
        "hour" | "hours" => Some((3_600_000_000_000, 24)),
        "minute" | "minutes" => Some((60_000_000_000, 60)),
        "second" | "seconds" => Some((1_000_000_000, 60)),
        "millisecond" | "milliseconds" => Some((1_000_000, 1_000)),
        "microsecond" | "microseconds" => Some((1_000, 1_000)),
        "nanosecond" | "nanoseconds" => Some((1, 1_000)),
        _ => None,
    }
}

/// `Temporal.PlainTime.prototype.round(roundTo)`：按最小单位、增量和模式在"当日"域内舍入。
///
/// # 步骤
/// 1. roundTo 解析：undefined → TypeError；字符串 → {smallestUnit: 串}；对象 → 依次 Get
///    roundingIncrement → roundingMode → smallestUnit（读序对齐 order-of-operations）。
/// 2. branding receiver 取槽 0 ns。
/// 3. 单位表查 smallestUnit（hour..nanosecond；None → RangeError "invalid smallest unit"）。
/// 4. roundingIncrement 校验：1..=1e9 且 **真因子**（increment < max_increment 且
///    max_increment % increment == 0，否则 RangeError）。
/// 5. mode = instant_rounding_mode（None → RangeError）；缺省 "halfExpand"。
/// 6. round_instant_ns(time_ns, unit_ns*increment, mode)，结果 rem_euclid(DAY_NS) 保持 0-24 域。
///
/// # 边界与前提
/// - 舍入跨午夜（如 23:59:59.9 舍入到秒）→ 取模回 00:00，符合 rounding-cross-midnight 语义。
pub fn plain_time_round<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let round_to = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
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

    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_time(vm, obj));
    let time_ns = get_double_prop(obj, 0) as i128;

    let Some(mode) = instant_rounding_mode(&mode_value) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding mode"));
    };
    let Some(unit_value) = unit_value else {
        return NativeResult::Err(crate::error::create_range_error(vm, "smallestUnit is required"));
    };
    let Some((unit_ns, max_increment)) = plain_time_round_unit(&unit_value) else {
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
    // 真因子校验：增量须严格小于最大增量且能整除（MaximumTemporalDurationRoundingIncrement
    // 语义；instant_round 只查 % 的缺陷不在此沿用）。
    if increment >= max_increment || max_increment % increment != 0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "rounding increment must divide a day"));
    }
    let Some(quantum_ns) = unit_ns.checked_mul(increment) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid rounding increment"));
    };
    let Some(rounded_ns) = round_instant_ns(time_ns, quantum_ns, mode) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid time"));
    };
    const DAY_NS: i128 = 86_400_000_000_000;
    let time_ns = rounded_ns.rem_euclid(DAY_NS);
    make_plain_time(vm, time_ns as f64)
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
    // 纯时间字符串（TemporalTimeString），例如 "15:23" / "T0030" / "152330.1-08"。
    if parse_plain_time_string(trimmed).is_some() {
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
            if obj.is_plain_month_day_obj() || obj.is_plain_year_month_obj() {
                return Ok(Some(get_calendar_id(obj, 3)));
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

/// 当地午夜纪元纳秒（含 Instant 范围校验，spec GetStartOfDay 语义）；越界返回 None。
fn start_of_day_epoch_ns(year: i32, month: u32, day: u32, offset_minutes: i32) -> Option<i128> {
    start_of_day_epoch_ns_by_days(days_from_civil(i128::from(year), i128::from(month), i128::from(day)), offset_minutes)
}

/// 按日数直接算当地午夜（hoursInDay 的明日边界复用：days+1 不经 civil 回填）。
fn start_of_day_epoch_ns_by_days(days: i128, offset_minutes: i32) -> Option<i128> {
    let start = days
        .checked_mul(86_400_000_000_000)?
        .checked_sub(i128::from(offset_minutes) * 60_000_000_000)?;
    (start.unsigned_abs() <= MAX_INSTANT_NS as u128).then_some(start)
}

/// 本地墙钟分量 + 时区偏移（分钟）→ 纪元纳秒（zoned_date_time_plain_parts 的逆）。
///
/// # 边界与前提
/// - (year, month, day, time_ns) 须已通过 valid_iso_date / valid_plain_time 校验（调用方保证）。
/// - 仅做 checked 溢出防护，Instant 范围校验由调用方按需执行。
fn local_to_epoch_ns(year: i32, month: u32, day: u32, time_ns: f64, offset_minutes: i32) -> Option<i128> {
    let days = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    days.checked_mul(86_400_000_000_000)?
        .checked_add(time_ns as i128)?
        .checked_sub(i128::from(offset_minutes) * 60_000_000_000)
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
/// default_largest 为 largestUnit 缺省/auto 时相对 smallestUnit 的取小上限
/// （PDT/PD 传 3=day，ZDT 传 4=hour）。
fn parse_difference_settings<H: VmHost>(
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

/// 差值核心：internal = end - start，按设置取整；since 用 NegateRoundingMode
/// 的舍入模式并在最后整体取反（不调换两端，调换会改变 0.5 边界所在的年长）。
#[allow(clippy::too_many_arguments)]
fn difference_core<H: VmHost>(
    vm: &mut H, start: (i128, i128, i128), start_time_ns: i128, end: (i128, i128, i128), end_time_ns: i128,
    settings: DifferenceSettings, since: bool,
) -> NativeResult {
    let values = match nudge_iso_difference(vm, start, start_time_ns, end, end_time_ns, settings, since) {
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
fn nudge_iso_difference<H: VmHost>(
    vm: &mut H, start: (i128, i128, i128), start_time_ns: i128, end: (i128, i128, i128), end_time_ns: i128,
    settings: DifferenceSettings, since: bool,
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

    let mut values = if smallest_index == 9 && increment == 1 {
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
    let settings = match parse_difference_settings(vm, options_value, false, 3) {
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

/// ZDT add/subtract 核心：本地分量叠加 duration（复用 plain_date_time_apply_duration 算法），
/// 先做中间日期检查（AddZonedDateTime 的中间日期越界校验），再经 local_to_epoch_ns 转回。
///
/// # 步骤
/// 1. branding + receiver 本地分量 + 时区偏移。
/// 2. duration_like_values 归一 + temporal_overflow 解析（读序：duration → options）。
/// 3. 时间增量（hours 起）与日期增量（y/m/w/d）分别按 sign 取反；时间部分
///    div_euclid/rem_euclid 拆出 extra_days/new_time_ns。
/// 4. 日期部分月份/日钳制叠加后先验中间日期（+ 原 time_ns 须在 PlainDateTime 范围）→ RangeError。
/// 5. 时间进位合并 → 最终 (yy, mm, dd, new_time_ns) → valid_plain_date_time_range 校验。
/// 6. local_to_epoch_ns + MAX_INSTANT_NS 校验 → make_zoned_date_time 保时区/日历槽。
///
/// # 边界与前提
/// - duration 全 0 → 值不变的新对象（blank-duration 语义）。
/// - 固定偏移下"中间 epoch + 时间增量"与"合并后 local_to_epoch_ns"严格相等，无需分步换算。
fn zoned_date_time_apply_duration<H: VmHost>(vm: &mut H, args: &[u8], sign: i64) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let time_zone_id = to_string(obj.get_prop_at(1));
    let offset_minutes = native_try!(zoned_date_time_offset_minutes(vm, obj));
    let (year, month, day, time_ns) = match zoned_date_time_plain_parts(vm, obj) {
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
    // 中间检查：CalendarDateAdd 后的日期 + 原 time_ns 须在 PlainDateTime 范围
    // （±MAX instant 的 {days:∓1} 在此拦截）。
    let added_days = days_from_civil(ny, nm, new_day) + day_delta * sign as i128;
    let (iy, im, id) = civil_from_days(added_days);
    if !valid_plain_date_time_range(iy as i32, im as u32, id as u32, time_ns) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time"));
    }
    let total_days = added_days + extra_days;
    let (yy, mm, dd) = civil_from_days(total_days);
    if !valid_plain_date_time_range(yy as i32, mm as u32, dd as u32, new_time_ns as f64) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date-time"));
    }
    let epoch_ns = match local_to_epoch_ns(yy as i32, mm as u32, dd as u32, new_time_ns as f64, offset_minutes) {
        Some(epoch_ns) if epoch_ns.unsigned_abs() <= MAX_INSTANT_NS as u128 => epoch_ns,
        _ => return NativeResult::Err(crate::error::create_range_error(vm, "ZonedDateTime outside supported range")),
    };
    let calendar_id = get_calendar_id(obj, 2);
    make_zoned_date_time(vm, epoch_ns, &time_zone_id, &calendar_id)
}

/// `Temporal.ZonedDateTime.prototype.add(durationLike, options)`。
pub fn zoned_date_time_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    zoned_date_time_apply_duration(vm, args, 1)
}

/// `Temporal.ZonedDateTime.prototype.subtract(durationLike, options)`。
pub fn zoned_date_time_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    zoned_date_time_apply_duration(vm, args, -1)
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
    let settings = match parse_difference_settings(vm, options_value, true, 3) {
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
    let settings = match parse_difference_settings(vm, options_value, true, 3) {
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

// Temporal.PlainMonthDay / Temporal.PlainYearMonth 对象地基：
// 数字分量转换、槽对象构造、构造器与 getter/toString/toJSON。

/// Temporal 数字分量转换（number-only 路径）：先 ToPrimitive(Number)——对象经
/// valueOf/toString 取值；Symbol/BigInt → TypeError；NaN/±Inf → RangeError
/// （undefined 与不可解析串都归 NaN，由此统一抛 RangeError）；其余截断取整。
fn temporal_number_component<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    let primitive = oxide_runtime_api::to_primitive(value, oxide_runtime_api::ToPrimitiveHint::Number, vm)
        .map_err(|error| native_engine_error(vm, &error))?;
    if primitive.is_symbol() || primitive.is_bigint() {
        return Err(crate::error::create_type_error(vm, "invalid number"));
    }
    let number = to_number(primitive);
    if number.is_infinite() || number.is_nan() {
        return Err(crate::error::create_range_error(vm, "invalid number"));
    }
    Ok(number.trunc())
}

/// ISO 年-月是否落在 PlainYearMonth 可表示范围（ISOYearMonthWithinLimits）：
/// 仅 -271821 年 3 月及以前、+275760 年 10 月及以后越界。
fn iso_year_month_within_limits(year: i32, month: u32) -> bool {
    !((year == -271821 && month < 4) || (year == 275760 && month > 9))
}

/// 构造 PlainMonthDay 实例对象（槽 0-3 = 月/日/参考年/日历 ID）。
/// from/toPlainDate 等返回新对象的成员使用；构造器走 receiver 初始化路径。
fn make_plain_month_day<H: VmHost>(vm: &mut H, month: u32, day: u32, ref_year: i32, calendar: &str) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_month_day_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_MONTH_DAY;
    obj.set_prop_at(0, JsValue::float(month as f64));
    obj.set_prop_at(1, JsValue::float(day as f64));
    obj.set_prop_at(2, JsValue::float(ref_year as f64));
    obj.set_prop_at(3, vm.new_string(calendar));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

/// 构造 PlainYearMonth 实例对象（槽 0-3 = 年/月/参考日/日历 ID）。
/// from/toPlainDate 等返回新对象的成员使用；构造器走 receiver 初始化路径。
#[expect(dead_code)]
fn make_plain_year_month<H: VmHost>(vm: &mut H, year: i32, month: u32, ref_day: u32, calendar: &str) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_year_month_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_YEAR_MONTH;
    obj.set_prop_at(0, JsValue::float(year as f64));
    obj.set_prop_at(1, JsValue::float(month as f64));
    obj.set_prop_at(2, JsValue::float(ref_day as f64));
    obj.set_prop_at(3, vm.new_string(calendar));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

fn ensure_plain_month_day<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_month_day_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

fn ensure_plain_year_month<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_year_month_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

/// 读 PlainMonthDay 的 月/日/参考年 三元组（槽 0-2），含 receiver 校验。
fn plain_month_day_mdy<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(f64, f64, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_month_day(vm, obj)?;
    Ok((get_double_prop(obj, 0), get_double_prop(obj, 1), get_double_prop(obj, 2)))
}

/// 读 PlainYearMonth 的 年/月/参考日 三元组（槽 0-2），含 receiver 校验。
fn plain_year_month_ymd<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(f64, f64, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_year_month(vm, obj)?;
    Ok((get_double_prop(obj, 0), get_double_prop(obj, 1), get_double_prop(obj, 2)))
}

/// 构造器日历参数解析（ToTemporalCalendarSlotValue，缺省 "iso8601"）：
/// undefined → 缺省；字符串 → 严格白名单（非法 RangeError）；Temporal 日期实例 →
/// 日历槽直读（不触发属性 getter）；函数对象 → 缺省（无日历行为）；
/// 其他对象与原始值 → TypeError。
fn temporal_constructor_calendar_id<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
    if value.is_undefined() {
        return Ok("iso8601".to_string());
    }
    if value.is_string() {
        return temporal_calendar_id_strict(vm, value)
            .map(|calendar| calendar.unwrap_or_else(|| "iso8601".to_string()));
    }
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            if obj.is_function() {
                return Ok("iso8601".to_string());
            }
            let slot = if obj.is_plain_date_obj() {
                3
            } else if obj.is_plain_date_time_obj() {
                4
            } else if obj.is_zoned_date_time_obj() {
                2
            } else if obj.is_plain_month_day_obj() || obj.is_plain_year_month_obj() {
                3
            } else {
                return Err(crate::error::create_type_error(vm, "invalid calendar"));
            };
            return Ok(get_calendar_id(obj, slot));
        }
    }
    Err(crate::error::create_type_error(vm, "invalid calendar"))
}

/// `Temporal.PlainMonthDay` 构造器：`new PlainMonthDay(month, day[, calendar[, referenceISOYear]])`。
pub fn plain_month_day_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().plain_month_day_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.PlainMonthDay cannot be invoked without 'new'",
        ));
    }
    // 转换序：month → day → calendar → referenceISOYear（后两者有缺省值）。
    let month = native_try!(temporal_number_component(
        vm,
        if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() },
    ));
    let day = native_try!(temporal_number_component(
        vm,
        if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() },
    ));
    let calendar_raw = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };
    let calendar = native_try!(temporal_constructor_calendar_id(vm, calendar_raw));
    let ref_year = if args.len() > 4 && !vm.reg(args[4]).is_undefined() {
        native_try!(temporal_number_component(vm, vm.reg(args[4]))) as i32
    } else {
        1972
    };
    if !valid_iso_date(ref_year, month as u32, day as u32) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    // 表示范围（ISODateWithinLimits）：-271821-04-19 … +275760-09-13。
    let day_count = days_from_civil(i128::from(ref_year), i128::from(month as i32), i128::from(day as i32));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    let calendar_value = vm.new_string(&calendar);
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_PLAIN_MONTH_DAY,
        [
            JsValue::float(month),
            JsValue::float(day),
            JsValue::float(ref_year as f64),
            calendar_value,
        ],
    )
}

/// `Temporal.PlainYearMonth` 构造器：`new PlainYearMonth(year, month[, calendar[, referenceISODay]])`。
pub fn plain_year_month_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ctor_proto = vm.session().builtin_world().plain_year_month_proto.as_ptr();
    if !is_ctor_call(vm, args, ctor_proto) {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Class constructor Temporal.PlainYearMonth cannot be invoked without 'new'",
        ));
    }
    // 转换序：year → month → calendar → referenceISODay（后两者有缺省值）。
    let year = native_try!(temporal_number_component(
        vm,
        if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() },
    )) as i32;
    let month = native_try!(temporal_number_component(
        vm,
        if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() },
    ));
    let calendar_raw = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };
    let calendar = native_try!(temporal_constructor_calendar_id(vm, calendar_raw));
    let ref_day = if args.len() > 4 && !vm.reg(args[4]).is_undefined() {
        native_try!(temporal_number_component(vm, vm.reg(args[4])))
    } else {
        1.0
    };
    let month_i = month as i32;
    if !(1..=12).contains(&month_i) {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    let ref_day_i = ref_day as i32;
    if ref_day_i < 1 || ref_day_i > days_in_month_iso(year, month_i as u32) as i32 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ISO date"));
    }
    if !iso_year_month_within_limits(year, month_i as u32) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO date is out of range"));
    }
    let calendar_value = vm.new_string(&calendar);
    initialize_temporal_receiver(
        vm,
        args,
        JsObject::OBJ_TYPE_PLAIN_YEAR_MONTH,
        [
            JsValue::float(year as f64),
            JsValue::float(month),
            JsValue::float(ref_day),
            calendar_value,
        ],
    )
}

/// `Temporal.PlainMonthDay.prototype.day` getter（槽 1）。
pub fn plain_month_day_day<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, day, _) = native_try!(plain_month_day_mdy(vm, args));
    NativeResult::Ok(JsValue::float(day))
}

/// `Temporal.PlainMonthDay.prototype.monthCode` getter：`M01`..`M12`。
pub fn plain_month_day_month_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (month, _, _) = native_try!(plain_month_day_mdy(vm, args));
    NativeResult::Ok(vm.new_string(&format!("M{:02}", month as u32)))
}

/// `Temporal.PlainMonthDay.prototype.calendarId` getter：读日历槽（槽 3）。
pub fn plain_month_day_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    NativeResult::Ok(vm.new_string(&get_calendar_id(obj, 3)))
}

/// `Temporal.PlainYearMonth.prototype.year` getter（槽 0）。
pub fn plain_year_month_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, _, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(year))
}

/// `Temporal.PlainYearMonth.prototype.month` getter（槽 1）。
pub fn plain_year_month_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, month, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(month))
}

/// `Temporal.PlainYearMonth.prototype.monthCode` getter：`M01`..`M12`。
pub fn plain_year_month_month_code<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (_, month, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(vm.new_string(&format!("M{:02}", month as u32)))
}

/// `Temporal.PlainYearMonth.prototype.calendarId` getter：读日历槽（槽 3）。
pub fn plain_year_month_calendar_id<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    NativeResult::Ok(vm.new_string(&get_calendar_id(obj, 3)))
}

/// `Temporal.PlainYearMonth.prototype.daysInMonth` getter：按年月的 ISO 月长。
pub fn plain_year_month_days_in_month<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, month, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(days_in_month_iso(year as i32, month as u32) as f64))
}

/// `Temporal.PlainYearMonth.prototype.daysInYear` getter：366（闰年）或 365。
pub fn plain_year_month_days_in_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, _, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(if is_leap_year_iso(year as i32) { 366.0 } else { 365.0 }))
}

/// `Temporal.PlainYearMonth.prototype.monthsInYear` getter：恒 12（ISO 日历）。
pub fn plain_year_month_months_in_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::float(12.0))
}

/// `Temporal.PlainYearMonth.prototype.inLeapYear` getter。
pub fn plain_year_month_in_leap_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let (year, _, _) = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::bool(is_leap_year_iso(year as i32)))
}

/// `Temporal.PlainYearMonth.prototype.era` getter：ISO 日历无纪元，恒 undefined。
pub fn plain_year_month_era<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::undefined())
}

/// `Temporal.PlainYearMonth.prototype.eraYear` getter：ISO 日历无纪元，恒 undefined。
pub fn plain_year_month_era_year<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let _ = native_try!(plain_year_month_ymd(vm, args));
    NativeResult::Ok(JsValue::undefined())
}

/// toString 的 calendarName 选项取值（auto/never 同默认形）。
enum ShowCalendar {
    Omitted,
    Always,
    Critical,
}

/// 读并校验 toString 的 calendarName 选项：options 非对象 → TypeError；
/// 值非字符串先 ToPrimitive(String) 转换（Symbol → TypeError）；
/// 仅 auto/always/never/critical 合法（大小写敏感），否则 RangeError。
fn temporal_to_show_calendar<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<ShowCalendar, JsValue> {
    let options = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if options.is_undefined() {
        return Ok(ShowCalendar::Omitted);
    }
    if !options.is_object() {
        return Err(crate::error::create_type_error(vm, "options must be an object"));
    }
    let ptr = options.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "options must be an object"));
    }
    let raw = temporal_option_value(vm, unsafe { &*ptr }, options, "calendarName")?;
    if raw.is_undefined() {
        return Ok(ShowCalendar::Omitted);
    }
    let value = temporal_option_string(vm, raw)?;
    match value.as_str() {
        "always" => Ok(ShowCalendar::Always),
        "critical" => Ok(ShowCalendar::Critical),
        "auto" | "never" => Ok(ShowCalendar::Omitted),
        _ => Err(crate::error::create_range_error(vm, "invalid calendarName")),
    }
}

/// PlainMonthDay 默认形串（`MM-DD`），含 receiver 校验，不读 options。
fn plain_month_day_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let month = get_double_prop(obj, 0) as i32;
    let day = get_double_prop(obj, 1) as i32;
    NativeResult::Ok(vm.new_string_owned(format!("{month:02}-{day:02}")))
}

/// `Temporal.PlainMonthDay.prototype.toString([options])`：
/// 默认 `MM-DD`；always/critical 补参考年与 `[u-ca=…]`/`[!u-ca=…]` 注解。
pub fn plain_month_day_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let show = native_try!(temporal_to_show_calendar(vm, args));
    if matches!(show, ShowCalendar::Omitted) {
        return plain_month_day_default_string(vm, args);
    }
    let month = get_double_prop(obj, 0) as i32;
    let day = get_double_prop(obj, 1) as i32;
    let ref_year = get_double_prop(obj, 2) as i32;
    let calendar = get_calendar_id(obj, 3);
    let annotation = match show {
        ShowCalendar::Always => format!("[u-ca={calendar}]"),
        _ => format!("[!u-ca={calendar}]"),
    };
    NativeResult::Ok(
        vm.new_string_owned(format!("{}-{month:02}-{day:02}{annotation}", format_iso_year(ref_year as i128))),
    )
}

/// `Temporal.PlainMonthDay.prototype.toJSON()`：默认形串，忽略参数。
pub fn plain_month_day_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_month_day_default_string(vm, args)
}

/// `Temporal.PlainMonthDay.prototype.toLocaleString()`：locale/options 无引擎行为，
/// 恒输出默认形串。
pub fn plain_month_day_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_month_day_default_string(vm, args)
}

/// `Temporal.PlainMonthDay.prototype.valueOf()`：Temporal 对象无值表示，恒 TypeError。
pub fn plain_month_day_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.PlainMonthDay has no valueOf"))
}

/// PlainYearMonth 默认形串（`±YYYY-MM`），含 receiver 校验，不读 options。
fn plain_year_month_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as i32;
    NativeResult::Ok(vm.new_string_owned(format!("{}-{month:02}", format_iso_year(year as i128))))
}

/// `Temporal.PlainYearMonth.prototype.toString([options])`：
/// 默认 `±YYYY-MM`；always/critical 补参考日与 `[u-ca=…]`/`[!u-ca=…]` 注解。
pub fn plain_year_month_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let show = native_try!(temporal_to_show_calendar(vm, args));
    if matches!(show, ShowCalendar::Omitted) {
        return plain_year_month_default_string(vm, args);
    }
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as i32;
    let ref_day = get_double_prop(obj, 2) as i32;
    let calendar = get_calendar_id(obj, 3);
    let annotation = match show {
        ShowCalendar::Always => format!("[u-ca={calendar}]"),
        _ => format!("[!u-ca={calendar}]"),
    };
    NativeResult::Ok(
        vm.new_string_owned(format!("{}-{month:02}-{ref_day:02}{annotation}", format_iso_year(year as i128))),
    )
}

/// `Temporal.PlainYearMonth.prototype.toJSON()`：默认形串，忽略参数。
pub fn plain_year_month_to_json<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_default_string(vm, args)
}

/// `Temporal.PlainYearMonth.prototype.toLocaleString()`：locale/options 无引擎行为，
/// 恒输出默认形串。
pub fn plain_year_month_to_locale_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_default_string(vm, args)
}

/// `Temporal.PlainYearMonth.prototype.valueOf()`：Temporal 对象无值表示，恒 TypeError。
pub fn plain_year_month_value_of<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    NativeResult::Err(crate::error::create_type_error(vm, "Temporal.PlainYearMonth has no valueOf"))
}

// ==================== PlainMonthDay.from ====================

/// PlainMonthDay 串注解校验：与 validate_temporal_annotation_suffix 同构，
/// 但 u-ca 值只接受 iso8601（大小写不敏感），其余内置日历 ID 拒绝。
fn validate_month_day_annotation_suffix(mut suffix: &str) -> Result<(), String> {
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
                } else if !value.eq_ignore_ascii_case("iso8601") {
                    return Err("invalid calendar annotation".into());
                } else {
                    calendar_seen = true;
                    saw_critical_calendar = critical;
                }
            } else if critical {
                return Err("unknown critical annotation".into());
            }
        } else if time_zone_seen {
            return Err("invalid time-zone annotation".into());
        } else {
            time_zone_seen = true;
        }
        suffix = &suffix[end + 1..];
    }
    Ok(())
}

/// PlainMonthDay 串的时间部分校验（值丢弃）：偏移剥离须恰好耗尽剩余文本；
/// 接受 T/t/空格分隔、小时可缺分钟秒、秒可 60（leap second）、小数秒 ≤9 位；
/// 禁止小数时/分、UTC 设计符在调用侧先行拒绝。
fn validate_month_day_time_suffix(input: &str) -> Result<(), String> {
    let time_end = input
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '+' | '-').then_some(index))
        .unwrap_or(input.len());
    if time_end < input.len() && !strip_iso_offset(&input[time_end..])?.is_empty() {
        return Err("invalid ISO offset".into());
    }
    let time = &input[..time_end];
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
    if !valid_plain_time(hour, minute, second, subsecond / 1_000_000, subsecond / 1_000 % 1_000, subsecond % 1_000) {
        return Err("invalid ISO time".into());
    }
    Ok(())
}

/// "MM-DD" 或 "MMDD" 形态的月日解析。
fn parse_month_day_md(input: &str) -> Result<(u32, u32), String> {
    let bytes = input.as_bytes();
    let (m_s, d_s) = if bytes.len() == 4 && bytes.iter().all(|b| b.is_ascii_digit()) {
        (&input[..2], &input[2..])
    } else if bytes.len() == 5
        && bytes[2] == b'-'
        && bytes[..2].iter().all(|b| b.is_ascii_digit())
        && bytes[3..].iter().all(|b| b.is_ascii_digit())
    {
        (&input[..2], &input[3..])
    } else {
        return Err("invalid ISO month-day".into());
    };
    let month: u32 = m_s.parse().map_err(|_| "invalid ISO month".to_string())?;
    let day: u32 = d_s.parse().map_err(|_| "invalid ISO day".to_string())?;
    Ok((month, day))
}

/// PlainMonthDay 串的日期部分：MM-DD / MMDD / --MM-DD / --MMDD / 完整日期（扩展或紧凑）。
/// 年份仅用于负零年拒绝（-000000 → 错），无范围检查；返回 (month, day)。
fn parse_month_day_date_part(input: &str) -> Result<(u32, u32), String> {
    if input.is_empty() {
        return Err("invalid ISO date".into());
    }
    if let Some(rest) = input.strip_prefix("--") {
        let (month, day) = parse_month_day_md(rest)?;
        return finish_month_day_range(month, day);
    }
    let bytes = input.as_bytes();
    let signed = matches!(bytes[0], b'-' | b'+');
    let negative = bytes[0] == b'-';
    let mut i = usize::from(signed);
    let y_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let digits = &input[y_start..i];
    if i < bytes.len() && bytes[i] == b'-' {
        // 扩展形：年份 + -MM-DD；无符号两位引导是 MM-DD 形态。
        if digits.len() == 2 && !signed {
            let (month, day) = parse_month_day_md(input)?;
            return finish_month_day_range(month, day);
        }
        let expected = usize::from(signed) * 6 + 4 * (1 - usize::from(signed));
        if digits.len() != expected {
            return Err("invalid ISO year".into());
        }
        i += 1;
        let month = read_iso2(input, &mut i, bytes)?;
        if i >= bytes.len() || bytes[i] != b'-' {
            return Err("invalid ISO date".into());
        }
        i += 1;
        let day = read_iso2(input, &mut i, bytes)?;
        if i != bytes.len() {
            return Err("invalid trailing content".into());
        }
        if negative && digits == "000000" {
            return Err("invalid ISO negative zero year".into());
        }
        return finish_month_day_range(month, day);
    }
    if i != bytes.len() {
        return Err("invalid trailing content".into());
    }
    // 紧凑纯数字：4 位 = MMDD（无符号）；否则 (4-6 位年) + 4 位月日。
    let len = digits.len();
    if !(len == 4 && !signed || len == 10 && signed || (!signed && (8..=10).contains(&len))) {
        return Err("invalid ISO date".into());
    }
    let tail = &input[y_start..];
    let y_len = tail.len() - 4;
    let (month, day) = parse_month_day_md(&tail[y_len..])?;
    if y_len == 6 && negative && &tail[..6] == "000000" {
        return Err("invalid ISO negative zero year".into());
    }
    finish_month_day_range(month, day)
}

/// 月日取值边界：月 1..=12、日 1..=31（串路径无参考年，不做月长检查）。
fn finish_month_day_range(month: u32, day: u32) -> Result<(u32, u32), String> {
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return Err("invalid ISO date".into());
    }
    Ok((month, day))
}

/// 解析 PlainMonthDay ISO 串（"MM-DD"/"MMDD"/"--MM-DD"/完整日期/完整 datetime）。
/// 注解与时间部分按规则校验；参考年恒 1972（串中年份丢弃）。
fn parse_month_day_string(input: &str) -> Result<(u32, u32), String> {
    let trimmed = input.trim();
    if trimmed.contains('\u{2212}') {
        return Err("variant minus sign is not valid for PlainMonthDay".into());
    }
    let text = trimmed.to_owned();
    let annotation_start = text.find('[').unwrap_or(text.len());
    validate_month_day_annotation_suffix(&text[annotation_start..])?;
    let text = &text[..annotation_start];
    if text.contains('Z') || text.contains('z') {
        return Err("UTC designator is not valid for PlainMonthDay".into());
    }
    let separator = text.find(['T', 't', ' ']);
    let (date_part, time_part) = match separator {
        Some(index) => (&text[..index], &text[index + 1..]),
        None => (text, ""),
    };
    if !time_part.is_empty() {
        validate_month_day_time_suffix(time_part)?;
    }
    parse_month_day_date_part(date_part)
}

/// bag 日历解析：undefined → iso8601；字符串走宽松日历串解析（18-ID 白名单归一小写，
/// 合法 ISO 日期-时间串归一 iso8601，含 YYYY-MM / MM-DD 部分日期）；
/// PD/PDT/ZDT/PMD/PYM 实例 → 直读日历槽；其余对象与原始值 → TypeError。
fn month_day_bag_calendar_id<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
    if value.is_undefined() {
        return Ok("iso8601".to_string());
    }
    if value.is_string() {
        let text = to_string(value);
        return parse_temporal_calendar_string(&text)
            .map_err(|_| crate::error::create_range_error(vm, "invalid calendar"));
    }
    if value.is_object() {
        let ptr = value.as_js_object_ptr();
        if !ptr.is_null() {
            let obj = unsafe { &*ptr };
            let slot = if obj.is_plain_date_obj() {
                Some(3)
            } else if obj.is_plain_date_time_obj() {
                Some(4)
            } else if obj.is_zoned_date_time_obj() {
                Some(2)
            } else if obj.is_plain_month_day_obj() || obj.is_plain_year_month_obj() {
                Some(3)
            } else {
                None
            };
            if let Some(slot) = slot {
                return Ok(get_calendar_id(obj, slot));
            }
        }
        return Err(crate::error::create_type_error(vm, "invalid calendar"));
    }
    Err(crate::error::create_type_error(vm, "invalid calendar"))
}

/// `Temporal.PlainMonthDay.from(item[, options])`：ISO 串 / PMD 实例 / PD 实例 / 字段对象。
/// 参考年恒 1972（串中年份与 bag year 均不入结果，year 仅供 overflow 月长判断）；
/// options.overflow 在全部字段 Get 之后读取。
pub fn plain_month_day_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if value.is_string() {
        // 规范顺序：先解析（错误即抛，不触 options），再读 overflow（结果不受其影响）。
        let (month, day) = native_try!(parse_month_day_string(&to_string(value))
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 month-day string")));
        native_try!(temporal_overflow(vm, args));
        return make_plain_month_day(vm, month, day, 1972, "iso8601");
    }
    if !value.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "from() argument must be a string or an object"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "from() argument must be a string or an object"));
    }
    let obj = unsafe { &*ptr };
    // 实例快路径：槽直读，不触发属性。
    if obj.is_plain_month_day_obj() {
        // 实例复制：槽直读（refYear/日历保留），overflow 读取但结果不受其影响。
        native_try!(temporal_overflow(vm, args));
        let calendar = get_calendar_id(obj, 3);
        return make_plain_month_day(
            vm,
            get_double_prop(obj, 0) as u32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as i32,
            &calendar,
        );
    }
    if obj.is_plain_date_obj() {
        // PlainDate 实例：年月日槽直读，overflow 按其年应用，参考年 1972。
        let constrain = native_try!(temporal_overflow(vm, args));
        let year = get_double_prop(obj, 0) as i32;
        let month = get_double_prop(obj, 1) as u32;
        let mut day = get_double_prop(obj, 2) as u32;
        let calendar = get_calendar_id(obj, 3);
        if constrain {
            day = day.min(days_in_month_iso(year, month));
        }
        return make_plain_month_day(vm, month, day, 1972, &calendar);
    }
    // 字段对象：Get 序 calendar → day → month → monthCode → year → era → eraYear，
    // 全部读原始值（不转换）；overflow 在全部 Get 之后读取。
    let calendar_raw = native_try!(temporal_option_value(vm, obj, value, "calendar"));
    let calendar = native_try!(month_day_bag_calendar_id(vm, calendar_raw));
    let day_raw = native_try!(temporal_option_value(vm, obj, value, "day"));
    let month_raw = native_try!(temporal_option_value(vm, obj, value, "month"));
    let month_code_raw = native_try!(temporal_option_value(vm, obj, value, "monthCode"));
    let month_code = match month_code_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_option_string(vm, month_code_raw))),
    };
    let year_raw = native_try!(temporal_option_value(vm, obj, value, "year"));
    let era_raw = native_try!(temporal_option_value(vm, obj, value, "era"));
    let era_year_raw = native_try!(temporal_option_value(vm, obj, value, "eraYear"));
    let constrain = native_try!(temporal_overflow(vm, args));
    // era 族：双现 → RangeError；恰一个 → 忽略（ISO 无纪元体系）。
    if !era_raw.is_undefined() && !era_year_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_range_error(vm, "era and eraYear cannot both be present"));
    }
    // 月来源：双给 → RangeError；双缺 → TypeError（先于日存在性）。
    if !month_raw.is_undefined() && month_code.is_some() {
        return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode cannot both be present"));
    }
    if month_raw.is_undefined() && month_code.is_none() {
        return NativeResult::Err(crate::error::create_type_error(vm, "month or monthCode is required"));
    }
    if day_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "day is required"));
    }
    // monthCode 两段校验：第一段语法（"M"+两位数字，可选再 +"L"）先于一切数值转换；
    // 第二段适配（闰月后缀、1..12 范围）在 year 转换之后。
    let (month_code_num, leap_month) = match &month_code {
        Some(text) => {
            let b = text.as_bytes();
            let well_formed = b[0] == b'M'
                && b.len() >= 3
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && (b.len() == 3 || (b.len() == 4 && b[3] == b'L'));
            if !well_formed {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
            }
            (Some(u32::from(b[1] - b'0') * 10 + u32::from(b[2] - b'0')), b.len() == 4)
        }
        None => (None, false),
    };
    // year 转换（TypeError/RangeError）在 monthCode 语法之后。
    let year = match year_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, year_raw)) as i32),
    };
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, month_raw))),
    };
    // monthCode 第二段适配：闰月后缀或月值越界 → RangeError。
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：monthCode 与数值 month 并存须一致（冲突 → RangeError）；
    // 数值 month 负值恒 RangeError，>12 时 constrain 钳 12、reject 抛错。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        let f = month_f.expect("month or monthCode is required checked above");
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
        }
        if f > 12.0 {
            if constrain {
                12
            } else {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
            }
        } else {
            f as u32
        }
    };
    let day_f = native_try!(temporal_number_component(vm, day_raw));
    if day_f < 1.0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
    }
    // overflow 应用：月长按 year（bag 缺省 1972）判断；constrain 钳制，reject 抛错。
    let overflow_year = year.unwrap_or(1972);
    let days = days_in_month_iso(overflow_year, month) as i32;
    let day_i = day_f as i32;
    let day = if day_i > days {
        if constrain {
            days
        } else {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
        }
    } else {
        day_i
    };
    make_plain_month_day(vm, month, day as u32, 1972, &calendar)
}

/// `Temporal.PlainMonthDay.prototype.equals(other)`：比较 (月, 日, 参考年) 三元与日历标识；
/// other 经 ToTemporalMonthDay（串 / 字段对象 / 实例）转换。
pub fn plain_month_day_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let other = match plain_month_day_from(vm, args) {
        NativeResult::Ok(val) => val,
        NativeResult::Err(err) => return NativeResult::Err(err),
        NativeResult::TailCall { .. } => unreachable!("工厂路径不产生尾调用"),
    };
    let other_ptr = other.as_js_object_ptr();
    if other_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "incompatible receiver"));
    }
    let o = unsafe { &*other_ptr };
    let equal = get_double_prop(obj, 0) == get_double_prop(o, 0)
        && get_double_prop(obj, 1) == get_double_prop(o, 1)
        && get_double_prop(obj, 2) == get_double_prop(o, 2)
        && get_calendar_id(obj, 3) == get_calendar_id(o, 3);
    NativeResult::Ok(JsValue::bool(equal))
}

/// `Temporal.PlainMonthDay.prototype.with(monthDayLike[, options])`：覆盖 day/month/monthCode；
/// year 只参与 overflow 月长判断；undefined 不覆盖；calendar/timeZone 键拒绝；
/// 无识别字段 TypeError；overflow 读自 options（缺省 constrain）。
/// 结果保留 receiver 日历，参考年恒 1972。
pub fn plain_month_day_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let like = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    native_try!(reject_partial_object_with_calendar_or_time_zone(vm, like));
    let lptr = like.as_js_object_ptr();
    if lptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let like_obj = unsafe { &*lptr };
    // Get 序（字典序）：day → month → monthCode → year；无识别字段 → TypeError。
    let day_raw = native_try!(temporal_option_value(vm, like_obj, like, "day"));
    let month_raw = native_try!(temporal_option_value(vm, like_obj, like, "month"));
    let month_code_raw = native_try!(temporal_option_value(vm, like_obj, like, "monthCode"));
    let month_code = match month_code_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_option_string(vm, month_code_raw))),
    };
    let year_raw = native_try!(temporal_option_value(vm, like_obj, like, "year"));
    if [day_raw, month_raw, month_code_raw, year_raw]
        .iter()
        .all(|raw| raw.is_undefined())
    {
        return NativeResult::Err(crate::error::create_type_error(vm, "no properties present"));
    }
    let constrain = native_try!(temporal_overflow(vm, args));
    // monthCode 两段校验：语法（"M"+两位数字，可选 +"L"）先于 year 转换；
    // 适配（闰月后缀、1..12 范围）与冲突检查在 year 转换之后。
    let (month_code_num, leap_month) = match &month_code {
        Some(text) => {
            let b = text.as_bytes();
            let well_formed = b[0] == b'M'
                && b.len() >= 3
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && (b.len() == 3 || (b.len() == 4 && b[3] == b'L'));
            if !well_formed {
                return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
            }
            (Some(u32::from(b[1] - b'0') * 10 + u32::from(b[2] - b'0')), b.len() == 4)
        }
        None => (None, false),
    };
    let year = match year_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, year_raw)) as i32),
    };
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, month_raw))),
    };
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：receiver 月（槽 0）为缺省；monthCode 与数值 month 并存须一致；
    // 数值 month 负值恒 RangeError，>12 时 constrain 钳 12、reject 抛错。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        match month_f {
            Some(f) => {
                if f < 1.0 {
                    return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
                }
                if f > 12.0 {
                    if constrain {
                        12
                    } else {
                        return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
                    }
                } else {
                    f as u32
                }
            }
            None => get_double_prop(obj, 0) as u32,
        }
    };
    // 日：receiver 日（槽 1）为缺省；<1 恒 RangeError。
    let day_f = if day_raw.is_undefined() {
        get_double_prop(obj, 1)
    } else {
        native_try!(temporal_number_component(vm, day_raw))
    };
    if day_f < 1.0 {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
    }
    // overflow 应用：月长按 bag year（缺省 1972）判断；constrain 钳制，reject 抛错。
    let overflow_year = year.unwrap_or(1972);
    let days = days_in_month_iso(overflow_year, month) as i32;
    let day_i = day_f as i32;
    let day = if day_i > days {
        if constrain {
            days
        } else {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
        }
    } else {
        day_i
    };
    let calendar = get_calendar_id(obj, 3);
    make_plain_month_day(vm, month, day as u32, 1972, &calendar)
}

/// `Temporal.PlainMonthDay.prototype.toPlainDate(yearLike)`：yearLike 为 number 或 {year}；
/// 只 Get item 的 year；overflow 恒 constrain 且不读 options；
/// constrain 钳 day、day 级范围检查后产出 PlainDate（receiver 日历）。
pub fn plain_month_day_to_plain_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let item = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !item.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let iptr = item.as_js_object_ptr();
    if iptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let item_obj = unsafe { &*iptr };
    let year_raw = native_try!(temporal_option_value(vm, item_obj, item, "year"));
    if year_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "year is required"));
    }
    let year = native_try!(temporal_number_component(vm, year_raw)) as i32;
    let month = get_double_prop(obj, 0) as u32;
    // constrain：day 钳到 (year, month) 月长。
    let day = (get_double_prop(obj, 1) as u32).min(days_in_month_iso(year, month));
    // day 级表示范围：-100_000_001..=100_000_000。
    let day_count = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
    }
    let calendar = get_calendar_id(obj, 3);
    make_plain_date(vm, year, month, day, &calendar)
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

    #[test]
    fn start_of_day_epoch_ns_instant_range() {
        // 当地午夜回推 epoch 越 Instant 界（±MAX）返回 None，供 startOfDay/hoursInDay RangeError 依据。
        // -100000001 天 + 1h < -MAX → 越界。
        assert_eq!(start_of_day_epoch_ns(-271_821, 4, 19, -60), None);
        // -100000000 天（-271821-04-20）UTC 当地午夜恰为 -MAX，在边界内。
        assert_eq!(start_of_day_epoch_ns(-271_821, 4, 20, 0), Some(-8_640_000_000_000_000_000_000));
        // 同日期 +1h 偏移使当地午夜 -MAX - 1h 越界。
        assert_eq!(start_of_day_epoch_ns(-271_821, 4, 20, 60), None);
        // 明日边界越界：+100000001 天（UTC）。
        assert_eq!(start_of_day_epoch_ns_by_days(100_000_001, 0), None);
        // 正常日期返回当地午夜。
        assert_eq!(start_of_day_epoch_ns(1970, 1, 1, 0), Some(0));
        assert_eq!(start_of_day_epoch_ns(1970, 1, 1, 60), Some(-3_600_000_000_000));
    }

    #[test]
    fn format_time_zone_annotation_normalizes_offset() {
        // 命名区原样；偏移区规范化为 ±HH:MM 带冒号；critical 时 `!` 置于括号内。
        assert_eq!(format_time_zone_annotation("UTC", false), "[UTC]");
        assert_eq!(format_time_zone_annotation("+01:00", false), "[+01:00]");
        assert_eq!(format_time_zone_annotation("+01", false), "[+01:00]");
        assert_eq!(format_time_zone_annotation("-05:00", false), "[-05:00]");
        assert_eq!(format_time_zone_annotation("UTC", true), "[!UTC]");
        assert_eq!(format_time_zone_annotation("+01", true), "[!+01:00]");
    }

    #[test]
    fn format_zoned_date_time_iso_annotations() {
        // 偏移/时区/日历注解各取值组合，验证段序 `{offset}[{tz}][{ca}]` 与 critical 前缀。
        let base = format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "auto", "auto", "auto");
        assert_eq!(base, Some("1970-01-01T01:00:00+01:00[+01:00]".to_string()));
        // offset never 省略偏移段，时区注解仍显示。
        let no_offset = format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "never", "auto", "auto");
        assert_eq!(no_offset, Some("1970-01-01T01:00:00[+01:00]".to_string()));
        // calendarName always 追加日历注解。
        let ca_always = format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "auto", "auto", "always");
        assert_eq!(ca_always, Some("1970-01-01T01:00:00+01:00[+01:00][u-ca=iso8601]".to_string()));
        // timeZoneName/calendarName critical 均 `!` 置于括号内。
        let critical =
            format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "auto", "critical", "critical");
        assert_eq!(critical, Some("1970-01-01T01:00:00+01:00[!+01:00][!u-ca=iso8601]".to_string()));
        // offset critical 偏移段前加 !。
        let offset_critical =
            format_zoned_date_time_iso(0, 60, "+01:00", "iso8601", true, None, "critical", "auto", "auto");
        assert_eq!(offset_critical, Some("1970-01-01T01:00:00!+01:00[+01:00]".to_string()));
    }

    #[test]
    fn format_zoned_date_time_iso_epoch_rounding_cross_midnight() {
        // 2000-01-01 前 1ns 已舍入到 2000-01-01 整点，8 位小数输出。
        let rounded = round_instant_ns(946_684_799_999_999_999, 100, InstantRoundingMode::HalfExpand).unwrap();
        let output = format_zoned_date_time_iso(rounded, 0, "UTC", "iso8601", true, Some(8), "auto", "auto", "auto");
        assert_eq!(output, Some("2000-01-01T00:00:00.00000000+00:00[UTC]".to_string()));
    }

    #[test]
    fn local_to_epoch_ns_roundtrip() {
        // 与 parse_instant_string 互逆对拍：同一时刻的本地分量 + 偏移换算回 epoch 一致。
        assert_eq!(parse_instant_string("2024-01-01T00:00:00+01:00"), local_to_epoch_ns(2024, 1, 1, 0.0, 60),);
        assert_eq!(
            parse_instant_string("1969-07-16T13:32:01.234567891Z"),
            local_to_epoch_ns(1969, 7, 16, 48_721_234_567_891.0, 0),
        );
        // startOfDay 最小边界：-271821-04-20 加 1h 再回推 1h 偏移回到 -MAX。
        assert_eq!(
            local_to_epoch_ns(-271821, 4, 20, 3_600_000_000_000.0, 60),
            Some(-8_640_000_000_000_000_000_000),
        );
    }

    #[test]
    fn zoned_date_time_string_wall_day_range_boundary() {
        // ZDT 字符串的墙钟日范围（CheckISODaysRange）边界：±10^8 天内合法，
        // 第 ±100000001 天（-271821-04-19 / +275760-09-14）即使 epoch 换算回界内也拒绝。
        assert!(days_from_civil(-271_821, 4, 19).abs() > 100_000_000);
        assert!(days_from_civil(-271_821, 4, 20).abs() <= 100_000_000);
        assert!(days_from_civil(275_760, 9, 14).abs() > 100_000_000);
        assert!(days_from_civil(275_760, 9, 13).abs() <= 100_000_000);
        // 解析路径一致性：边界内字符串可解析，越界墙钟经偏移拉回界内也不放行。
        assert_eq!(
            parse_instant_string("+275760-09-13T01:00+01:00[+01:00]"),
            Some(8_640_000_000_000_000_000_000),
        );
    }

    #[test]
    fn canonical_time_zone_4_digit_offset() {
        // ±HHMM 无冒号形式归一：ID 保留原串，offset 分钟数正确换算。
        assert_eq!(canonical_time_zone("+0000"), Some(("+0000".to_string(), 0)));
        assert_eq!(canonical_time_zone("-0530"), Some(("-0530".to_string(), -330)));
        assert_eq!(canonical_time_zone("+2330"), Some(("+2330".to_string(), 1410)));
        // 非法分钟/小时拒绝。
        assert_eq!(canonical_time_zone("+2400"), None);
        assert_eq!(canonical_time_zone("+0060"), None);
        assert_eq!(canonical_time_zone("+123"), None); // 长度不符
    }

    #[test]
    fn extract_time_zone_annotation_forms() {
        // UTC / critical / 数值偏移各形式提取；u-ca 注解跳过，时区注解在前后均能取到。
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[UTC]"), Some(("UTC".to_string(), false)));
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[!UTC]"), Some(("UTC".to_string(), true)));
        assert_eq!(
            extract_time_zone_annotation("1976-11-18T15:23:30+01:00[+01:00]"),
            Some(("+01:00".to_string(), false))
        );
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[+01]"), Some(("+01".to_string(), false)));
        assert_eq!(
            extract_time_zone_annotation("1976-11-18T15:23:30[+0100]"),
            Some(("+0100".to_string(), false))
        );
        assert_eq!(
            extract_time_zone_annotation("1976-11-18T15:23:30[u-ca=iso8601][UTC]"),
            Some(("UTC".to_string(), false))
        );
        assert_eq!(
            extract_time_zone_annotation("1976-11-18T15:23:30[UTC][u-ca=iso8601]"),
            Some(("UTC".to_string(), false))
        );
        // 无注解 / 非法偏移注解返回 None。
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30Z"), None);
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[+24:00]"), None);
        assert_eq!(extract_time_zone_annotation("1976-11-18T15:23:30[]"), None);
    }

    #[test]
    fn parse_plain_time_string_cases() {
        // 明确时间：T 前缀 / 冒号 / 非法日期数字串 / 带 offset 与注解。
        assert_eq!(parse_plain_time_string("T00:30"), Some(1_800_000_000_000.0));
        assert_eq!(parse_plain_time_string("T0030"), Some(1_800_000_000_000.0));
        assert_eq!(parse_plain_time_string("12:34:56.987654321+00:00"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("12:34:56.987654321+00:00[UTC]"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("1976-11-18T12:34:56.987654321+00:00"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("1976-11-18 12:34:56.987654321"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("1314"), Some(47_640_000_000_000.0)); // 13:14
        assert_eq!(parse_plain_time_string("0631"), Some(23_460_000_000_000.0)); // 06:31
        assert_eq!(parse_plain_time_string("2021-13"), Some(73_260_000_000_000.0)); // 20:21 + offset -13 忽略
                                                                                    // 歧义日期 → None（须 T 前缀）。
        assert_eq!(parse_plain_time_string("2019-10-01"), None);
        assert_eq!(parse_plain_time_string("1214"), None); // MMDD 合法
        assert_eq!(parse_plain_time_string("0229"), None); // 闰年 2 月 29 判歧义
        assert_eq!(parse_plain_time_string("1130"), None);
        assert_eq!(parse_plain_time_string("12-14"), None); // MM-DD 合法
        assert_eq!(parse_plain_time_string("202112"), None); // YYYYMM 合法
        assert_eq!(parse_plain_time_string("2021-12"), None); // YYYY-MM 合法
        assert_eq!(parse_plain_time_string("2021-12[-12:00]"), None);
        assert_eq!(parse_plain_time_string("202112[UTC]"), None);
        assert_eq!(parse_plain_time_string("1214[u-ca=iso8601]"), None);
        // T 前缀后歧义消除；空格不能替代 T。
        assert!(parse_plain_time_string("T2021-12").is_some());
        assert_eq!(parse_plain_time_string(" 2021-12"), None);
        // Z designator / 越界 / 多小数 / 负零年。
        assert_eq!(parse_plain_time_string("09:00:00Z"), None);
        assert_eq!(parse_plain_time_string("2019-10-01T09:00:00Z"), None);
        assert_eq!(parse_plain_time_string("24:00"), None);
        assert_eq!(parse_plain_time_string("12:34:56.1234567890"), None);
        assert_eq!(parse_plain_time_string("-000000-12-07T03:24:30"), None);
        // 闰秒按前一秒。
        assert_eq!(parse_plain_time_string("2016-12-31T23:59:60"), Some(86_399_000_000_000.0));
    }

    #[test]
    fn parse_plain_time_string_offsets_and_fractions() {
        // offset 小数秒 ≤9 位合法，>9 位拒绝；逗号小数接受。
        assert_eq!(
            parse_plain_time_string("12:34:56.987654321+00:00:00.000000000"),
            Some(45_296_987_654_321.0)
        );
        assert_eq!(parse_plain_time_string("12:34:56.987654321+00:00:00,0"), Some(45_296_987_654_321.0));
        assert_eq!(parse_plain_time_string("00:00:00.1234567891"), None);
        assert_eq!(parse_plain_time_string("00+00:00:00.1234567891"), None);
        // 日期+offset 但无时间部分 → 拒绝。
        assert_eq!(parse_plain_time_string("2022-09-15Z"), None);
        assert_eq!(parse_plain_time_string("2022-09-15+00:00"), None);
    }

    #[test]
    fn zoned_date_time_round_unit_table() {
        // day 含独立条目，每单位 (ns, 更高一级数量) 与 spec 对齐；day 更高一级数量为 1。
        assert_eq!(zoned_date_time_round_unit("day"), Some((86_400_000_000_000, 1)));
        assert_eq!(zoned_date_time_round_unit("days"), Some((86_400_000_000_000, 1)));
        assert_eq!(zoned_date_time_round_unit("hour"), Some((3_600_000_000_000, 24)));
        assert_eq!(zoned_date_time_round_unit("minute"), Some((60_000_000_000, 60)));
        assert_eq!(zoned_date_time_round_unit("second"), Some((1_000_000_000, 60)));
        assert_eq!(zoned_date_time_round_unit("millisecond"), Some((1_000_000, 1_000)));
        assert_eq!(zoned_date_time_round_unit("microsecond"), Some((1_000, 1_000)));
        assert_eq!(zoned_date_time_round_unit("nanosecond"), Some((1, 1_000)));
        // year/month/week 及拼写错误不在单位表。
        assert_eq!(zoned_date_time_round_unit("years"), None);
        assert_eq!(zoned_date_time_round_unit("months"), None);
        assert_eq!(zoned_date_time_round_unit("weeks"), None);
        assert_eq!(zoned_date_time_round_unit("hourz"), None);
    }

    #[test]
    fn zoned_date_time_round_else_path_epoch() {
        // 217175010123456789n +01:00：本地 time_ns = 55_410_123_456_789。
        // hour/4 quantum 14_400e9 → rounded_time 57_600e9 → local_to_epoch_ns 回推 217177200000000000。
        let quantum = 3_600_000_000_000 * 4;
        let rounded = round_instant_ns(55_410_123_456_789, quantum, InstantRoundingMode::HalfExpand).unwrap();
        assert_eq!(rounded, 57_600_000_000_000);
        // 本地墙钟日回推：offset 60 分，local_to_epoch_ns 得目标 epoch。
        let epoch = local_to_epoch_ns(1976, 11, 18, rounded as f64, 60).unwrap();
        assert_eq!(epoch, 217_177_200_000_000_000);
    }

    #[test]
    fn zoned_date_time_round_day_path_epoch() {
        // 同日本地午夜 startNs（2513 天，offset 60 分），dayProgress 舍到次日 → 217206000000000000。
        let start_ns = start_of_day_epoch_ns(1976, 11, 18, 60).unwrap();
        assert_eq!(start_ns, 217_119_600_000_000_000);
        let day_progress = 217_175_010_123_456_789 - start_ns;
        let rounded = round_instant_ns(day_progress, 86_400_000_000_000, InstantRoundingMode::HalfExpand).unwrap();
        assert_eq!(rounded, 86_400_000_000_000);
        assert_eq!(start_ns + rounded, 217_206_000_000_000_000);
    }

    #[test]
    fn format_plain_time_iso_seconds_and_fractions() {
        // 无小数（亚秒为 0）：仅 HH:MM:SS。
        assert_eq!(format_plain_time_iso(45_296_000_000_000, true, None), "12:34:56");
        // 固定小数位补足 9 位。
        assert_eq!(format_plain_time_iso(45_296_987_654_321, true, Some(9)), "12:34:56.987654321");
        // 固定 3 位截断亚秒。
        assert_eq!(format_plain_time_iso(45_296_987_654_321, true, Some(3)), "12:34:56.987");
        // 固定 0 位不输出小数。
        assert_eq!(format_plain_time_iso(45_296_987_654_321, true, Some(0)), "12:34:56");
        // 自动模式去尾零（.500 归一为 .5，整秒无小数）。
        assert_eq!(format_plain_time_iso(45_296_500_000_000, true, None), "12:34:56.5");
        assert_eq!(format_plain_time_iso(45_296_000_000_000, true, None), "12:34:56");
        // 不含秒：仅 HH:MM。
        assert_eq!(format_plain_time_iso(45_296_000_000_000, false, Some(0)), "12:34");
        assert_eq!(format_plain_time_iso(45_296_987_654_321, false, None), "12:34");
    }

    #[test]
    fn plain_time_rounding_cross_midnight() {
        // 23:59:59.9 以 second 舍入（halfExpand）→ 24:00:00 → 取模回 00:00:00。
        let total_ns = 86_399_900_000_000_i128;
        let quantum = 1_000_000_000_i128;
        let rounded = round_instant_ns(total_ns, quantum, InstantRoundingMode::HalfExpand).unwrap();
        assert_eq!(rounded, 86_400_000_000_000);
        const DAY_NS: i128 = 86_400_000_000_000;
        assert_eq!(rounded.rem_euclid(DAY_NS), 0);
        assert_eq!(format_plain_time_iso(rounded.rem_euclid(DAY_NS), true, Some(0)), "00:00:00");
    }

    #[test]
    fn plain_time_round_increment_real_factor() {
        // 真因子校验：增量须严格小于最大增量且能整除（MaximumTemporalDurationRoundingIncrement）。
        // hour 合法增量：[1,2,3,4,6,8,12]，24（= 最大增量）与 11（不整除）均拒。
        let (_, max_increment) = plain_time_round_unit("hour").unwrap();
        for increment in [1, 2, 3, 4, 6, 8, 12] {
            assert!(increment < max_increment && max_increment % increment == 0, "hour {increment} 应合法");
        }
        for increment in [11, 24] {
            assert!(increment >= max_increment || max_increment % increment != 0, "hour {increment} 应拒绝");
        }
        // minute 合法增量：整除 60 且严格小于 60；60（= 最大增量）与 29（不整除）均拒。
        let (_, max_increment) = plain_time_round_unit("minute").unwrap();
        for increment in [1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30] {
            assert!(increment < max_increment && max_increment % increment == 0, "minute {increment} 应合法");
        }
        for increment in [29, 60] {
            assert!(increment >= max_increment || max_increment % increment != 0, "minute {increment} 应拒绝");
        }
        // millisecond 最大增量 1000：1000 拒，29 拒。
        let (_, max_increment) = plain_time_round_unit("millisecond").unwrap();
        assert!(max_increment == 1000);
        for increment in [29, 1000] {
            assert!(increment >= max_increment || max_increment % increment != 0, "ms {increment} 应拒绝");
        }
    }

    #[test]
    fn plain_time_apply_duration_ignores_date_units() {
        // 时间域加总仅取 hours 起字段；days 及以上对 PlainTime 忽略（与 instant_round 日期单位报错不同）。
        let mut values = [0.0; 10];
        values[3] = 5.0; // days
        values[4] = 2.0; // hours
        values[5] = 30.0; // minutes
        let time_delta = duration_component_integer(values[4]).unwrap() * 3_600_000_000_000
            + duration_component_integer(values[5]).unwrap() * 60_000_000_000;
        assert_eq!(time_delta, 9_000_000_000_000); // 2h30m
                                                   // 忽略 days：time_delta 不含 DAY_NS 分量。
        assert_eq!(time_delta % 86_400_000_000_000, 9_000_000_000_000);
    }
}
