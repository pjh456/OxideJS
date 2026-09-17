//! Temporal.ZonedDateTime：ISO 字符串与 property bag 解析、构造、getter、
//! with/until/since、舍入、加减与格式化。

use chrono::{Datelike, NaiveDate};
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::common::native_try;
use super::common::{
    canonical_time_zone, civil_from_days, days_from_civil, days_in_month, ensure_zoned_date_time, format_iso_year,
    get_calendar_id, get_double_prop, get_instant_epoch_ns, initialize_temporal_receiver, is_ctor_call,
    local_to_epoch_ns, make_instant, make_zoned_date_time, parse_digits, parse_fractional_second_digits,
    parse_temporal_string_impl, plain_date_time_object_parts, receiver_obj,
    reject_partial_object_with_calendar_or_time_zone, temporal_calendar_id, temporal_calendar_id_strict,
    temporal_option_number, temporal_option_string, temporal_option_value, temporal_overflow, temporal_string_strict,
    zoned_date_time_plain_parts, FractionalSecondDigitsInput, MAX_INSTANT_NS,
};
use super::difference::{difference_core, parse_difference_settings, MAX_ISO_DAY};
use super::instant::{
    instant_rounding_mode, instant_string_without_annotations, instant_time_zone_offset, parse_instant_string,
    primitive_to_bigint, round_instant_ns,
};
use super::{
    days_in_month_iso, duration_component_integer, duration_like_values, is_leap_year_iso, make_duration,
    make_plain_date, make_plain_date_time, make_plain_time, plain_time_components, plain_time_like_ns, valid_iso_date,
    valid_plain_date_time_range,
};

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
pub(crate) fn extract_time_zone_annotation(input: &str) -> Option<(String, bool)> {
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
pub(crate) fn parse_any_offset_minutes(value: &str) -> Option<i32> {
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
pub(crate) fn zoned_date_time_string_parts<H: VmHost>(
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
/// - timeZone、offset 的读取与规范化先于年/月/日字段，读取顺序固定不可交换。
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
pub(crate) fn format_time_zone_annotation(time_zone_id: &str, critical: bool) -> String {
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
pub(crate) fn format_zoned_date_time_iso(
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

/// with 的部分字段读取与合并：按字典序读 bag 字段，与 receiver 默认分量合并。
///
/// # 步骤
/// 1. 数值字段 ToNumber→trunc（NaN/±Inf RangeError，day 额外拒绝 <1）；monthCode/offset 走 ToString。
/// 2. undefined 不覆盖；至少一个字段有定义否则 TypeError。
/// 3. 返回合并后的原始分量（未钳制）+ monthCode 解析 + bag offset 分钟。
///
/// # 边界与前提
/// - 调用方须先完成 RejectObjectWithCalendarOrTimeZone（calendar/timeZone 已拒绝）。
/// - monthCode 的闰月、超界与同 month 冲突的校验推迟到 options 解析完成之后：
///   选项本身的取值错误优先于这些算法性校验抛出。
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

    // 数值字段的转换：先 ToNumber 再向零截断；结果为 NaN 或 ±Infinity 时抛
    // RangeError，day 还要求截断后的值不小于 1。
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

    // monthCode 先经 ToString，再做格式校验：必须形如 M 加两位数字，可再带一个
    // L 后缀；闰月与月份越界的判定推迟到随后按日历解析字段时进行。
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

    // offset 先经 ToString，再按偏移字符串语法校验（小数秒最多 9 位），解析出的
    // 分钟数留给 offset 选项决策使用。
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
/// 1. 校验 receiver 类型标签，并排除 partial 参数携带的日历/时区：参数非对象、为
///    Temporal 实例或 calendar、timeZone 有定义均抛 TypeError（IsPartialTemporalObject 语义）。
/// 2. receiver 本地分量与偏移；zoned_date_time_with_fields 读字段合并。
/// 3. options：disambiguation → offset（默认 prefer）→ overflow（逐项 Get/校验）。
/// 4. monthCode 闰月/超界/冲突校验 + constrain/reject 钳制 + PlainDateTime 范围校验。
/// 5. offset 选项决策 → local_to_epoch_ns → Instant 范围校验 → make_zoned_date_time。
///
/// # 边界与前提
/// - 字段读取先于 options 解析：即使 options 本身类型不合法，也先读完 partial 字段，
///   让字段的类型错误先抛出。
/// - options 解析先于 monthCode 的算法校验：选项取值错误优先于 monthCode 超界这类
///   算法性错误抛出。
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

    // 按 ISO 日历解析合并后的月字段（CalendarResolveFields 语义）：partial 的 monthCode
    // 带闰月后缀或月值超 12 → RangeError；month 与 monthCode 的一致性校验仅在两者
    // 都来自 partial 时执行（partial 有 monthCode 时 receiver 的 month 被覆盖，
    // 不参与比较）。
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
        // reject 语义须先做范围校验再截断：负分量或超上限分量立即抛错，
        // 不允许 f64→u32 饱和把负值静默归 0 而逃过范围检查。
        let limits = [23.0, 59.0, 59.0, 999.0, 999.0, 999.0];
        let values = [
            merged.hour,
            merged.minute,
            merged.second,
            merged.millisecond,
            merged.microsecond,
            merged.nanosecond,
        ];
        if values
            .iter()
            .zip(limits.iter())
            .any(|(value, limit)| *value < 0.0 || *value > *limit)
        {
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
pub(crate) fn valid_offset_fraction(value: &str) -> bool {
    let Some(dot) = value.find(['.', ',']) else {
        return true;
    };
    let fraction = &value[dot + 1..];
    fraction.len() <= 9 && fraction.bytes().all(|byte| byte.is_ascii_digit())
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
        9,
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
pub(crate) fn zoned_date_time_round_unit(value: &str) -> Option<(i128, i128)> {
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

    // roundTo 解析：字符串简写直接取用，对象选项按 roundingIncrement、roundingMode、
    // smallestUnit 的固定读取顺序取值。
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

/// 求本地日期在给定时区偏移下的当地午夜纪元纳秒（GetStartOfDay 语义），
/// 结果超出 Instant 范围时返回 None。
pub(crate) fn start_of_day_epoch_ns(year: i32, month: u32, day: u32, offset_minutes: i32) -> Option<i128> {
    start_of_day_epoch_ns_by_days(days_from_civil(i128::from(year), i128::from(month), i128::from(day)), offset_minutes)
}

/// 按日数直接算当地午夜（hoursInDay 的明日边界复用：days+1 不经 civil 回填）。
pub(crate) fn start_of_day_epoch_ns_by_days(days: i128, offset_minutes: i32) -> Option<i128> {
    let start = days
        .checked_mul(86_400_000_000_000)?
        .checked_sub(i128::from(offset_minutes) * 60_000_000_000)?;
    (start.unsigned_abs() <= MAX_INSTANT_NS as u128).then_some(start)
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

/// `Temporal.ZonedDateTime.prototype.toInstant()`：直读纪元槽构造 Instant，零时区换算。
///
/// # 边界与前提
/// - 纪元槽非 BigInt 值（品牌对象被篡改）→ RangeError。
pub fn zoned_date_time_to_instant<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let Some(epoch_ns) = get_instant_epoch_ns(obj) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid ZonedDateTime"));
    };
    make_instant(vm, epoch_ns)
}

/// `Temporal.ZonedDateTime.prototype.toPlainDate()`：本地日期分量 + 日历槽传播。
///
/// # 边界与前提
/// - 本地分量经 zoned_date_time_plain_parts 换算：时区槽解析失败或日期时间越界 → RangeError。
pub fn zoned_date_time_to_plain_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let (year, month, day, _) = native_try!(zoned_date_time_plain_parts(vm, obj));
    make_plain_date(vm, year, month, day, &get_calendar_id(obj, 2))
}

/// `Temporal.ZonedDateTime.prototype.toPlainDateTime()`：本地全分量 + 日历槽传播。
///
/// # 边界与前提
/// - 本地分量经 zoned_date_time_plain_parts 换算：时区槽解析失败或日期时间越界 → RangeError。
pub fn zoned_date_time_to_plain_date_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let (year, month, day, time_ns) = native_try!(zoned_date_time_plain_parts(vm, obj));
    make_plain_date_time(vm, year, month, day, time_ns, &get_calendar_id(obj, 2))
}

/// `Temporal.ZonedDateTime.prototype.toPlainTime()`：本地时间分量（PlainTime 无日历槽）。
///
/// # 边界与前提
/// - 本地分量经 zoned_date_time_plain_parts 换算：时区槽解析失败或日期时间越界 → RangeError。
pub fn zoned_date_time_to_plain_time<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let (_, _, _, time_ns) = native_try!(zoned_date_time_plain_parts(vm, obj));
    make_plain_time(vm, time_ns)
}

/// `Temporal.ZonedDateTime.prototype.startOfDay()`：当地午夜 ZDT，时区/日历槽保留。
///
/// # 边界与前提
/// - 当地午夜换算委托 start_of_day_epoch_ns（含 Instant 范围校验），越界 → RangeError。
pub fn zoned_date_time_start_of_day<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_zoned_date_time(vm, obj));
    let (year, month, day, _) = native_try!(zoned_date_time_plain_parts(vm, obj));
    let offset_minutes = native_try!(zoned_date_time_offset_minutes(vm, obj));
    let Some(epoch_ns) = start_of_day_epoch_ns(year, month, day, offset_minutes) else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid date"));
    };
    make_zoned_date_time(vm, epoch_ns, &to_string(obj.get_prop_at(1)), &get_calendar_id(obj, 2))
}
