//! Temporal.PlainMonthDay：ISO 月日解析、构造、getter 与 with/toPlainDate。

use oxide_runtime_api::{to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::common::native_try;
use super::{
    calendar_annotation, days_from_civil, days_in_month_iso, ensure_plain_month_day, format_iso_year, get_calendar_id,
    get_double_prop, initialize_temporal_receiver, is_ctor_call, make_plain_date, make_plain_month_day,
    month_day_bag_calendar_id, read_iso2, receiver_obj, reject_partial_object_with_calendar_or_time_zone,
    strip_iso_offset, temporal_calendar_id_strict, temporal_number_component, temporal_option_string,
    temporal_option_value, temporal_overflow, temporal_to_show_calendar, valid_iso_date, valid_plain_time,
    ShowCalendar,
};

/// 读 PlainMonthDay 的 月/日/参考年 三元组（槽 0-2），含 receiver 校验。
fn plain_month_day_mdy<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(f64, f64, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_month_day(vm, obj)?;
    Ok((get_double_prop(obj, 0), get_double_prop(obj, 1), get_double_prop(obj, 2)))
}

/// 构造器日历参数解析（ToTemporalCalendarSlotValue，缺省 "iso8601"）：
/// undefined → 缺省；字符串 → 严格白名单（非法 RangeError）；Temporal 日期实例 →
/// 日历槽直读（不触发属性 getter）；函数对象 → 缺省（无日历行为）；
/// 其他对象与原始值 → TypeError。
pub(crate) fn temporal_constructor_calendar_id<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
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

/// PlainMonthDay ISO 串：always/critical 或非 iso8601 日历补参考年与日历注解；
/// iso8601 日历的 auto/never 形保持裸 `MM-DD`。
fn plain_month_day_iso_string(obj: &JsObject, show: ShowCalendar) -> String {
    let month = get_double_prop(obj, 0) as i32;
    let day = get_double_prop(obj, 1) as i32;
    let calendar = get_calendar_id(obj, 3);
    let with_year = matches!(show, ShowCalendar::Always | ShowCalendar::Critical) || calendar != "iso8601";
    if !with_year {
        return format!("{month:02}-{day:02}");
    }
    let ref_year = get_double_prop(obj, 2) as i32;
    let annotation = calendar_annotation(&calendar, show);
    format!("{}-{month:02}-{day:02}{annotation}", format_iso_year(ref_year as i128))
}

/// PlainMonthDay 默认形串（auto 形），含 receiver 校验，不读 options。
fn plain_month_day_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    NativeResult::Ok(vm.new_string_owned(plain_month_day_iso_string(obj, ShowCalendar::Auto)))
}

/// `Temporal.PlainMonthDay.prototype.toString([options])`：
/// 默认 `MM-DD`；非 iso8601 日历补参考年与 `[u-ca=…]` 注解，
/// always/critical 另按注解表补 `[u-ca=…]`/`[!u-ca=…]`。
pub fn plain_month_day_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_month_day(vm, obj));
    let show = native_try!(temporal_to_show_calendar(vm, args));
    NativeResult::Ok(vm.new_string_owned(plain_month_day_iso_string(obj, show)))
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

// ==================== PlainMonthDay.from ====================

/// PlainMonthDay 串的注解后缀校验：后缀语法与 validate_temporal_annotation_suffix
/// 一致，但 u-ca 注解只接受 iso8601（大小写不敏感），其余内置日历 ID 一律拒绝。
pub(crate) fn validate_month_day_annotation_suffix(mut suffix: &str) -> Result<(), String> {
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
pub(crate) fn validate_month_day_time_suffix(input: &str) -> Result<(), String> {
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
    // monthCode 两段校验：语法（"M"+两位数字，可选 +"L"）先于 year 转换；
    // 适配（闰月后缀、1..12 范围）与冲突检查在 options 读取之后。
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
    // 部分字段先于 options 完整转换（RangeError/TypeError 先于 options 形态错误）。
    let year = match year_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, year_raw)) as i32),
    };
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, month_raw))),
    };
    let day_f = match day_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_number_component(vm, day_raw))),
    };
    if let Some(f) = month_f {
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
        }
    }
    if let Some(f) = day_f {
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid day"));
        }
    }
    let constrain = native_try!(temporal_overflow(vm, args));
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
    // 日：receiver 日（槽 1）为缺省（partial 日已先行转换并过 <1 检查）。
    let day_f = day_f.unwrap_or_else(|| get_double_prop(obj, 1));
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
