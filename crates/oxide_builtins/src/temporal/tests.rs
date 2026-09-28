use super::*;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

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
    assert_eq!(start_of_day_epoch_ns(-271_821, 4, 19, -3600), None);
    // -100000000 天（-271821-04-20）UTC 当地午夜恰为 -MAX，在边界内。
    assert_eq!(start_of_day_epoch_ns(-271_821, 4, 20, 0), Some(-8_640_000_000_000_000_000_000));
    // 同日期 +1h 偏移使当地午夜 -MAX - 1h 越界。
    assert_eq!(start_of_day_epoch_ns(-271_821, 4, 20, 3600), None);
    // 明日边界越界：+100000001 天（UTC）。
    assert_eq!(start_of_day_epoch_ns_by_days(100_000_001, 0), None);
    // 正常日期返回当地午夜。
    assert_eq!(start_of_day_epoch_ns(1970, 1, 1, 0), Some(0));
    assert_eq!(start_of_day_epoch_ns(1970, 1, 1, 3600), Some(-3_600_000_000_000));
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
    let base = format_zoned_date_time_iso(0, 3600, "+01:00", "iso8601", true, None, "auto", "auto", "auto");
    assert_eq!(base, Some("1970-01-01T01:00:00+01:00[+01:00]".to_string()));
    // offset never 省略偏移段，时区注解仍显示。
    let no_offset = format_zoned_date_time_iso(0, 3600, "+01:00", "iso8601", true, None, "never", "auto", "auto");
    assert_eq!(no_offset, Some("1970-01-01T01:00:00[+01:00]".to_string()));
    // calendarName always 追加日历注解。
    let ca_always = format_zoned_date_time_iso(0, 3600, "+01:00", "iso8601", true, None, "auto", "auto", "always");
    assert_eq!(ca_always, Some("1970-01-01T01:00:00+01:00[+01:00][u-ca=iso8601]".to_string()));
    // timeZoneName/calendarName critical 均 `!` 置于括号内。
    let critical = format_zoned_date_time_iso(0, 3600, "+01:00", "iso8601", true, None, "auto", "critical", "critical");
    assert_eq!(critical, Some("1970-01-01T01:00:00+01:00[!+01:00][!u-ca=iso8601]".to_string()));
    // offset critical 偏移段前加 !。
    let offset_critical =
        format_zoned_date_time_iso(0, 3600, "+01:00", "iso8601", true, None, "critical", "auto", "auto");
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
    assert_eq!(
        parse_instant_string("2024-01-01T00:00:00+01:00"),
        local_to_epoch_ns(2024, 1, 1, 0.0, 3600),
    );
    assert_eq!(
        parse_instant_string("1969-07-16T13:32:01.234567891Z"),
        local_to_epoch_ns(1969, 7, 16, 48_721_234_567_891.0, 0),
    );
    // startOfDay 最小边界：-271821-04-20 加 1h 再回推 1h 偏移回到 -MAX。
    assert_eq!(
        local_to_epoch_ns(-271821, 4, 20, 3_600_000_000_000.0, 3600),
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
    // ±HHMM 无冒号形式归一：ID 保留原串（偏移秒值由 fixed_offset_seconds 单独提供）。
    assert_eq!(canonical_time_zone("+0000"), Some("+0000".to_string()));
    assert_eq!(canonical_time_zone("-0530"), Some("-0530".to_string()));
    assert_eq!(canonical_time_zone("+2330"), Some("+2330".to_string()));
    // 非法分钟/小时拒绝。
    assert_eq!(canonical_time_zone("+2400"), None);
    assert_eq!(canonical_time_zone("+0060"), None);
    assert_eq!(canonical_time_zone("+123"), None); // 长度不符
}

#[test]
fn offset_seconds_representation_sub_minute() {
    // i64 秒表示保留亚分钟偏移（±HH:MM:SS 秒位），旧分钟表示丢弃秒位；
    // 第二分量标记秒分量在场（亚分钟精度，比较口径切精确一致）。
    assert_eq!(parse_any_offset_seconds("+00:44:30"), Some((2670, true)));
    assert_eq!(parse_any_offset_seconds("-00:44:30"), Some((-2670, true)));
    assert_eq!(parse_any_offset_seconds("+01:00:00"), Some((3600, true)));
    assert_eq!(parse_any_offset_seconds("-23:59:59"), Some((-(23 * 3600 + 59 * 60 + 59), true)));
    assert_eq!(parse_any_offset_seconds("+00:45"), Some((2700, false)));
    assert_eq!(parse_any_offset_seconds("-00:45:00"), Some((-2700, true)));
    // 固定偏移秒值与分钟值 ×60 一致（±HH:MM 六字符形）。
    assert_eq!(canonical_time_zone("+01:00"), Some("+01:00".to_string()));
    assert_eq!(canonical_time_zone("-05:30"), Some("-05:30".to_string()));
    // 亚分钟偏移经 local_to_epoch_ns 与 parse_instant_string 互逆对拍。
    assert_eq!(
        parse_instant_string("2024-01-01T00:00:00-00:44:30"),
        local_to_epoch_ns(2024, 1, 1, 0.0, -2670),
    );
    assert_eq!(
        parse_instant_string("2024-01-01T00:00:00+00:44:30"),
        local_to_epoch_ns(2024, 1, 1, 0.0, 2670),
    );
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
    // 本地墙钟日回推：offset 3600 秒，local_to_epoch_ns 得目标 epoch。
    let epoch = local_to_epoch_ns(1976, 11, 18, rounded as f64, 3600).unwrap();
    assert_eq!(epoch, 217_177_200_000_000_000);
}

#[test]
fn zoned_date_time_round_day_path_epoch() {
    // 同日本地午夜 startNs（2513 天，offset 3600 秒），dayProgress 舍到次日 → 217206000000000000。
    let start_ns = start_of_day_epoch_ns(1976, 11, 18, 3600).unwrap();
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

#[test]
fn canonical_time_zone_iana_acceptance() {
    // IANA 区名（含 legacy 别名）解到规范名；非法区名拒绝。
    assert_eq!(canonical_time_zone("America/New_York"), Some("America/New_York".to_string()));
    assert_eq!(canonical_time_zone("Asia/Calcutta"), Some("Asia/Kolkata".to_string()));
    assert_eq!(canonical_time_zone("Etc/Ignored"), Some("Etc/UTC".to_string()));
    assert_eq!(canonical_time_zone("Not/AZone"), None);
    // UTC / 数值偏移臂不变（只返回规范名，不再返回偏移）。
    assert_eq!(canonical_time_zone("UTC"), Some("UTC".to_string()));
    assert_eq!(canonical_time_zone("Z"), Some("UTC".to_string()));
    assert_eq!(canonical_time_zone("+01:00"), Some("+01:00".to_string()));
    // 注解串：IANA 区注解解到规范名。
    assert_eq!(canonical_time_zone("2024-01-01T00:00:00[UTC]"), Some("UTC".to_string()));
    assert_eq!(
        canonical_time_zone("2024-01-01T00:00:00[America/New_York]"),
        Some("America/New_York".to_string())
    );
}

#[test]
fn zone_offset_seconds_fixed_and_iana() {
    // 固定区退化情形：偏移与 epoch 无关，恒返常量（含亚分钟偏移）。
    assert_eq!(zone_offset_seconds("+05:30", 0), Some(19800));
    assert_eq!(zone_offset_seconds("-00:44:30", 1_000_000_000), Some(-2670));
    assert_eq!(zone_offset_seconds("UTC", 0), Some(0));
    // IANA 区：1970 前后两点（EST / EDT 边界 2000-04-02T07:00Z，纽约本地 02:00）。
    assert_eq!(zone_offset_seconds("America/New_York", 0), Some(-18000));
    assert_eq!(zone_offset_seconds("America/New_York", 954_658_800), Some(-14400));
    assert_eq!(zone_offset_seconds("America/New_York", 954_658_800 - 1), Some(-18000));
    // 非法区名返回 None。
    assert_eq!(zone_offset_seconds("Not/AZone", 0), None);
}

#[test]
fn sub_minute_offset_getter_branch() {
    // Monrovia -00:44:30：四格式化面补秒段；整分钟偏移不补。
    assert_eq!(format_offset_seconds_text(-2670), "-00:44:30");
    assert_eq!(format_offset_seconds_text(2670), "+00:44:30");
    assert_eq!(format_offset_seconds_text(3600), "+01:00");
    assert_eq!(format_time_zone_annotation("-00:44:30", false), "[-00:44:30]");
    let iso = format_zoned_date_time_iso(0, -2670, "-00:44:30", "iso8601", true, None, "auto", "auto", "auto");
    assert_eq!(iso, Some("1969-12-31T23:15:30-00:44:30[-00:44:30]".to_string()));
    assert_eq!(
        format_instant_iso(0, Some(-2670), true, None),
        Some("1969-12-31T23:15:30-00:44:30".to_string())
    );
}

#[test]
fn round_offset_to_minutes_half_expand() {
    // 半值远离零：正负两侧对称，整分钟偏移不变；半分钟边界（±30 秒）向上舍入。
    assert_eq!(round_offset_to_minutes(2700), 2700);
    assert_eq!(round_offset_to_minutes(-2700), -2700);
    assert_eq!(round_offset_to_minutes(2670), 2700); // +00:44:30 → +00:45
    assert_eq!(round_offset_to_minutes(-2670), -2700); // -00:44:30 → -00:45
    assert_eq!(round_offset_to_minutes(2640), 2640);
    assert_eq!(round_offset_to_minutes(2669), 2640); // 差 1 秒不到半分钟，仍舍入到 +00:44
    assert_eq!(round_offset_to_minutes(-2669), -2640);
    assert_eq!(round_offset_to_minutes(0), 0);
}

#[test]
fn offset_matches_exact_and_minutes() {
    // 精确口径：秒级相等才成立。
    assert!(offset_matches(2700, 2700, true));
    assert!(!offset_matches(2670, 2700, true));
    assert!(!offset_matches(2670, 2669, true));
    // 分钟口径：zone 偏移舍入到分钟后与输入相等；精确相等是其子集。
    assert!(offset_matches(2700, 2700, false));
    assert!(offset_matches(2670, 2700, false)); // zone 亚分钟 -00:44:30 舍入后接受分钟输入
    assert!(offset_matches(-2670, -2700, false));
    assert!(!offset_matches(2670, 2640, false)); // 舍入到 2700，与 2640 不符
    assert!(!offset_matches(0, 19800, false)); // UTC 区对 +05:30 输入
}

#[test]
fn zone_aware_string_offset_vs_zone() {
    // zone-aware 创建解析（纯函数层）：注解经 canonical_time_zone 得规范名，
    // 候选 epoch 点 zone 偏移经 zone_offset_seconds 读取，offset-vs-zone 判定走 offset_matches。
    // 固定偏移区退化情形：zone 偏移恒为常量，分钟输入精确一致。
    let tz = canonical_time_zone("+05:30").unwrap();
    let zone = zone_offset_seconds(&tz, 0).unwrap();
    let (offset, has_sub) = parse_any_offset_seconds("+05:30").unwrap();
    assert_eq!(offset, 19800);
    assert!(!has_sub);
    assert!(offset_matches(zone, offset, has_sub));
    // 亚分钟输入走精确口径：zone 偏移 -00:44:30 与输入 -00:44:30 秒级相等。
    let (offset, has_sub) = parse_any_offset_seconds("-00:44:30").unwrap();
    assert!(has_sub);
    assert_eq!(offset, -2670);
    assert!(offset_matches(-2670, offset, has_sub));
    // IANA 区亚分钟历史偏移：Monrovia 1970 年 zone 偏移 -00:44:30，
    // 分钟输入 -00:45 经舍入口径接受，亚分钟输入 -00:44:30 精确一致、-00:44:31 精确口径拒绝。
    let zone = zone_offset_seconds("Africa/Monrovia", 0).unwrap();
    assert_eq!(zone, -2670);
    let (minute_input, has_sub) = parse_any_offset_seconds("-00:45").unwrap();
    assert!(!has_sub);
    assert!(offset_matches(zone, minute_input, has_sub));
    let (sub_input, has_sub) = parse_any_offset_seconds("-00:44:30").unwrap();
    assert!(has_sub);
    assert!(offset_matches(zone, sub_input, has_sub));
    let (sub_input, has_sub) = parse_any_offset_seconds("-00:44:31").unwrap();
    assert!(has_sub);
    assert!(!offset_matches(zone, sub_input, has_sub));
    // 注解提取：IANA 区注解命中，key 注解（u-ca）跳过，亚分钟偏移注解拒作时区标识符。
    assert_eq!(
        extract_time_zone_annotation("2000-01-01T00:00[Africa/Monrovia]"),
        Some(("Africa/Monrovia".to_string(), false))
    );
    assert_eq!(extract_time_zone_annotation("2000-01-01T00:00[u-ca=iso8601]"), None);
    assert_eq!(extract_time_zone_annotation("2000-01-01T00:00[-00:44:30]"), None);
}

#[test]
fn add_zoned_disambiguation_four_quadrants() {
    use super::zoned_date_time::{disambiguate_possible_epoch_nanoseconds, get_possible_epoch_nanoseconds};

    // NY 重叠 2025-11-02T01:30：双候选 [05:30Z, 06:30Z] 升序。
    let cands = get_possible_epoch_nanoseconds("America/New_York", 2025, 11, 2, 5_400_000_000_000).unwrap();
    assert_eq!(cands, vec![1_762_061_400_000_000_000, 1_762_065_000_000_000_000]);
    let iso = (2025_i128, 11, 2);
    let time_ns = 5_400_000_000_000;
    assert_eq!(
        disambiguate_possible_epoch_nanoseconds("America/New_York", iso, time_ns, "earlier", &cands),
        Ok(1_762_061_400_000_000_000)
    );
    assert_eq!(
        disambiguate_possible_epoch_nanoseconds("America/New_York", iso, time_ns, "later", &cands),
        Ok(1_762_065_000_000_000_000)
    );
    assert_eq!(
        disambiguate_possible_epoch_nanoseconds("America/New_York", iso, time_ns, "compatible", &cands),
        Ok(1_762_061_400_000_000_000)
    );
    assert!(disambiguate_possible_epoch_nanoseconds("America/New_York", iso, time_ns, "reject", &cands).is_err());

    // NY 间隙 2025-03-09T02:30：零候选，compatible 平移 +1h 取 07:30Z，earlier 取 06:30Z。
    let empty = get_possible_epoch_nanoseconds("America/New_York", 2025, 3, 9, 9_000_000_000_000).unwrap();
    assert!(empty.is_empty());
    let iso = (2025_i128, 3, 9);
    let time_ns = 9_000_000_000_000;
    assert_eq!(
        disambiguate_possible_epoch_nanoseconds("America/New_York", iso, time_ns, "compatible", &empty),
        Ok(1_741_505_400_000_000_000)
    );
    assert_eq!(
        disambiguate_possible_epoch_nanoseconds("America/New_York", iso, time_ns, "earlier", &empty),
        Ok(1_741_501_800_000_000_000)
    );
    assert_eq!(
        disambiguate_possible_epoch_nanoseconds("America/New_York", iso, time_ns, "later", &empty),
        Ok(1_741_505_400_000_000_000)
    );
    assert!(disambiguate_possible_epoch_nanoseconds("America/New_York", iso, time_ns, "reject", &empty).is_err());

    // NY 无歧义 2025-06-15T12:00（EDT）：单候选 16:00Z。
    let cands = get_possible_epoch_nanoseconds("America/New_York", 2025, 6, 15, 43_200_000_000_000).unwrap();
    assert_eq!(cands, vec![1_750_003_200_000_000_000]);

    // Apia 24 小时间隙 2011-12-30T12:00：零候选，compatible 平移 +24h 取 2011-12-30T22:00Z，
    // earlier 取 2011-12-29T22:00Z。
    let empty = get_possible_epoch_nanoseconds("Pacific/Apia", 2011, 12, 30, 43_200_000_000_000).unwrap();
    assert!(empty.is_empty());
    let iso = (2011_i128, 12, 30);
    let time_ns = 43_200_000_000_000;
    assert_eq!(
        disambiguate_possible_epoch_nanoseconds("Pacific/Apia", iso, time_ns, "compatible", &empty),
        Ok(1_325_282_400_000_000_000)
    );
    assert_eq!(
        disambiguate_possible_epoch_nanoseconds("Pacific/Apia", iso, time_ns, "earlier", &empty),
        Ok(1_325_196_000_000_000_000)
    );
}

#[test]
fn add_zoned_datetime_pure_end_to_end() {
    use super::common::MAX_INSTANT_NS;
    use super::zoned_date_time::{add_zoned_datetime_pure, AddZonedError};

    // 快速路径（UTC）：无日期分量，时间部分 epoch 级加。
    let values = [0.0_f64, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    assert_eq!(add_zoned_datetime_pure(0, "UTC", "iso8601", &values, true), Ok(7_200_000_000_000));

    // 固定偏移区 +01:00：+1 天保持本地时刻不变（epoch 前进整日）。
    let values = [0.0_f64, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    assert_eq!(add_zoned_datetime_pure(0, "+01:00", "iso8601", &values, true), Ok(86_400_000_000_000));

    // NY 跨 DST 结束（2024-11-02T06:00-04:00 加 1 月 → 12-02T06:00 EST，epoch 11:00Z）。
    let values = [0.0_f64, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    assert_eq!(
        add_zoned_datetime_pure(1_730_541_600_000_000_000, "America/New_York", "iso8601", &values, true),
        Ok(1_733_137_200_000_000_000)
    );

    // Apia 跨 24 小时间隙减 1 天：2011-12-31T00:00+14:00 减 1 天落间隙起点 2011-12-30T00:00，
    // compatible 平移 +24h 回 2011-12-31T00:00，epoch 落回 transition 瞬间（与输入同刻）。
    let values = [0.0_f64, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    assert_eq!(
        add_zoned_datetime_pure(1_325_239_200_000_000_000, "Pacific/Apia", "iso8601", &values, true),
        Ok(1_325_239_200_000_000_000)
    );

    // 下边界：-MAX instant 再加 -1 天，墙历越出 PlainDateTime 范围（引擎的 instant 与
    // plain 范围同为 10^8 天量级，ISO 日边界检查先于 instant 检查触发），与旧路径的
    // 中间日期检查同为 RangeError「invalid date-time」。
    let values = [0.0_f64, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    assert_eq!(
        add_zoned_datetime_pure(-MAX_INSTANT_NS, "UTC", "iso8601", &values, true),
        Err(AddZonedError::OutOfDateTimeRange)
    );

    // 上边界：+MAX instant 快速路径加 1 小时溢出。
    let values = [0.0_f64, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    assert_eq!(
        add_zoned_datetime_pure(MAX_INSTANT_NS, "UTC", "iso8601", &values, true),
        Err(AddZonedError::OutOfInstantRange)
    );
}

#[test]
fn calendar_date_add_iso_constrain_semantics() {
    use super::zoned_date_time::{calendar_date_add_iso, AddZonedError};

    // 月末截断：constrain 钳到目标月末，reject 抛 DayOutOfRange。
    assert_eq!(calendar_date_add_iso((2025, 1, 31), 0, 1, 0, 0, true), Ok((2025, 2, 28)));
    assert_eq!(calendar_date_add_iso((2025, 1, 31), 0, 1, 0, 0, false), Err(AddZonedError::DayOutOfRange));
    // 闰年二月：constrain 钳到 29。
    assert_eq!(calendar_date_add_iso((2024, 1, 31), 0, 1, 0, 0, true), Ok((2024, 2, 29)));
}

#[test]
fn zoned_difference_day_correction() {
    use super::zoned_date_time::{difference_zoned_datetime, get_epoch_nanos_for};

    // 同日纯时间差：无日期部分，时间部分为纳秒差。
    let ns1 = get_epoch_nanos_for("America/New_York", (2025, 6, 15), 43_200_000_000_000).unwrap();
    let ns2 = get_epoch_nanos_for("America/New_York", (2025, 6, 15), 66_600_000_000_000).unwrap();
    let d = difference_zoned_datetime(ns1, ns2, "America/New_York", "iso8601", 3).unwrap();
    assert_eq!(d.date, [0.0, 0.0, 0.0, 0.0]);
    assert_eq!(d.time_ns, 23_400_000_000_000);

    // 跨 DST 间隙日（23 小时）：墙历时刻相同，日期部分 1 天、时间部分 0。
    let ns1 = get_epoch_nanos_for("America/New_York", (2025, 3, 8), 43_200_000_000_000).unwrap();
    let ns2 = get_epoch_nanos_for("America/New_York", (2025, 3, 9), 43_200_000_000_000).unwrap();
    let d = difference_zoned_datetime(ns1, ns2, "America/New_York", "iso8601", 3).unwrap();
    assert_eq!(d.date, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(d.time_ns, 0);

    // dayCorrection 循环推进：12 小时跨间隙日，日期部分 0、时间部分 12 小时。
    let ns2b = get_epoch_nanos_for("America/New_York", (2025, 3, 9), 0).unwrap();
    let d = difference_zoned_datetime(ns1, ns2b, "America/New_York", "iso8601", 3).unwrap();
    assert_eq!(d.date, [0.0, 0.0, 0.0, 0.0]);
    assert_eq!(d.time_ns, 43_200_000_000_000);

    // 反向（sign 为正）：日期部分为负一天、时间部分 0。
    let d = difference_zoned_datetime(ns2, ns1, "America/New_York", "iso8601", 3).unwrap();
    assert_eq!(d.date, [0.0, 0.0, 0.0, -1.0]);
    assert_eq!(d.time_ns, 0);
}

#[test]
fn zoned_total_dst_fraction() {
    use super::zoned_date_time::{difference_zoned_datetime_with_total, get_epoch_nanos_for};

    // 整日差（23 小时间隙日）：总量 1。
    let ns1 = get_epoch_nanos_for("America/New_York", (2025, 3, 8), 43_200_000_000_000).unwrap();
    let ns2 = get_epoch_nanos_for("America/New_York", (2025, 3, 9), 43_200_000_000_000).unwrap();
    let total = difference_zoned_datetime_with_total(ns1, ns2, "America/New_York", "iso8601", 3).unwrap();
    assert!((total - 1.0).abs() < 1e-12);

    // 12 小时跨 23 小时间隙日：总量 12/23（按实际日长折算，非 24 小时）。
    let ns2b = get_epoch_nanos_for("America/New_York", (2025, 3, 9), 0).unwrap();
    let total = difference_zoned_datetime_with_total(ns1, ns2b, "America/New_York", "iso8601", 3).unwrap();
    assert!((total - 12.0 / 23.0).abs() < 1e-12);
}

#[test]
fn zoned_round_nudge_to_zoned_time() {
    use super::duration::internal_duration_to_values;
    use super::instant::InstantRoundingMode;
    use super::zoned_date_time::{
        add_zoned_datetime_pure, difference_zoned_datetime_with_rounding, get_epoch_nanos_for,
    };

    // Apia 2011-12-30 整日跳过（24 小时间隙）：起点墙历为 12-31 00:30。
    let origin = get_epoch_nanos_for("Pacific/Apia", (2011, 12, 30), 1_800_000_000_000).unwrap();

    // P25H 按小时取整：跨间隙日不产生进位，保持 25 小时（非 1 天 1 小时）。
    let p25h = [0.0_f64, 0.0, 0.0, 0.0, 25.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let target = add_zoned_datetime_pure(origin, "Pacific/Apia", "iso8601", &p25h, true).unwrap();
    let internal = difference_zoned_datetime_with_rounding(
        origin,
        target,
        "Pacific/Apia",
        "iso8601",
        4,
        1,
        4,
        InstantRoundingMode::Trunc,
    )
    .unwrap();
    let values = internal_duration_to_values(&internal, 4);
    assert_eq!(values, [0.0, 0.0, 0.0, 0.0, 25.0, 0.0, 0.0, 0.0, 0.0, 0.0]);

    // P1DT26H 按小时取整：日长 24 小时，取整后 2 天 2 小时。
    let p1dt26h = [0.0_f64, 0.0, 0.0, 1.0, 26.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let target = add_zoned_datetime_pure(origin, "Pacific/Apia", "iso8601", &p1dt26h, true).unwrap();
    let internal = difference_zoned_datetime_with_rounding(
        origin,
        target,
        "Pacific/Apia",
        "iso8601",
        3,
        1,
        4,
        InstantRoundingMode::Trunc,
    )
    .unwrap();
    let values = internal_duration_to_values(&internal, 4);
    assert_eq!(values, [0.0, 0.0, 0.0, 2.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
}

#[test]
fn zoned_compare_pure_epoch() {
    use super::zoned_date_time::{add_zoned_datetime_pure, get_epoch_nanos_for};

    // 跨 23 小时间隙日：P1D（实际 23 小时）短于 P24H（实际 24 小时）。
    let origin = get_epoch_nanos_for("America/New_York", (2025, 3, 8), 43_200_000_000_000).unwrap();
    let p1d = [0.0_f64, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let p24h = [0.0_f64, 0.0, 0.0, 0.0, 24.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let after1d = add_zoned_datetime_pure(origin, "America/New_York", "iso8601", &p1d, true).unwrap();
    let after24h = add_zoned_datetime_pure(origin, "America/New_York", "iso8601", &p24h, true).unwrap();
    assert!(after1d < after24h);
}
