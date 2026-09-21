//! Temporal.PlainYearMonth：ISO 年月解析、构造、getter、日历属性与 with/加减差值。

use oxide_runtime_api::{to_string, NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::common::native_try;
use super::{
    calendar_annotation, compare_iso_date, days_from_civil, days_in_month_iso, difference_core, duration_like_values,
    ensure_plain_year_month, format_iso_year, get_calendar_id, get_double_prop, initialize_temporal_receiver,
    is_ctor_call, is_leap_year_iso, iso_year_month_within_limits, make_duration, make_plain_date,
    make_plain_year_month, month_day_bag_calendar_id, native_engine_error, parse_year_month_difference_settings,
    read_iso2, receiver_obj, reject_partial_object_with_calendar_or_time_zone, temporal_constructor_calendar_id,
    temporal_number_component, temporal_option_string, temporal_option_value, temporal_overflow,
    temporal_to_show_calendar, validate_month_day_annotation_suffix, validate_month_day_time_suffix, ShowCalendar,
    MAX_ISO_DAY,
};

/// 读 PlainYearMonth 的 年/月/参考日 三元组（槽 0-2），含 receiver 校验。
fn plain_year_month_ymd<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(f64, f64, f64), JsValue> {
    let ptr = receiver_obj(vm, args)?;
    let obj = unsafe { &*ptr };
    ensure_plain_year_month(vm, obj)?;
    Ok((get_double_prop(obj, 0), get_double_prop(obj, 1), get_double_prop(obj, 2)))
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

/// PlainYearMonth ISO 串：always/critical 或非 iso8601 日历补参考日与日历注解；
/// iso8601 日历的 auto/never 形保持裸 `±YYYY-MM`。
fn plain_year_month_iso_string(obj: &JsObject, show: ShowCalendar) -> String {
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as i32;
    let calendar = get_calendar_id(obj, 3);
    let with_day = matches!(show, ShowCalendar::Always | ShowCalendar::Critical) || calendar != "iso8601";
    if !with_day {
        return format!("{}-{month:02}", format_iso_year(year as i128));
    }
    let ref_day = get_double_prop(obj, 2) as i32;
    let annotation = calendar_annotation(&calendar, show);
    format!("{}-{month:02}-{ref_day:02}{annotation}", format_iso_year(year as i128))
}

/// PlainYearMonth 默认形串（auto 形），含 receiver 校验，不读 options。
fn plain_year_month_default_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    NativeResult::Ok(vm.new_string_owned(plain_year_month_iso_string(obj, ShowCalendar::Auto)))
}

/// `Temporal.PlainYearMonth.prototype.toString([options])`：
/// 默认 `±YYYY-MM`；非 iso8601 日历补参考日与 `[u-ca=…]` 注解，
/// always/critical 另按注解表补 `[u-ca=…]`/`[!u-ca=…]`。
pub fn plain_year_month_to_string<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let show = native_try!(temporal_to_show_calendar(vm, args));
    NativeResult::Ok(vm.new_string_owned(plain_year_month_iso_string(obj, show)))
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

/// 年月串边界：月 1..=12；日（若有）1..=31 且过语法级月日静态检查
/// （2 月 30/31、30 天月份 31 拒；无参考年，2 月 29 不拒）。
fn finish_year_month_range(year: i32, month: u32, day: Option<u32>) -> Result<(i32, u32, Option<u32>), String> {
    if !(1..=12).contains(&month) {
        return Err("invalid ISO month".into());
    }
    if let Some(day) = day {
        if !(1..=31).contains(&day) || (month == 2 && day > 29) || (matches!(month, 4 | 6 | 9 | 11) && day == 31) {
            return Err("invalid ISO date".into());
        }
    }
    Ok((year, month, day))
}

/// PlainYearMonth 串的日期部分：扩展形（无符号 4 位年或带符号 6 位年）+ -MM[-DD]，
/// 紧凑形（无符号 6/8 位、带符号 8/10 位纯数字）；日部分可缺（年-月形）；
/// 负零年（-000000）拒；返回 (year, month, day)。
fn parse_year_month_date_part(input: &str) -> Result<(i32, u32, Option<u32>), String> {
    if input.is_empty() {
        return Err("invalid ISO date".into());
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
    if negative && digits == "000000" {
        return Err("invalid ISO negative zero year".into());
    }
    if i < bytes.len() && bytes[i] == b'-' {
        // 扩展形：无符号 4 位年或带符号 6 位年，随后 -MM[-DD]。
        let expected = usize::from(signed) * 6 + 4 * (1 - usize::from(signed));
        if digits.len() != expected {
            return Err("invalid ISO year".into());
        }
        let year = digits.parse::<i32>().map_err(|_| "invalid ISO year".to_string())?;
        let year = if negative { -year } else { year };
        i += 1;
        let month = read_iso2(input, &mut i, bytes)?;
        if i >= bytes.len() {
            return finish_year_month_range(year, month, None);
        }
        if bytes[i] != b'-' {
            return Err("invalid trailing content".into());
        }
        i += 1;
        let day = read_iso2(input, &mut i, bytes)?;
        if i != bytes.len() {
            return Err("invalid trailing content".into());
        }
        return finish_year_month_range(year, month, Some(day));
    }
    if i != bytes.len() {
        return Err("invalid trailing content".into());
    }
    // 紧凑纯数字：无符号 6（年+月）/ 8（年+月+日）；带符号 8 / 10（6 位年起步）。
    let len = digits.len();
    if !(matches!(len, 6 | 8) && !signed || matches!(len, 8 | 10) && signed) {
        return Err("invalid ISO date".into());
    }
    let month_start = if signed { 6 } else { 4 };
    let year = digits[..month_start]
        .parse::<i32>()
        .map_err(|_| "invalid ISO year".to_string())?;
    let year = if negative { -year } else { year };
    let month = digits[month_start..month_start + 2]
        .parse::<u32>()
        .map_err(|_| "invalid ISO month".to_string())?;
    let day = if digits.len() > month_start + 2 {
        Some(
            digits[month_start + 2..]
                .parse::<u32>()
                .map_err(|_| "invalid ISO day".to_string())?,
        )
    } else {
        None
    };
    finish_year_month_range(year, month, day)
}

/// 解析 PlainYearMonth ISO 串（年-月 / 年-月-日 / 完整 datetime 形态）。
/// 返回 (year, month)：日部分仅参与语法级校验，参考日恒 1（不入结果）。
fn parse_year_month_string(input: &str) -> Result<(i32, u32), String> {
    let trimmed = input.trim();
    if trimmed.contains('\u{2212}') {
        return Err("variant minus sign is not valid for PlainYearMonth".into());
    }
    let text = trimmed.to_owned();
    let annotation_start = text.find('[').unwrap_or(text.len());
    validate_month_day_annotation_suffix(&text[annotation_start..])?;
    let text = &text[..annotation_start];
    if text.contains('Z') || text.contains('z') {
        return Err("UTC designator is not valid for PlainYearMonth".into());
    }
    let separator = text.find(['T', 't', ' ']);
    let (date_part, time_part) = match separator {
        Some(index) => (&text[..index], &text[index + 1..]),
        None => (text, ""),
    };
    if !time_part.is_empty() {
        validate_month_day_time_suffix(time_part)?;
    }
    let (year, month, _) = parse_year_month_date_part(date_part)?;
    Ok((year, month))
}

/// `Temporal.PlainYearMonth.from(item[, options])`：ISO 串 / PYM 实例 / PD 实例 / 字段对象。
/// 串路径按 (年, 月) 查表示范围（日不参与）；实例分支槽直读（PYM 复制保留参考日，
/// PD 参考日恒 1）；bag 路径 year 必填、day 从不读取（参考日恒 1）、
/// monthCode 先 ToPrimitive 再两段校验（语法先于 year 转换、适配在 year 转换后）。
pub fn plain_year_month_from<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if value.is_string() {
        // 规范顺序：先解析（坏串 RangeError 不触 options），再读 overflow，再查 (年, 月) 范围。
        let (year, month) = native_try!(parse_year_month_string(&to_string(value))
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 year-month string")));
        native_try!(temporal_overflow(vm, args));
        if !iso_year_month_within_limits(year, month) {
            return NativeResult::Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
        }
        return make_plain_year_month(vm, year, month, 1, "iso8601");
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
    if obj.is_plain_year_month_obj() {
        // 实例复制：overflow 先读（坏 options 先抛）后复制槽（参考日/日历保留）。
        native_try!(temporal_overflow(vm, args));
        let calendar = get_calendar_id(obj, 3);
        return make_plain_year_month(
            vm,
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
            &calendar,
        );
    }
    if obj.is_plain_date_obj() {
        // PlainDate 实例：年/月槽直读，参考日恒 1（日不读），日历槽直读。
        native_try!(temporal_overflow(vm, args));
        let year = get_double_prop(obj, 0) as i32;
        let month = get_double_prop(obj, 1) as u32;
        let calendar = get_calendar_id(obj, 3);
        return make_plain_year_month(vm, year, month, 1, &calendar);
    }
    // 字段对象：Get 序 calendar → year → month → monthCode → era → eraYear（day 不读）；
    // overflow 在全部 Get 之后读取。
    let calendar_raw = native_try!(temporal_option_value(vm, obj, value, "calendar"));
    let calendar = native_try!(month_day_bag_calendar_id(vm, calendar_raw));
    let year_raw = native_try!(temporal_option_value(vm, obj, value, "year"));
    let month_raw = native_try!(temporal_option_value(vm, obj, value, "month"));
    let month_code_raw = native_try!(temporal_option_value(vm, obj, value, "monthCode"));
    let era_raw = native_try!(temporal_option_value(vm, obj, value, "era"));
    let era_year_raw = native_try!(temporal_option_value(vm, obj, value, "eraYear"));
    let constrain = native_try!(temporal_overflow(vm, args));
    // era 族：双现 → RangeError；恰一个 → 忽略（ISO 无纪元体系）。
    if !era_raw.is_undefined() && !era_year_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_range_error(vm, "era and eraYear cannot both be present"));
    }
    // 存在性：year 必填（先于 monthCode 语法）；month|monthCode 至少其一。
    if year_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "year is required"));
    }
    if month_raw.is_undefined() && month_code_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "month or monthCode is required"));
    }
    // monthCode ToPrimitive 语义：字符串直用；对象（含函数）经 valueOf/toString；
    // 转换结果非字符串（number/bigint/boolean/null/Symbol 等）→ TypeError。
    let month_code = if month_code_raw.is_undefined() {
        None
    } else if month_code_raw.is_string() {
        Some(to_string(month_code_raw))
    } else if month_code_raw.is_object() {
        let primitive =
            match oxide_runtime_api::to_primitive(month_code_raw, oxide_runtime_api::ToPrimitiveHint::String, vm) {
                Ok(primitive) => primitive,
                Err(error) => return NativeResult::Err(native_engine_error(vm, &error)),
            };
        if !primitive.is_string() {
            return NativeResult::Err(crate::error::create_type_error(vm, "invalid monthCode"));
        }
        Some(to_string(primitive))
    } else {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid monthCode"));
    };
    // monthCode 第一段语法（"M"+两位数字，可选再 +"L"）先于一切数值转换。
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
    let year_raw = native_try!(temporal_number_component(vm, year_raw));
    let year = if year_raw >= i32::MIN as f64 && year_raw <= i32::MAX as f64 {
        year_raw as i32
    } else {
        return NativeResult::Err(crate::error::create_range_error(vm, "invalid year"));
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
    // 数值 month <1 恒 RangeError，>12 时 constrain 钳 12、reject 抛错。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return NativeResult::Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        let f = match month_f {
            Some(v) => v,
            None => return NativeResult::Err(crate::error::create_type_error(vm, "month is required")),
        };
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
    // (年, 月) 表示范围。
    if !iso_year_month_within_limits(year, month) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
    }
    make_plain_year_month(vm, year, month, 1, &calendar)
}

/// ToTemporalYearMonth 四分支共享件（compare 等无 options 参数的成员复用）：
/// 串（解析 + (年, 月) 范围，参考日恒 1）/ PYM 实例（槽直读，参考日保留）/
/// PD 实例（年月槽直读，参考日恒 1）/ 字段 bag（year 必填、day 从不读取、参考日恒 1、
/// monthCode 两段校验同 from）；constrain 决定 month >12 钳制或抛错。
fn year_month_like_parts<H: VmHost>(
    vm: &mut H, value: JsValue, constrain: bool,
) -> Result<(i32, u32, u32, String), JsValue> {
    if value.is_string() {
        let (year, month) = parse_year_month_string(&to_string(value))
            .map_err(|_| crate::error::create_range_error(vm, "invalid ISO 8601 year-month string"))?;
        if !iso_year_month_within_limits(year, month) {
            return Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
        }
        return Ok((year, month, 1, "iso8601".to_string()));
    }
    if !value.is_object() {
        return Err(crate::error::create_type_error(vm, "argument must be a string or an object"));
    }
    let ptr = value.as_js_object_ptr();
    if ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "argument must be a string or an object"));
    }
    let obj = unsafe { &*ptr };
    // 实例快路径：槽直读，不触发属性。
    if obj.is_plain_year_month_obj() {
        return Ok((
            get_double_prop(obj, 0) as i32,
            get_double_prop(obj, 1) as u32,
            get_double_prop(obj, 2) as u32,
            get_calendar_id(obj, 3),
        ));
    }
    if obj.is_plain_date_obj() {
        return Ok((get_double_prop(obj, 0) as i32, get_double_prop(obj, 1) as u32, 1, get_calendar_id(obj, 3)));
    }
    // 字段对象：Get 序 calendar → year → month → monthCode → era → eraYear（day 不读）。
    let calendar_raw = temporal_option_value(vm, obj, value, "calendar")?;
    let calendar = month_day_bag_calendar_id(vm, calendar_raw)?;
    let year_raw = temporal_option_value(vm, obj, value, "year")?;
    let month_raw = temporal_option_value(vm, obj, value, "month")?;
    let month_code_raw = temporal_option_value(vm, obj, value, "monthCode")?;
    let era_raw = temporal_option_value(vm, obj, value, "era")?;
    let era_year_raw = temporal_option_value(vm, obj, value, "eraYear")?;
    // era 族：双现 → RangeError；恰一个 → 忽略（ISO 无纪元体系）。
    if !era_raw.is_undefined() && !era_year_raw.is_undefined() {
        return Err(crate::error::create_range_error(vm, "era and eraYear cannot both be present"));
    }
    // 存在性：year 必填（先于 monthCode 语法）；month|monthCode 至少其一。
    if year_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "year is required"));
    }
    if month_raw.is_undefined() && month_code_raw.is_undefined() {
        return Err(crate::error::create_type_error(vm, "month or monthCode is required"));
    }
    // monthCode ToPrimitive 语义：字符串直用；对象（含函数）经 valueOf/toString；
    // 转换结果非字符串（number/bigint/boolean/null/Symbol 等）→ TypeError。
    let month_code = if month_code_raw.is_undefined() {
        None
    } else if month_code_raw.is_string() {
        Some(to_string(month_code_raw))
    } else if month_code_raw.is_object() {
        let primitive =
            match oxide_runtime_api::to_primitive(month_code_raw, oxide_runtime_api::ToPrimitiveHint::String, vm) {
                Ok(primitive) => primitive,
                Err(error) => return Err(native_engine_error(vm, &error)),
            };
        if !primitive.is_string() {
            return Err(crate::error::create_type_error(vm, "invalid monthCode"));
        }
        Some(to_string(primitive))
    } else {
        return Err(crate::error::create_type_error(vm, "invalid monthCode"));
    };
    // monthCode 第一段语法（"M"+两位数字，可选再 +"L"）先于一切数值转换。
    let (month_code_num, leap_month) = match &month_code {
        Some(text) => {
            let b = text.as_bytes();
            let well_formed = b[0] == b'M'
                && b.len() >= 3
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && (b.len() == 3 || (b.len() == 4 && b[3] == b'L'));
            if !well_formed {
                return Err(crate::error::create_range_error(vm, "invalid monthCode"));
            }
            (Some(u32::from(b[1] - b'0') * 10 + u32::from(b[2] - b'0')), b.len() == 4)
        }
        None => (None, false),
    };
    // year 转换（TypeError/RangeError）在 monthCode 语法之后。
    let year_raw = temporal_number_component(vm, year_raw)?;
    let year = if year_raw >= i32::MIN as f64 && year_raw <= i32::MAX as f64 {
        year_raw as i32
    } else {
        return Err(crate::error::create_range_error(vm, "invalid year"));
    };
    let month_f = match month_raw.is_undefined() {
        true => None,
        false => Some(temporal_number_component(vm, month_raw)?),
    };
    // monthCode 第二段适配：闰月后缀或月值越界 → RangeError。
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：monthCode 与数值 month 并存须一致（冲突 → RangeError）；
    // 数值 month <1 恒 RangeError，>12 时 constrain 钳 12、reject 抛错。
    let month = if let Some(code) = month_code_num {
        if let Some(f) = month_f {
            if f as u32 != code {
                return Err(crate::error::create_range_error(vm, "month and monthCode conflict"));
            }
        }
        code
    } else {
        let f = month_f.ok_or_else(|| crate::error::create_type_error(vm, "month is required"))?;
        if f < 1.0 {
            return Err(crate::error::create_range_error(vm, "invalid month"));
        }
        if f > 12.0 {
            if constrain {
                12
            } else {
                return Err(crate::error::create_range_error(vm, "invalid month"));
            }
        } else {
            f as u32
        }
    };
    // (年, 月) 表示范围。
    if !iso_year_month_within_limits(year, month) {
        return Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
    }
    Ok((year, month, 1, calendar))
}

/// `Temporal.PlainYearMonth.compare(one, two)`：静态比较，返回 -1/0/1。
/// 比较 (年, 月, 参考日) 三元字典序；两参均经 ToTemporalYearMonth。
pub fn plain_year_month_compare<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let a_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let b_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let a = native_try!(year_month_like_parts(vm, a_val, true));
    let b = native_try!(year_month_like_parts(vm, b_val, true));
    let cmp = (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2));
    NativeResult::Ok(JsValue::float(match cmp {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

/// `Temporal.PlainYearMonth.prototype.equals(other)`：比较 (年, 月, 参考日) 三元与日历标识；
/// other 经 ToTemporalYearMonth（串 / 字段对象 / PD 实例 / PYM 实例）转换。
pub fn plain_year_month_equals<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let other = match plain_year_month_from(vm, args) {
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

/// add/subtract 的 (年, 月) 可表示性检查，按日 = 1 计算（ISODateWithinLimits）。
/// 与 ISOYearMonthWithinLimits 的差异仅在 -271821 年：该年 4 月 1 日早于最早
/// 可表示的日期时间，故 4 月及以前越界（5 月起在界内）；+275760 年须 9 月及以前。
fn iso_year_month_day1_within_limits(year: i64, month: i64) -> bool {
    if !(-271_821_i64..=275_760_i64).contains(&year) {
        return false;
    }
    !((year == -271_821 && month < 5) || (year == 275_760 && month > 9))
}

/// `Temporal.PlainYearMonth.prototype.add/subtract(durationLike [, options])` 核心。
///
/// # 步骤
/// 1. receiver branding（TypeError）。
/// 2. ToTemporalDuration（串 / bag / 实例；TypeError、RangeError 各自语义）。
/// 3. 读 overflow（规范顺序先于后续算法校验；ISO 日历下取值不可观测，仅校验）。
/// 4. weeks / days / 时间分量任一非零 → RangeError。
/// 5. receiver (年, 月) 按日 1 检查可表示性 → RangeError。
/// 6. 年、月分量折算绝对月数后相加再平衡（div/rem_euclid）。
/// 7. 结果 (年, 月) 按日 1 检查可表示性 → RangeError。
///
/// # 边界与副作用
/// - 结果参考日恒 1，日历标识取自 receiver 槽 3。
/// - subtract 与 add 共用本函数，sign 参数取反时长分量。
fn plain_year_month_apply_duration<H: VmHost>(vm: &mut H, args: &[u8], sign: i64) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = match duration_like_values(vm, val) {
        Ok(values) => values,
        Err(error) => return NativeResult::Err(error),
    };
    let _overflow = native_try!(temporal_overflow(vm, args));
    if values[2..].iter().any(|value| *value != 0.0) {
        return NativeResult::Err(crate::error::create_range_error(
            vm,
            "year-month cannot add weeks, days, or time units",
        ));
    }
    let year = get_double_prop(obj, 0) as i64;
    let month = get_double_prop(obj, 1) as i64;
    if !iso_year_month_day1_within_limits(year, month) {
        return NativeResult::Err(crate::error::create_range_error(vm, "year-month is out of range"));
    }
    let months = (values[0] as i64) * 12 + values[1] as i64;
    let absolute = year * 12 + month - 1 + sign * months;
    let result_year = absolute.div_euclid(12);
    let result_month = absolute.rem_euclid(12) + 1;
    if !iso_year_month_day1_within_limits(result_year, result_month) {
        return NativeResult::Err(crate::error::create_range_error(vm, "year-month is out of range"));
    }
    make_plain_year_month(vm, result_year as i32, result_month as u32, 1, &get_calendar_id(obj, 3))
}

/// `Temporal.PlainYearMonth.prototype.add(durationLike)`。
pub fn plain_year_month_add<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_apply_duration(vm, args, 1)
}

/// `Temporal.PlainYearMonth.prototype.subtract(durationLike)`。
pub fn plain_year_month_subtract<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_apply_duration(vm, args, -1)
}

/// `Temporal.PlainYearMonth.prototype.with(yearMonthLike[, options])`：覆盖 year/month/monthCode；
/// undefined 不覆盖；calendar/timeZone 键拒绝；无识别字段 → TypeError。
///
/// # 步骤
/// 1. receiver branding（TypeError）。
/// 2. reject calendar/timeZone 键与 Temporal 实例（TypeError）。
/// 3. Get 序 month → monthCode → year（字典序）；全 undefined → TypeError。
/// 4. monthCode 第一段语法校验与字段数值转换先于 options 读取。
/// 5. 读 overflow（缺省 constrain）；monthCode 第二段适配（闰月后缀/1..12）与
///    month/monthCode 冲突、>12 钳制在此。
/// 6. 缺省分量取 receiver 槽（年 0 / 月 1）；(年, 月) 过 ISOYearMonthWithinLimits。
///
/// # 边界与副作用
/// - 结果参考日恒 1，日历标识取自 receiver 槽 3。
/// - 年月级范围检查（非按日 1），-271821-04 可经 with 产出。
pub fn plain_year_month_with<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let like = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    native_try!(reject_partial_object_with_calendar_or_time_zone(vm, like));
    let lptr = like.as_js_object_ptr();
    if lptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let like_obj = unsafe { &*lptr };
    // Get 序（字典序）：month → monthCode → year；无识别字段 → TypeError。
    let month_raw = native_try!(temporal_option_value(vm, like_obj, like, "month"));
    let month_code_raw = native_try!(temporal_option_value(vm, like_obj, like, "monthCode"));
    let month_code = match month_code_raw.is_undefined() {
        true => None,
        false => Some(native_try!(temporal_option_string(vm, month_code_raw))),
    };
    let year_raw = native_try!(temporal_option_value(vm, like_obj, like, "year"));
    if [month_raw, month_code_raw, year_raw].iter().all(|raw| raw.is_undefined()) {
        return NativeResult::Err(crate::error::create_type_error(vm, "no properties present"));
    }
    // monthCode 第一段语法（"M"+两位数字，可选再 +"L"）先于一切数值转换。
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
    if let Some(f) = month_f {
        if f < 1.0 {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid month"));
        }
    }
    let constrain = native_try!(temporal_overflow(vm, args));
    if let Some(code) = month_code_num {
        if leap_month || !(1..=12).contains(&code) {
            return NativeResult::Err(crate::error::create_range_error(vm, "invalid monthCode"));
        }
    }
    // 月定值：monthCode 与数值 month 并存须一致（冲突 → RangeError）；
    // 数值 month >12 时 constrain 钳 12、reject 抛错；缺省取 receiver 月（槽 1）。
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
            None => get_double_prop(obj, 1) as u32,
        }
    };
    let year = year.unwrap_or_else(|| get_double_prop(obj, 0) as i32);
    // (年, 月) 可表示范围（年月级，非按日 1）。
    if !iso_year_month_within_limits(year, month) {
        return NativeResult::Err(crate::error::create_range_error(vm, "ISO year-month is out of range"));
    }
    let calendar = get_calendar_id(obj, 3);
    make_plain_year_month(vm, year, month, 1, &calendar)
}

/// `Temporal.PlainYearMonth.prototype.toPlainDate(dayLike)`：dayLike 须为对象，
/// 只 Get `day`（year/month/monthCode/calendar 从不读取）；options.overflow 绝不读取；
/// 合并 (年, 月, 日) 后 constrain 钳 day 至 [1, 月长]，day 级可表示范围检查后
/// 产出 PlainDate（receiver 日历）。
pub fn plain_year_month_to_plain_date<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let item = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !item.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let iptr = item.as_js_object_ptr();
    if iptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "invalid argument"));
    }
    let item_obj = unsafe { &*iptr };
    let day_raw = native_try!(temporal_option_value(vm, item_obj, item, "day"));
    if day_raw.is_undefined() {
        return NativeResult::Err(crate::error::create_type_error(vm, "day is required"));
    }
    let day = native_try!(temporal_number_component(vm, day_raw));
    let year = get_double_prop(obj, 0) as i32;
    let month = get_double_prop(obj, 1) as u32;
    // constrain：day 钳到 [1, 月长]。
    let day = (day as i32).clamp(1, days_in_month_iso(year, month) as i32) as u32;
    // day 级表示范围：-100_000_001..=100_000_000（年月级合法而 day 级越界在此抛）。
    let day_count = days_from_civil(i128::from(year), i128::from(month), i128::from(day));
    if !(-100_000_001..=100_000_000).contains(&day_count) {
        return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
    }
    let calendar = get_calendar_id(obj, 3);
    make_plain_date(vm, year, month, day, &calendar)
}

/// `Temporal.PlainYearMonth.prototype.until(other[, options])`。
pub fn plain_year_month_until<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_difference(vm, args, false)
}

/// `Temporal.PlainYearMonth.prototype.since(other[, options])`。
pub fn plain_year_month_since<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    plain_year_month_difference(vm, args, true)
}

/// until/since 差值核心（DifferenceTemporalPlainYearMonth）。
///
/// # 步骤
/// 1. receiver branding（TypeError）。
/// 2. other 经 ToTemporalYearMonth（串 / bag / PD / PYM 实例；day 不读）。
/// 3. 日历相等（不等 RangeError；在 options 读取之前）。
/// 4. options 对象 + 差值设置（单位表仅 year/month，smallestUnit 缺省 month，
///    largestUnit 缺省/auto 为 year）。
/// 5. (年, 月, 参考日) 三元全等 → 空 Duration（先于可表示性检查）。
/// 6. 两端按日 1 查可表示性（ISODateWithinLimits，day 级）→ RangeError。
/// 7. 既有差值机件（nudge 窗口端点同样带 day 级范围检查）；since 整体取反。
///
/// # 边界与副作用
/// - 参考日只参与第 5 步的全等判断；差值本身恒按日 1 计算。
/// - 第 2 步的字符串/袋路径为年月级范围检查（自 ToTemporalYearMonth），
///   day 级补查在第 6 步，两步错误类相同（RangeError），仅抛错时点不同。
fn plain_year_month_difference<H: VmHost>(vm: &mut H, args: &[u8], since: bool) -> NativeResult {
    let ptr = native_try!(receiver_obj(vm, args));
    let obj = unsafe { &*ptr };
    native_try!(ensure_plain_year_month(vm, obj));
    let other_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (oy, om, od, ocal) = native_try!(year_month_like_parts(vm, other_val, true));
    // 日历相等（ToTemporalYearMonth 之后、options 读取之前）。
    if get_calendar_id(obj, 3) != ocal {
        return NativeResult::Err(crate::error::create_range_error(vm, "calendars must be equal"));
    }
    let options_value = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let settings = match parse_year_month_difference_settings(vm, options_value) {
        Ok(settings) => settings,
        Err(error) => return NativeResult::Err(error),
    };
    // (年, 月, 参考日) 三元全等 → 空 Duration（先于可表示性检查）。
    let ry = get_double_prop(obj, 0) as i128;
    let rm = get_double_prop(obj, 1) as i128;
    let rd = get_double_prop(obj, 2) as i128;
    if compare_iso_date((ry, rm, rd), (i128::from(oy), i128::from(om), i128::from(od))) == 0 {
        return make_duration(vm, [0.0; 10]);
    }
    // 两端按日 1 的可表示性检查（ISODateWithinLimits，day 级）。
    if days_from_civil(ry, rm, 1).abs() > MAX_ISO_DAY
        || days_from_civil(i128::from(oy), i128::from(om), 1).abs() > MAX_ISO_DAY
    {
        return NativeResult::Err(crate::error::create_range_error(vm, "date is out of range"));
    }
    difference_core(vm, (ry, rm, 1), 0, (i128::from(oy), i128::from(om), 1), 0, settings, since, 1)
}
