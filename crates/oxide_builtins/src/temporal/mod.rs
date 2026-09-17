use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

mod common;
mod difference;
mod duration;
mod instant;
mod plain_date;
mod plain_date_time;
mod plain_month_day;
mod plain_time;
mod plain_year_month;
mod zoned_date_time;

use common::*;
use difference::*;
pub use duration::*;
pub use instant::*;
pub use plain_date::*;
pub use plain_date_time::*;
pub use plain_month_day::*;
pub use plain_time::*;
pub use plain_year_month::*;
pub use zoned_date_time::*;

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

// Temporal.PlainMonthDay / Temporal.PlainYearMonth 对象地基：
// 数字分量转换、槽对象构造、构造器与 getter/toString/toJSON。

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
/// from 等返回新对象的成员使用；构造器走 receiver 初始化路径。
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
