//! Temporal 命名空间的最小实现子集：Temporal.Now / Temporal.Instant /
//! Temporal.PlainDate / Temporal.PlainTime / Temporal.PlainDateTime /
//! Temporal.PlainMonthDay / Temporal.PlainYearMonth / Temporal.ZonedDateTime。
//! 内部数据按对象类型存入 prop 槽：
//! Instant 存纪元纳秒（BigInt，prop 0）、PlainDate 存年/月/日/日历 ID（prop 0-3）、
//! PlainTime 存午夜后纳秒（f64，prop 0）、PlainDateTime 存年/月/日/午夜后纳秒/日历 ID（prop 0-4）、
//! PlainMonthDay 存月/日/参考年/日历 ID（prop 0-3）、
//! PlainYearMonth 存年/月/参考日/日历 ID（prop 0-3）、
//! ZonedDateTime 存纪元纳秒、时区 ID、日历 ID（prop 0-2）。
//! 日历 ID 未显式给定时统一取 "iso8601"。

use num_traits::ToPrimitive;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::{
    days_in_month, parse_temporal_calendar_string, temporal_calendar_id, valid_iso_date, valid_plain_date_time_range,
    valid_plain_time,
};

pub(crate) const MAX_INSTANT_NS: i128 = 8_640_000_000_000_000_000_000;

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}
pub(crate) use native_try;

pub(crate) fn get_double_prop(obj: &JsObject, pos: usize) -> f64 {
    let v = obj.get_prop_at(pos);
    if v.is_double() {
        v.as_double()
    } else {
        f64::NAN
    }
}

/// 读对象日历槽：字符串槽直接返回，undefined/非 string 兜底 `"iso8601"`。
/// 只读 prop 槽，不触发属性 getter，getter 与日历传播共用。
pub(crate) fn get_calendar_id(obj: &JsObject, slot: usize) -> String {
    let value = obj.get_prop_at(slot);
    if value.is_string() {
        to_string(value)
    } else {
        "iso8601".to_string()
    }
}

pub(crate) fn get_instant_epoch_ns(obj: &JsObject) -> Option<i128> {
    let value = obj.get_prop_at(0);
    if value.is_bigint() {
        return Some(unsafe { oxide_runtime_api::bigint_data(value) }.to_i128().unwrap_or(i128::MAX));
    }
    if value.is_int() || value.is_double() {
        return Some(to_number(value) as i128);
    }
    None
}

pub(crate) fn ensure_instant<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_instant_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

pub(crate) fn ensure_plain_date<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_date_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

pub(crate) fn ensure_plain_time<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_time_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

pub(crate) fn ensure_plain_date_time<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_date_time_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

pub(crate) fn ensure_duration<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_duration_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

pub(crate) fn ensure_zoned_date_time<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_zoned_date_time_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

pub(crate) fn receiver_obj<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<*mut JsObject, JsValue> {
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

pub(crate) fn initialize_temporal_receiver<H: VmHost, const N: usize>(
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
pub(crate) fn is_ctor_call<H: VmHost>(vm: &mut H, args: &[u8], proto_ptr: *const JsObject) -> bool {
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

pub(crate) fn make_instant<H: VmHost>(vm: &mut H, epoch_ns: i128) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().instant_proto.as_ptr() as *mut JsObject);
    let epoch_value = vm.new_bigint(num_bigint::BigInt::from(epoch_ns));
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_INSTANT;
    obj.set_prop_at(0, epoch_value);
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

pub(crate) fn make_zoned_date_time<H: VmHost>(
    vm: &mut H, epoch_ns: i128, time_zone_id: &str, calendar: &str,
) -> NativeResult {
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

pub(crate) fn native_engine_error<H: VmHost>(vm: &mut H, error: &str) -> JsValue {
    vm.take_uncaught_value()
        .unwrap_or_else(|| crate::error::create_from_text(vm, error))
}

pub(crate) fn temporal_option_value<H: VmHost>(
    vm: &mut H, obj: &JsObject, receiver: JsValue, name: &str,
) -> Result<JsValue, JsValue> {
    let key = vm.new_string(name);
    let si = vm.property_key_si(key);
    vm.ordinary_get(obj, si, receiver)
        .map_err(|error| native_engine_error(vm, &error))
}

pub(crate) fn temporal_option_number<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
    let primitive = oxide_runtime_api::to_primitive(value, oxide_runtime_api::ToPrimitiveHint::Number, vm)
        .map_err(|error| native_engine_error(vm, &error))?;
    if primitive.is_symbol() || primitive.is_bigint() {
        return Err(crate::error::create_type_error(vm, "cannot convert option to number"));
    }
    Ok(to_number(primitive))
}

pub(crate) fn temporal_option_string<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
    let primitive = oxide_runtime_api::to_primitive(value, oxide_runtime_api::ToPrimitiveHint::String, vm)
        .map_err(|error| native_engine_error(vm, &error))?;
    if primitive.is_symbol() {
        return Err(crate::error::create_type_error(vm, "cannot convert option to string"));
    }
    Ok(to_string(primitive))
}

/// ToPrimitive(String) 后要求结果为字符串，否则 TypeError（ParseMonthCode/ToOffsetString 语义）。
pub(crate) fn temporal_string_strict<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
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
pub(crate) fn reject_partial_object_with_calendar_or_time_zone<H: VmHost>(
    vm: &mut H, value: JsValue,
) -> Result<(), JsValue> {
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

pub(crate) fn plain_date_time_object_parts<H: VmHost>(
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

pub(crate) fn temporal_overflow<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<bool, JsValue> {
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

/// Temporal 数字分量转换（number-only 路径）：先 ToPrimitive(Number)——对象经
/// valueOf/toString 取值；Symbol/BigInt → TypeError；NaN/±Inf → RangeError
/// （undefined 与不可解析串都归 NaN，由此统一抛 RangeError）；其余截断取整。
pub(crate) fn temporal_number_component<H: VmHost>(vm: &mut H, value: JsValue) -> Result<f64, JsValue> {
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
/// 年超 ±271821..=275760、或 -271821 年 3 月及以前、+275760 年 10 月及以后越界。
pub(crate) fn iso_year_month_within_limits(year: i32, month: u32) -> bool {
    if !(-271_821..=275_760).contains(&year) {
        return false;
    }
    !((year == -271_821 && month < 4) || (year == 275_760 && month > 9))
}

/// toString 的 calendarName 选项取值（缺省 = auto；auto 与 never 的差异仅在注解）。
pub(crate) enum ShowCalendar {
    Auto,
    Always,
    Critical,
    Never,
}

/// ISO 串尾部日历注解（FormatCalendarAnnotation）：never 与 auto+iso8601 为空；
/// critical 前置 `!`；其余为 `[u-ca=…]`。
pub(crate) fn calendar_annotation(calendar: &str, show: ShowCalendar) -> String {
    match (show, calendar) {
        (ShowCalendar::Never, _) => String::new(),
        (ShowCalendar::Auto, "iso8601") => String::new(),
        (ShowCalendar::Always, _) => format!("[u-ca={calendar}]"),
        (ShowCalendar::Critical, _) => format!("[!u-ca={calendar}]"),
        (ShowCalendar::Auto, _) => format!("[u-ca={calendar}]"),
    }
}

/// 读并校验 toString 的 calendarName 选项：options 非对象 → TypeError；
/// 值非字符串先 ToPrimitive(String) 转换（Symbol → TypeError）；
/// 仅 auto/always/never/critical 合法（大小写敏感），否则 RangeError。
pub(crate) fn temporal_to_show_calendar<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<ShowCalendar, JsValue> {
    let options = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if options.is_undefined() {
        return Ok(ShowCalendar::Auto);
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
        return Ok(ShowCalendar::Auto);
    }
    let value = temporal_option_string(vm, raw)?;
    match value.as_str() {
        "always" => Ok(ShowCalendar::Always),
        "critical" => Ok(ShowCalendar::Critical),
        "never" => Ok(ShowCalendar::Never),
        "auto" => Ok(ShowCalendar::Auto),
        _ => Err(crate::error::create_range_error(vm, "invalid calendarName")),
    }
}

/// bag 日历解析：undefined → iso8601；字符串走宽松日历串解析（18-ID 白名单归一小写，
/// 合法 ISO 日期-时间串归一 iso8601，含 YYYY-MM / MM-DD 部分日期）；
/// PD/PDT/ZDT/PMD/PYM 实例 → 直读日历槽；其余对象与原始值 → TypeError。
pub(crate) fn month_day_bag_calendar_id<H: VmHost>(vm: &mut H, value: JsValue) -> Result<String, JsValue> {
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
