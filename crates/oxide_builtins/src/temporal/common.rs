//! Temporal 命名空间对象的存储约定：各类型实例的载荷按固定 prop 槽（对象
//! 内置属性表的下标位置）写入，槽外不设隐藏字段。
//!
//! 槽位布局：
//! - Temporal.Instant：prop 0 存纪元纳秒（BigInt）。
//! - Temporal.Duration：prop 0-9 依次存年、月、周、日、时、分、秒、毫秒、
//!   微秒、纳秒（均为 f64）。
//! - Temporal.PlainDate：prop 0-3 存年、月、日、日历 ID。
//! - Temporal.PlainTime：prop 0 存午夜后纳秒（f64）。
//! - Temporal.PlainDateTime：prop 0-4 存年、月、日、午夜后纳秒、日历 ID。
//! - Temporal.PlainMonthDay：prop 0-3 存月、日、参考年、日历 ID。
//! - Temporal.PlainYearMonth：prop 0-3 存年、月、参考日、日历 ID。
//! - Temporal.ZonedDateTime：prop 0-2 存纪元纳秒、时区 ID、日历 ID。
//!
//! 日历 ID 未显式给定时统一取 "iso8601"。

use num_traits::ToPrimitive;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_number, to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::{
    instant_time_zone_offset, parse_iso_date, parse_plain_date_time_string, parse_plain_time_string, valid_iso_date,
    valid_plain_date_time_range, valid_plain_time,
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
        let n = to_number(value);
        // f64 转 i128：超大值饱和到 i128 边界。
        if n.is_finite() && n >= i128::MIN as f64 && n <= i128::MAX as f64 {
            return Some(n as i128);
        }
        return Some(i128::MAX);
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

/// 校验 `with` 族方法的 partial 参数并排除日历/时区载体。
///
/// 非对象、已初始化的 Temporal 日期时间类实例，或 calendar、timeZone 任一
/// 非 undefined 的对象都抛 TypeError（RejectObjectWithCalendarOrTimeZone 语义）。
///
/// # 边界与前提
/// - 内部类型标签检查先于 calendar、timeZone 属性的读取，两步的先后不可交换。
/// - 只有本函数返回成功，调用方才能继续读取 partial 的字段。
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
    // 分量值可能超出目标整型范围（f64 截断后仍超 i32/u32 上限），先做有界转换。
    // 超出范围后 valid_iso_date / valid_plain_time 会拒绝，此处兜住 debug 模式 panic。
    let year = if year.is_finite() && year >= i32::MIN as f64 && year <= i32::MAX as f64 {
        year as i32
    } else {
        return Err(crate::error::create_range_error(vm, "invalid date-time component"));
    };
    let mut to_u32 = |v: f64| -> Result<u32, JsValue> {
        if v.is_finite() && v >= 0.0 && v <= u32::MAX as f64 {
            Ok(v as u32)
        } else {
            Err(crate::error::create_range_error(vm, "invalid date-time component"))
        }
    };
    let month = to_u32(month)?;
    let day = to_u32(day)?;
    let hour = to_u32(hour)?;
    let minute = to_u32(minute)?;
    let second = to_u32(second)?;
    let millisecond = to_u32(millisecond)?;
    let microsecond = to_u32(microsecond)?;
    let nanosecond = to_u32(nanosecond)?;
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

pub(crate) fn parse_digits(bytes: &[u8], cursor: &mut usize, count: usize) -> Option<i128> {
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

pub(crate) fn is_leap_year(year: i128) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

pub(crate) fn days_in_month(year: i128, month: i128) -> Option<i128> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 => Some(if is_leap_year(year) { 29 } else { 28 }),
        _ => None,
    }
}

pub(crate) fn days_from_civil(mut year: i128, month: i128, day: i128) -> i128 {
    year -= i128::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(crate) enum FractionalSecondDigitsInput {
    Auto,
    Number(f64),
    String(String),
}

pub(crate) fn parse_fractional_second_digits<H: VmHost>(
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

pub(crate) fn parse_offset_minutes(value: &str) -> Option<i32> {
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

pub(crate) fn canonical_time_zone(value: &str) -> Option<(String, i32)> {
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

pub(crate) fn civil_from_days(days: i128) -> (i128, i128, i128) {
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

pub(crate) fn format_iso_year(year: i128) -> String {
    if (0..=9_999).contains(&year) {
        format!("{year:04}")
    } else if year >= 0 {
        format!("+{year:06}")
    } else {
        format!("-{:06}", -year)
    }
}

pub(crate) fn validate_temporal_annotation_suffix(mut suffix: &str) -> Result<(), String> {
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
pub(crate) fn strip_iso_offset(input: &str) -> Result<&str, String> {
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

/// ParseTemporalDateString：PlainDate 字符串（时间部分可选且被忽略，仅按日期做范围校验）。
pub(crate) fn parse_plain_date_string(input: &str) -> Result<(i32, u32, u32), String> {
    parse_temporal_string_impl(input, false).map(|(year, month, day, _)| (year, month, day))
}

pub(crate) fn parse_temporal_string_impl(
    input: &str, enforce_date_time_range: bool,
) -> Result<(i32, u32, u32, f64), String> {
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
pub(crate) const CALENDAR_ID_WHITELIST: [&str; 18] = [
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
pub(crate) fn is_builtin_calendar_id(value: &str) -> Option<&'static str> {
    CALENDAR_ID_WHITELIST.iter().find(|id| value.eq_ignore_ascii_case(id)).copied()
}

/// 严格日历 ID 解析：只走 18 项白名单，拒绝 ISO 串（含注解/compact/extended/空串）。
/// 供构造器日历参数与注解值判定使用。
pub(crate) fn parse_temporal_calendar_id_strict(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    is_builtin_calendar_id(trimmed)
        .map(str::to_string)
        .ok_or_else(|| "invalid calendar".to_string())
}

/// ParseTemporalCalendarString：日历标识符 = 18 项内置日历 ID（ASCII 大小写不敏感）
/// 或合法 ISO 日期(-时间)字符串（含部分日期 YYYY-MM / MM-DD，可选时间、偏移、注解）。
/// 返回规范化日历 ID：白名单命中返回规范小写，ISO 串路径恒为 "iso8601"。
pub(crate) fn parse_temporal_calendar_string(input: &str) -> Result<String, String> {
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
pub(crate) fn parse_partial_calendar_date(input: &str) -> Result<(), String> {
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
pub(crate) fn temporal_calendar_id<H: VmHost>(vm: &mut H, value: JsValue) -> Result<Option<String>, JsValue> {
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
pub(crate) fn temporal_calendar_id_strict<H: VmHost>(vm: &mut H, value: JsValue) -> Result<Option<String>, JsValue> {
    if value.is_string() {
        return parse_temporal_calendar_id_strict(&to_string(value))
            .map(Some)
            .map_err(|_| crate::error::create_range_error(vm, "invalid calendar"));
    }
    temporal_calendar_id(vm, value)
}

/// 将 ZonedDateTime 按时区偏移转换为本地 PlainDateTime 分量。
pub(crate) fn zoned_date_time_plain_parts<H: VmHost>(
    vm: &mut H, obj: &JsObject,
) -> Result<(i32, u32, u32, f64), JsValue> {
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

/// 本地墙钟分量 + 时区偏移（分钟）→ 纪元纳秒（zoned_date_time_plain_parts 的逆）。
///
/// # 边界与前提
/// - (year, month, day, time_ns) 须已通过 valid_iso_date / valid_plain_time 校验（调用方保证）。
/// - 仅做 checked 溢出防护，Instant 范围校验由调用方按需执行。
pub(crate) fn local_to_epoch_ns(year: i32, month: u32, day: u32, time_ns: f64, offset_minutes: i32) -> Option<i128> {
    let days = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    days.checked_mul(86_400_000_000_000)?
        .checked_add(time_ns as i128)?
        .checked_sub(i128::from(offset_minutes) * 60_000_000_000)
}

pub(crate) fn make_plain_date<H: VmHost>(vm: &mut H, year: i32, month: u32, day: u32, calendar: &str) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_date_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_DATE;
    obj.set_prop_at(0, JsValue::float(year as f64));
    obj.set_prop_at(1, JsValue::float(month as f64));
    obj.set_prop_at(2, JsValue::float(day as f64));
    obj.set_prop_at(3, vm.new_string(calendar));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

pub(crate) fn make_plain_time<H: VmHost>(vm: &mut H, total_ns: f64) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_time_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_TIME;
    obj.set_prop_at(0, JsValue::float(total_ns));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

pub(crate) fn make_plain_date_time<H: VmHost>(
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

pub(crate) fn make_duration<H: VmHost>(vm: &mut H, values: [f64; 10]) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().duration_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_DURATION;
    for (index, value) in values.into_iter().enumerate() {
        obj.set_prop_at(index, JsValue::float(value));
    }
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

// Temporal.PlainMonthDay / Temporal.PlainYearMonth 对象地基：
// 数字分量转换、槽对象构造、构造器与 getter/toString/toJSON。

/// 构造 PlainMonthDay 实例对象（槽 0-3 = 月/日/参考年/日历 ID）。
/// from/toPlainDate 等返回新对象的成员使用；构造器走 receiver 初始化路径。
pub(crate) fn make_plain_month_day<H: VmHost>(
    vm: &mut H, month: u32, day: u32, ref_year: i32, calendar: &str,
) -> NativeResult {
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
/// from 等返回新对象的成员使用；构造器走 receiver 初始化路径。
pub(crate) fn make_plain_year_month<H: VmHost>(
    vm: &mut H, year: i32, month: u32, ref_day: u32, calendar: &str,
) -> NativeResult {
    let proto = JsValue::from_js_object(vm.session().builtin_world().plain_year_month_proto.as_ptr() as *mut JsObject);
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto);
    obj.type_tag = JsObject::OBJ_TYPE_PLAIN_YEAR_MONTH;
    obj.set_prop_at(0, JsValue::float(year as f64));
    obj.set_prop_at(1, JsValue::float(month as f64));
    obj.set_prop_at(2, JsValue::float(ref_day as f64));
    obj.set_prop_at(3, vm.new_string(calendar));
    NativeResult::Ok(JsValue::from_js_object(vm.alloc_object(obj)))
}

pub(crate) fn ensure_plain_month_day<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_month_day_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}

pub(crate) fn ensure_plain_year_month<H: VmHost>(vm: &mut H, obj: &JsObject) -> Result<(), JsValue> {
    if !obj.is_plain_year_month_obj() {
        return Err(crate::error::create_type_error(vm, "called on incompatible receiver"));
    }
    Ok(())
}
