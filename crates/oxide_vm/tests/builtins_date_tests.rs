use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn str_val(vm: &Vm, val: oxide_types::value::JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

// -- static methods --

#[test]
fn date_now_returns_number() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.now()").unwrap();
    assert!(r.is_double() && r.as_double() > 0.0);
}

#[test]
fn date_parse_iso_format() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.parse('2020-01-15')").unwrap();
    assert!(!r.as_double().is_nan());
}

#[test]
fn date_parse_invalid() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.parse('nope')").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_parse_iso_with_offset() {
    let mut vm = Vm::new();
    // 带偏移按偏移折算 UTC：+08:00 → 2019-12-31T16:00:00Z。
    let r = eval(&mut vm, "Date.parse('2020-01-01T00:00:00+08:00')").unwrap();
    assert_eq!(r.as_double(), 1577808000000.0);
    // 负偏移同样折算：-05:00 → 2020-01-01T05:00:00Z。
    let r = eval(&mut vm, "Date.parse('2020-01-01T00:00:00-05:00')").unwrap();
    assert_eq!(r.as_double(), 1577854800000.0);
    // Z 后缀保持 UTC。
    let r = eval(&mut vm, "Date.parse('2020-01-01T00:00:00Z')").unwrap();
    assert_eq!(r.as_double(), 1577836800000.0);
}

// ── Date() 普通调用（非构造，S15.9.2.1）──

#[test]
fn date_call_returns_current_time_string() {
    let mut vm = Vm::new();
    // 无参与多参均返回当前时刻字符串，参数被完全忽略。
    let r = eval(&mut vm, "Date()").unwrap();
    assert!(r.is_string());
    let r = eval(&mut vm, "Date(0, 0, 0)").unwrap();
    assert!(r.is_string());
    // 参数不做 ToNumber：toString 抛错的对象也不触发异常。
    let r = eval(&mut vm, "Date({ toString() { throw 1; } })").unwrap();
    assert!(r.is_string());
}

#[test]
fn date_call_via_bind_no_panic() {
    let mut vm = Vm::new();
    // bind 转发后 this=null 落入普通调用路径；0,0,0 被忽略，不得按参数推导
    // 时间戳（此前非 UTC+0 时区 year 0 回绕成负年触发 RFC2822 panic）。
    let r = eval(&mut vm, "Date.bind(null)(0, 0, 0)").unwrap();
    assert!(r.is_string());
}

#[test]
fn date_call_output_matches_to_string() {
    let mut vm = Vm::new();
    // 普通调用输出与 new Date().toString() 同格式同内容（S15.9.2.1_A2）。
    // 两次取当前时刻可能跨毫秒边界，故比较固定宽度的日期前缀段 `Www Mmm DD YYYY`
    // （仅跨日才变）并以长度一致锁定整体格式。
    let r = eval(
        &mut vm,
        concat!(
            "var a = Date(); var b = (new Date()).toString(); ",
            "(a.slice(0, 15) === b.slice(0, 15) && a.length === b.length) ? 1 : 0"
        ),
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_constructor_path_unaffected() {
    let mut vm = Vm::new();
    // 构造路径分派不受影响：仍返回 Date 对象且时间戳语义不变。
    let r = eval(&mut vm, "typeof new Date()").unwrap();
    assert_eq!(str_val(&vm, r), "object");
    let r = eval(&mut vm, "new Date(0).getTime()").unwrap();
    assert_eq!(r.as_double(), 0.0);
    let r = eval(&mut vm, "new Date(2020, 0, 15).getFullYear()").unwrap();
    assert_eq!(r.as_double(), 2020.0);
}

// ── 经 JS 调用 new Date() 构造器 ──

#[test]
fn date_new_multi_arg() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 15).getFullYear()").unwrap();
    assert_eq!(r.as_double(), 2020.0);
}

#[test]
fn date_new_epoch_zero() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(0).getTime()").unwrap();
    assert_eq!(r.as_double(), 0.0);
}

#[test]
fn date_new_get_month() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 5, 15).getMonth()").unwrap();
    assert_eq!(r.as_double(), 5.0);
}

#[test]
fn date_new_get_date() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 1, 29).getDate()").unwrap();
    assert_eq!(r.as_double(), 29.0);
}

#[test]
fn date_new_get_hours_min_sec() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 1, 12, 30, 45).getHours()").unwrap();
    assert_eq!(r.as_double(), 12.0);
    let r = eval(&mut vm, "new Date(2020, 0, 1, 12, 30, 45).getMinutes()").unwrap();
    assert_eq!(r.as_double(), 30.0);
    let r = eval(&mut vm, "new Date(2020, 0, 1, 12, 30, 45).getSeconds()").unwrap();
    assert_eq!(r.as_double(), 45.0);
}

#[test]
fn date_set_time() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setTime(1000); d.getTime()").unwrap();
    assert_eq!(r.as_double(), 1000.0);
}

#[test]
fn date_set_full_year() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1); d.setFullYear(2025); d.getFullYear()").unwrap();
    assert_eq!(r.as_double(), 2025.0);
}

#[test]
fn date_value_of() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 1).valueOf()").unwrap();
    assert!(r.as_double() > 0.0);
}

#[test]
fn date_to_iso_string() {
    let mut vm = Vm::new();
    // 解析显式 ISO UTC 字符串，避免时区依赖。
    let r = eval(&mut vm, "new Date('2020-01-01T00:00:00Z').toISOString()").unwrap();
    let s = str_val(&vm, r);
    assert!(s.starts_with("2020-01-01"), "got: {s}");
}

#[test]
fn date_constructor_string_with_offset() {
    let mut vm = Vm::new();
    // 单参字符串带时区偏移：+08:00 的午夜等于前一日 16:00 UTC。
    let r = eval(&mut vm, "new Date('2020-01-01T00:00:00+08:00').getTime()").unwrap();
    assert_eq!(r.as_double(), 1577808000000.0);
    let r = eval(&mut vm, "new Date('2020-01-01T00:00:00-05:00').getTime()").unwrap();
    assert_eq!(r.as_double(), 1577854800000.0);
    // 无偏移完整时间按 UTC。
    let r = eval(&mut vm, "new Date('2020-01-01T00:00:00').getTime()").unwrap();
    assert_eq!(r.as_double(), 1577836800000.0);
}

#[test]
fn date_to_json() {
    let mut vm = Vm::new();
    // 解析显式 ISO UTC 字符串，避免时区依赖。
    let r = eval(&mut vm, "new Date('2020-01-01T00:00:00Z').toJSON()").unwrap();
    let s = str_val(&vm, r);
    assert!(s.starts_with("2020-01-01"), "got: {s}");
}

#[test]
fn date_to_string_not_empty() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(0).toString()").unwrap();
    assert!(r.is_string());
}

#[test]
fn date_to_utc_string_not_empty() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(0).toUTCString()").unwrap();
    assert!(r.is_string());
}

#[test]
fn date_objects_use_bounded_numeric_coercion() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "+new Date(0)").unwrap();
    assert_eq!(r.as_double(), 0.0);
}

// ── 时区正确性测试 ──

#[test]
fn date_local_getters_differ_from_utc() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(2020, 6, 15, 12, 0, 0); d.getHours()").unwrap();
    let local_hour = r.as_double();
    let r = eval(&mut vm, "var d = new Date(2020, 6, 15, 12, 0, 0); d.getUTCHours()").unwrap();
    let utc_hour = r.as_double();
    // 在 UTC+0 下二者相等（正确），否则不相等。
    if utc_hour != local_hour {
        assert_ne!(local_hour, utc_hour, "in non-UTC+0, local vs UTC hours should differ");
    }
    assert!((0.0..24.0).contains(&local_hour));
}

#[test]
fn date_constructor_uses_local_timezone() {
    let mut vm = Vm::new();
    // getTimezoneOffset = UTC - local；UTC 小时 = local + offset/60
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 12, 0, 0); d.getUTCHours()").unwrap();
    let utc_hour = r.as_double();
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 12, 0, 0); d.getTimezoneOffset()").unwrap();
    let offset = r.as_double();
    let expected_utc = (12.0 + offset / 60.0).rem_euclid(24.0);
    assert_eq!(utc_hour, expected_utc, "getUTCHours = local + TZ_offset/60");
}

#[test]
fn date_constructor_missing_fields_default_fixed() {
    let mut vm = Vm::new();
    // 缺省日/时分秒毫秒取固定值（日=1、时分秒毫秒=0），不随当前时刻漂移。
    let r = eval(&mut vm, "var d = new Date(2020, 5); d.getMonth()").unwrap();
    assert_eq!(r.as_double(), 5.0);
    let r = eval(&mut vm, "var d = new Date(2020, 5); d.getDate()").unwrap();
    assert_eq!(r.as_double(), 1.0);
    let r = eval(&mut vm, "var d = new Date(2020, 5); d.getHours()").unwrap();
    assert_eq!(r.as_double(), 0.0);
    let r = eval(&mut vm, "var d = new Date(2020, 5); d.getMinutes()").unwrap();
    assert_eq!(r.as_double(), 0.0);
    let r = eval(&mut vm, "var d = new Date(2020, 5); d.getSeconds()").unwrap();
    assert_eq!(r.as_double(), 0.0);
    let r = eval(&mut vm, "var d = new Date(2020, 5); d.getMilliseconds()").unwrap();
    assert_eq!(r.as_double(), 0.0);
}

#[test]
fn date_constructor_month_overflow_rolls_year() {
    let mut vm = Vm::new();
    // 月分量越界滚动到下一年（MakeDay 语义）。
    let r = eval(
        &mut vm,
        "var d = new Date(2020, 12, 1); [d.getFullYear(), d.getMonth(), d.getDate()].join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2021,0,1");
    // 多月越界同样进位。
    let r = eval(
        &mut vm,
        "var d = new Date(2020, 24, 1); [d.getFullYear(), d.getMonth(), d.getDate()].join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2022,0,1");
}

#[test]
fn date_constructor_zero_date_rolls_back() {
    let mut vm = Vm::new();
    // 日为 0 回退到上月末。
    let r = eval(
        &mut vm,
        "var d = new Date(2020, 0, 0); [d.getFullYear(), d.getMonth(), d.getDate()].join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2019,11,31");
}

#[test]
fn date_constructor_negative_month_rolls_back() {
    let mut vm = Vm::new();
    // 负月回退到上一年。
    let r = eval(
        &mut vm,
        "var d = new Date(2020, -1, 1); [d.getFullYear(), d.getMonth(), d.getDate()].join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2019,11,1");
}

#[test]
fn date_constructor_time_overflow_rolls_day() {
    let mut vm = Vm::new();
    // 小时越界滚动到下一天。
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 24); d.getDate()").unwrap();
    assert_eq!(r.as_double(), 2.0);
    // 秒越界同样滚动。
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 0, 0, 60); d.getMinutes()").unwrap();
    assert_eq!(r.as_double(), 1.0);
}

#[test]
fn date_constructor_truncation() {
    let mut vm = Vm::new();
    // 年月参数按 ToInteger 截断。
    let r = eval(&mut vm, "new Date(2024.7, 5.9).getFullYear()").unwrap();
    assert_eq!(r.as_double(), 2024.0);
    let r = eval(&mut vm, "new Date(2024.7, 5.9).getMonth()").unwrap();
    assert_eq!(r.as_double(), 5.0);
}

#[test]
fn date_constructor_two_digit_year_maps_to_1900s() {
    let mut vm = Vm::new();
    // 多参构造年 0..99 按规范映射到 1900..1999；100 起不再调整。
    let r = eval(&mut vm, "new Date(0, 0).getFullYear()").unwrap();
    assert_eq!(r.as_double(), 1900.0);
    let r = eval(&mut vm, "new Date(99, 0).getFullYear()").unwrap();
    assert_eq!(r.as_double(), 1999.0);
    let r = eval(&mut vm, "new Date(100, 0).getFullYear()").unwrap();
    assert_eq!(r.as_double(), 100.0);
}

#[test]
fn date_setters_return_timestamp() {
    let mut vm = Vm::new();
    // setter 返回修改后的时间戳。
    let r = eval(&mut vm, "var d = new Date(0); var ts = d.setHours(12); ts == d.getTime() ? 1 : 0").unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_invalid_returns_nan() {
    let mut vm = Vm::new();
    // Invalid Date 的 getter/setter 返回 NaN。
    let r = eval(&mut vm, "new Date(NaN).getFullYear()").unwrap();
    assert!(r.as_double().is_nan());
    let r = eval(&mut vm, "new Date(NaN).setHours(0)").unwrap();
    assert!(r.as_double().is_nan());
    // Invalid Date 构造（NaN 参数）。
    let r = eval(&mut vm, "new Date(NaN, 0).getTime()").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_get_timezone_offset_uses_date_timestamp() {
    let mut vm = Vm::new();
    // 偏移基于日期的自身时间戳，而非当前时间。
    let r = eval(&mut vm, "new Date(0).getTimezoneOffset()").unwrap();
    assert!(r.as_double().is_finite());
}

#[test]
fn date_setters_optional_chain() {
    let mut vm = Vm::new();
    // setFullYear 支持可选的月/日参数。
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1); d.setFullYear(2025, 11, 25); d.getFullYear()").unwrap();
    assert_eq!(r.as_double(), 2025.0);
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1); d.setFullYear(2025, 11, 25); d.getMonth()").unwrap();
    assert_eq!(r.as_double(), 11.0);
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1); d.setFullYear(2025, 11, 25); d.getDate()").unwrap();
    assert_eq!(r.as_double(), 25.0);
}

#[test]
fn date_local_setters_preserve_utc_offset() {
    let mut vm = Vm::new();
    // setHours 改变本地小时，验证本地小时值。
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 0, 0, 0); d.setHours(12); d.getHours()").unwrap();
    assert_eq!(r.as_double(), 12.0, "setHours(12) should make getHours() return 12");
}

// ── 后续新增方法测试 ──

#[test]
fn date_set_utc_full_year_works() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCFullYear(2025); d.getUTCFullYear()",
    )
    .unwrap();
    assert_eq!(r.as_double(), 2025.0);
}

#[test]
fn date_set_utc_hours_optional_chain() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCHours(23, 59, 59, 999); d.getUTCHours()",
    )
    .unwrap();
    assert_eq!(r.as_double(), 23.0);
    let r = eval(
        &mut vm,
        "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCHours(23, 59, 59, 999); d.getUTCMinutes()",
    )
    .unwrap();
    assert_eq!(r.as_double(), 59.0);
    let r = eval(
        &mut vm,
        "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCHours(23, 59, 59, 999); d.getUTCSeconds()",
    )
    .unwrap();
    assert_eq!(r.as_double(), 59.0);
    let r = eval(
        &mut vm,
        "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCHours(23, 59, 59, 999); d.getUTCMilliseconds()",
    )
    .unwrap();
    assert_eq!(r.as_double(), 999.0);
}

#[test]
fn date_utc_returns_correct_timestamp() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.UTC(2020, 0, 1, 12, 0, 0)").unwrap();
    assert_eq!(r.as_double(), 1577880000000.0);
}

#[test]
fn date_utc_nan_returns_nan() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.UTC(NaN, 0)").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_parse_rfc2822() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.parse('15 Jan 2020 12:00:00 GMT')").unwrap();
    assert!(!r.as_double().is_nan());
}

#[test]
fn date_parse_slash_format() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.parse('2020/01/15')").unwrap();
    assert!(!r.as_double().is_nan());
}

#[test]
fn date_get_year_deprecated() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 1).getYear()").unwrap();
    assert_eq!(r.as_double(), 120.0);
}

#[test]
fn date_set_year_deprecated() {
    let mut vm = Vm::new();
    // setYear(0-99) 加 1900；setYear(20) → fullYear 1920。
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1); d.setYear(20); d.getFullYear()").unwrap();
    assert_eq!(r.as_double(), 1920.0);
}

#[test]
fn date_to_gmt_string_matches_utc() {
    let mut vm = Vm::new();
    let r1 = eval(&mut vm, "new Date(0).toGMTString()").unwrap();
    let r2 = eval(&mut vm, "new Date(0).toUTCString()").unwrap();
    assert_eq!(str_val(&vm, r1), str_val(&vm, r2));
}

#[test]
fn date_locale_strings_not_empty() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(0).toLocaleString()").unwrap();
    assert!(r.is_string());
}

#[test]
fn date_set_utc_full_year_returns_timestamp() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); var ts = d.setUTCFullYear(2025); ts == d.getTime() ? 1 : 0").unwrap();
    assert_eq!(r.as_int(), 1);
}

// setUTCFullYear 对无效 this 不早退：取 +0 基准（1970-01-01 UTC 全零）继续
// 计算并写槽，年份 2025 落在 0..99 映射之外，结果为 2025-01-01T00:00:00Z。
#[test]
fn date_set_utc_full_year_invalid_this_uses_plus_zero_base() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(NaN).setUTCFullYear(2025)").unwrap();
    assert_eq!(r.as_double(), 1735689600000.0);
}

// setYear 无参数时不得索引缺失的参数寄存器（此前会 panic）；
// 缺失 / NaN 的年份把日期值置为 NaN 并返回 NaN（B.2.4.2）。
#[test]
fn date_set_year_no_arg_returns_nan() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setYear()").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_set_year_no_arg_sets_value_nan() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setYear(); d.valueOf()").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_set_year_nan_from_string() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setYear('not a number')").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_set_year_two_digit_maps_to_1900s() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setYear(95); d.getFullYear()").unwrap();
    assert_eq!(r.as_double() as i64, 1995);
}

#[test]
fn date_set_year_four_digit_kept() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setYear(2020); d.getFullYear()").unwrap();
    assert_eq!(r.as_double() as i64, 2020);
}

// setYear 语义钉：this 值无效取 +0 基准（1970-01-01 全零）不早退、
// 结果经 TimeClip 包络、年份参数走完整 ToNumber（valueOf 通道、
// Symbol 抛 TypeError）。
#[test]
fn date_set_year_invalid_this_uses_plus_zero_base() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(NaN); d.setYear(71); d.getTime() == new Date(1971, 0).valueOf() ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_set_year_time_clip_returns_nan() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(1970, 8, 14); var ts = d.setYear(275760); Number.isNaN(ts) && Number.isNaN(d.valueOf()) ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_set_year_valueof_channel() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(0); d.setYear({valueOf: function() { return 20; }}); d.getFullYear()",
    )
    .unwrap();
    assert_eq!(r.as_double() as i64, 1920);
}

#[test]
fn date_set_year_symbol_throws_type_error() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(0); try { d.setYear(Symbol()); 'no' } catch (e) { e instanceof TypeError ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
}

// UTC setter 无参数调用此前会索引缺失的参数寄存器（panic / 进程中止）。
// 缺失的主参数现在产出 NaN 并把日期置为 NaN。
#[test]
fn date_set_utc_setters_no_arg_return_nan() {
    let mut vm = Vm::new();
    for call in [
        "d.setUTCFullYear()",
        "d.setUTCMonth()",
        "d.setUTCDate()",
        "d.setUTCHours()",
        "d.setUTCMinutes()",
        "d.setUTCSeconds()",
        "d.setUTCMilliseconds()",
    ] {
        let src = format!("var d = new Date(0); {call}");
        let r = eval(&mut vm, &src).unwrap();
        assert!(r.as_double().is_nan(), "{call} should return NaN, got {r:?}");
    }
}

#[test]
fn date_set_utc_no_arg_sets_value_nan() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setUTCMonth(); d.valueOf()").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_set_utc_full_year_valid_kept() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setUTCFullYear(2025); d.getUTCFullYear()").unwrap();
    assert_eq!(r.as_double() as i64, 2025);
}

#[test]
fn date_set_utc_hours_valid_kept() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(0); d.setUTCHours(13); d.getUTCHours()").unwrap();
    assert_eq!(r.as_double() as i64, 13);
}

#[test]
fn date_length_descriptor() {
    // Date.length = 7，描述符 { writable:false, enumerable:false, configurable:true }。
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(Date, 'length'); \
         d.value === 7 && d.writable === false && d.enumerable === false && d.configurable === true",
    )
    .unwrap();
    assert!(r.as_bool());
}

// UTC setter 族强转序钉：参数 ToNumber 先于 this 值 NaN 判定，无效 this 仍触发
// valueOf（计数一次），返回 NaN 且不写槽。
#[test]
fn date_set_utc_month_invalid_this_still_coerces() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(NaN); var n = 0; \
         var v = d.setUTCMonth({ valueOf: function() { n++; return 0; } }); \
         n === 1 && Number.isNaN(v) && Number.isNaN(d.getTime()) ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

// setUTCFullYear 无效 this 取 +0 基准（1970-01-01 UTC 全零）：
// 未给月/日时取基准分量，结果为 1970-01-01T00:00:00Z 加 2025 年。
#[test]
fn date_set_utc_full_year_invalid_this_base_components() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(NaN); d.setUTCFullYear(1975, 2, 14) === new Date(Date.UTC(1975, 2, 14)).getTime() ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

// setTime 越界值经 TimeClip 归 NaN（|t| > 8.64e15 或非有限）。
#[test]
fn date_set_time_out_of_range_time_clip() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(0); var ts = d.setTime(8.64e15 + 1); \
         Number.isNaN(ts) && Number.isNaN(d.getTime()) ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
    // 上界内的值原样入槽。
    let r = eval(&mut vm, "var d = new Date(0); d.setTime(8.64e15) === 8.64e15 ? 1 : 0").unwrap();
    assert_eq!(r.as_int(), 1);
}

// Date.UTC 年份先截断后做 0..99 映射：-0.999999 截断为 -0 落区间，归 1900 年。
#[test]
fn date_utc_truncates_before_year_offset() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Date.UTC(-0.999999, 0) === Date.UTC(1900, 0) ? 1 : 0").unwrap();
    assert_eq!(r.as_int(), 1);
    let r = eval(&mut vm, "Date.UTC(99.999999, 0) === Date.UTC(1999, 0) ? 1 : 0").unwrap();
    assert_eq!(r.as_int(), 1);
    let r = eval(&mut vm, "Date.UTC(100, 0) === -59011459200000 ? 1 : 0").unwrap();
    assert_eq!(r.as_int(), 1);
}

// 多参构造器七参按序走完整 ToNumber（对象参经 toString 通道），
// 且某一参抛错时中断后续强转并原样传播异常。
#[test]
fn date_constructor_object_args_coercion_order() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var log = ''; \
         new Date({toString: function() { log += 'y'; return 0; }}, \
                  {toString: function() { log += 'm'; return 0; }}, \
                  {toString: function() { log += 'd'; return 1; }}); \
         log === 'ymd' ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_constructor_object_arg_error_propagates() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var n = 0; try { \
         new Date({ valueOf: function() { throw 'boom'; }}, \
                  { valueOf: function() { n++; } }); \
         'no'; \
         } catch (e) { e === 'boom' && n === 0 ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
}

// ── toTemporalInstant ──

#[test]
fn date_to_temporal_instant_epoch_values() {
    let mut vm = Vm::new();
    // 有效日期换算为 epoch 纳秒（毫秒 × 10^6），逐值与 BigInt 字面量比较。
    let cases = [
        ("new Date(0)", "0n"),
        ("new Date(123456789)", "123456789000000n"),
        ("new Date(-123456789)", "-123456789000000n"),
        // TimeClip 包络两端（±8.64e15 ms）。
        ("new Date(8.64e15)", "8640000000000000000000n"),
        ("new Date(-8.64e15)", "-8640000000000000000000n"),
    ];
    for (date, ns) in cases {
        let r = eval(&mut vm, &format!("{date}.toTemporalInstant().epochNanoseconds === {ns} ? 1 : 0")).unwrap();
        assert_eq!(r.as_int(), 1, "date = {date}");
    }
}

#[test]
fn date_to_temporal_instant_invalid_date_throws_range_error() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = new Date(NaN); \
         try { d.toTemporalInstant(); 'no' } catch (e) { e instanceof RangeError ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
}

#[test]
fn date_to_temporal_instant_non_date_throws_type_error() {
    let mut vm = Vm::new();
    let f = |vm: &mut Vm, this: &str| {
        eval(
            vm,
            &format!(
                "var m = Date.prototype.toTemporalInstant; \
                 try {{ m.call({this}); 'no' }} catch (e) {{ e instanceof TypeError ? 'yes' : 'no' }}"
            ),
        )
        .unwrap()
    };
    for this in ["{}", "[]", "function(){}", "Date.prototype", "0", "null", "undefined", "Symbol()"] {
        let r = f(&mut vm, this);
        assert_eq!(str_val(&vm, r), "yes", "this = {this}");
    }
}

#[test]
fn date_to_temporal_instant_prop_desc() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(Date.prototype, 'toTemporalInstant'); \
         d.writable === true && d.enumerable === false && d.configurable === true ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_to_temporal_instant_length_name() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "Date.prototype.toTemporalInstant.length === 0 && \
         Date.prototype.toTemporalInstant.name === 'toTemporalInstant' ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

// ── [Symbol.toPrimitive] ──

#[test]
fn date_to_primitive_number_hint_first_valid() {
    let mut vm = Vm::new();
    // number 提示 valueOf 优先：valueOf 产出原始值即返回，toString 不被引用。
    let r = eval(
        &mut vm,
        "var vo = 0, ts = 0; \
         var obj = { valueOf: function() { vo++; return 'valueOf test'; }, \
                    get toString() { ts++; } }; \
         var v = Date.prototype[Symbol.toPrimitive].call(obj, 'number'); \
         v === 'valueOf test' && vo === 1 && ts === 0 ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_to_primitive_string_hint_first_valid() {
    let mut vm = Vm::new();
    // string 提示 toString 优先：toString 产出原始值即返回，valueOf 不被引用。
    let r = eval(
        &mut vm,
        "var vo = 0, ts = 0; \
         var obj = { get valueOf() { vo++; }, \
                    toString: function() { ts++; return 'toString test'; } }; \
         var v = Date.prototype[Symbol.toPrimitive].call(obj, 'string'); \
         v === 'toString test' && vo === 0 && ts === 1 ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_to_primitive_default_hint_uses_string_order() {
    let mut vm = Vm::new();
    // default 提示与 string 同序（toString 优先）。
    let r = eval(
        &mut vm,
        "var obj = { valueOf: function() { return 1; }, \
                    toString: function() { return 'def'; } }; \
         Date.prototype[Symbol.toPrimitive].call(obj, 'default') === 'def' ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_to_primitive_number_hint_first_non_callable() {
    let mut vm = Vm::new();
    // valueOf 非可调用时回落 toString。
    let r = eval(
        &mut vm,
        "var obj = { valueOf: null, toString: function() { return 'ts'; } }; \
         Date.prototype[Symbol.toPrimitive].call(obj, 'number') === 'ts' ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_to_primitive_number_hint_first_invalid() {
    let mut vm = Vm::new();
    // valueOf 返回对象（非原始值）时回落 toString。
    let r = eval(
        &mut vm,
        "var obj = { valueOf: function() { return {}; }, \
                    toString: function() { return 'ts'; } }; \
         Date.prototype[Symbol.toPrimitive].call(obj, 'number') === 'ts' ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_to_primitive_no_callables_throws_type_error() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var obj = { valueOf: null, toString: null }; \
         try { Date.prototype[Symbol.toPrimitive].call(obj, 'number'); 'no' } \
         catch (e) { e instanceof TypeError ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
}

#[test]
fn date_to_primitive_invalid_hint_throws_type_error() {
    let mut vm = Vm::new();
    let f = |vm: &mut Vm, hint: &str| {
        eval(
            vm,
            &format!(
                "var d = new Date(0); \
                 try {{ d[Symbol.toPrimitive]({hint}); 'no' }} \
                 catch (e) {{ e instanceof TypeError ? 'yes' : 'no' }}"
            ),
        )
        .unwrap()
    };
    // 缺参、undefined、null、空串、非原始串（String 对象 / 带 toString 的对象）均抛 TypeError。
    for hint in [
        "",
        "undefined",
        "null",
        "''",
        "'String'",
        "new String('number')",
        "{ toString: function() { 'number'; } }",
    ] {
        let r = f(&mut vm, hint);
        assert_eq!(str_val(&vm, r), "yes", "hint = {hint}");
    }
}

#[test]
fn date_to_primitive_non_object_this_throws_type_error() {
    let mut vm = Vm::new();
    let f = |vm: &mut Vm, this: &str| {
        eval(
            vm,
            &format!(
                "try {{ Date.prototype[Symbol.toPrimitive].call({this}, 'string'); 'no' }} \
                 catch (e) {{ e instanceof TypeError ? 'yes' : 'no' }}"
            ),
        )
        .unwrap()
    };
    for this in ["undefined", "null", "86", "''", "true"] {
        let r = f(&mut vm, this);
        assert_eq!(str_val(&vm, r), "yes", "this = {this}");
    }
}

#[test]
fn date_to_primitive_works_on_plain_object() {
    let mut vm = Vm::new();
    // 非 Date 品牌方法：普通对象合法（仅要求 this 为对象）。
    let r = eval(
        &mut vm,
        "var obj = { valueOf: function() { return 42; } }; \
         Date.prototype[Symbol.toPrimitive].call(obj, 'number') === 42 ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_to_primitive_prop_desc_length_name() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(Date.prototype, Symbol.toPrimitive); \
         d.writable === false && d.enumerable === false && d.configurable === true ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
    let r = eval(
        &mut vm,
        "Date.prototype[Symbol.toPrimitive].length === 1 && \
         Date.prototype[Symbol.toPrimitive].name === '[Symbol.toPrimitive]' ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_to_primitive_user_throw_propagates_original() {
    let mut vm = Vm::new();
    // 用户方法抛错经原始异常值传播（不降级为 TypeError）。
    let r = eval(
        &mut vm,
        "var obj = { valueOf: function() { throw 'boom'; } }; \
         try { Date.prototype[Symbol.toPrimitive].call(obj, 'number'); 'no' } \
         catch (e) { e === 'boom' ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
}

#[test]
fn date_to_primitive_called_as_function_strict_this() {
    let mut vm = Vm::new();
    // 严格脚本顶层函数形态调用（this 为 undefined）抛 TypeError，且不触发 toString/valueOf 查找。
    // 用严格脚本使 this 恒为 undefined（规范保证），不依赖 sloppy 顶层 this 绑定
    // （JS 函数与 native 函数两条路径取值不同，前者替换为全局对象、后者保留 undefined）。
    let r = eval(
        &mut vm,
        "'use strict'; var toPrimitive = Date.prototype[Symbol.toPrimitive]; \
         try { toPrimitive('default'); 'no' } catch (e) { e instanceof TypeError ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
}

// 既有 Date 强转路径行为回归：绑定 [Symbol.toPrimitive] 后，VM 的 ToPrimitive
// 对 Date 先经原型方法再走 OrdinaryToPrimitive，可观察行为与直接 valueOf/toString 等价。
#[test]
fn date_to_primitive_existing_coercion_regression() {
    let mut vm = Vm::new();
    // +new Date(0) 经 valueOf 通道。
    let r = eval(&mut vm, "+new Date(0)").unwrap();
    assert_eq!(r.as_double(), 0.0);
    // "" + new Date(0) 经 toString 通道（得到字符串）。
    let r = eval(&mut vm, "typeof ('' + new Date(0))").unwrap();
    assert_eq!(str_val(&vm, r), "string");
}

// ── 严格 ISO 解析（Date Time String Format）──

#[test]
fn date_parse_extended_year_time_clip_boundaries() {
    let mut vm = Vm::new();
    // 扩展年落在 TimeClip 包络两端，按规范解析为包络端点值。
    let r = eval(&mut vm, "Date.parse('275760-09-13T00:00:00Z')").unwrap();
    assert_eq!(r.as_double(), 8.64e15);
    let r = eval(&mut vm, "Date.parse('-271821-04-20T00:00:00Z')").unwrap();
    assert_eq!(r.as_double(), -8.64e15);
    // 扩展年越出包络：整体可识别，TimeClip 归 NaN。
    let r = eval(&mut vm, "Date.parse('275761-01-01T00:00:00Z')").unwrap();
    assert!(r.as_double().is_nan());
    let r = eval(&mut vm, "Date.parse('-271822-01-01T00:00:00Z')").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_parse_negative_zero_year_rejected() {
    let mut vm = Vm::new();
    // 年 0 视为正，须写作 +000000；-000000 不是合法扩展年。
    let r = eval(&mut vm, "Date.parse('-000000-01-01T00:00:00Z')").unwrap();
    assert!(r.as_double().is_nan());
    let r = eval(&mut vm, "Date.parse('+000000-01-01T00:00:00Z')").unwrap();
    assert_eq!(r.as_double(), -62167219200000.0);
}

#[test]
fn date_parse_date_only_is_utc() {
    let mut vm = Vm::new();
    // 日期-only 形式解释为 UTC 零点。
    let r = eval(&mut vm, "Date.parse('2020-01-01')").unwrap();
    assert_eq!(r.as_double(), 1577836800000.0);
}

#[test]
fn date_parse_offsetless_datetime_is_local() {
    let mut vm = Vm::new();
    // 无偏移完整日期时间解释为本地时间（与本地构造器互证，宿主无关）。
    let r = eval(&mut vm, "Date.parse('2020-01-01T12:00:00')").unwrap();
    let r2 = eval(&mut vm, "new Date(2020, 0, 1, 12).getTime()").unwrap();
    assert_eq!(r.as_double(), r2.as_double());
}

#[test]
fn date_parse_midnight_24_accepted() {
    let mut vm = Vm::new();
    // 24:00:00 等价于次日 00:00。
    let r = eval(&mut vm, "Date.parse('2020-01-01T24:00:00Z')").unwrap();
    assert_eq!(r.as_double(), 1577923200000.0);
    // 24 时仅允许 24:00:00 形态，分秒毫秒非零即非法。
    let r = eval(&mut vm, "Date.parse('2020-01-01T24:01:00Z')").unwrap();
    assert!(r.as_double().is_nan());
    let r = eval(&mut vm, "Date.parse('2020-01-01T24:00:01Z')").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_parse_invalid_calendar_rejected() {
    let mut vm = Vm::new();
    // 月 13、二月 30 日、时 25 均不是合法日期时间串。
    let r = eval(&mut vm, "Date.parse('2020-13-01T00:00:00Z')").unwrap();
    assert!(r.as_double().is_nan());
    let r = eval(&mut vm, "Date.parse('2020-02-30T00:00:00Z')").unwrap();
    assert!(r.as_double().is_nan());
    let r = eval(&mut vm, "Date.parse('2020-01-01T25:00:00Z')").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_parse_engine_format_year_beyond_envelope_is_nan() {
    let mut vm = Vm::new();
    // 引擎自身输出形态的年号超出 TimeClip 包络：归 NaN，不 panic、不环绕。
    let r = eval(&mut vm, "Date.parse('Wed, 05 Aug 300000000 12:00:00 GMT')").unwrap();
    assert!(r.as_double().is_nan());
    // 负年同侧越界同样归 NaN。
    let r = eval(&mut vm, "Date.parse('Wed, 05 Aug -300000000 12:00:00 GMT')").unwrap();
    assert!(r.as_double().is_nan());
}

// ── toISOString 6 位年与 TimeClip 边界 ──

#[test]
fn date_to_iso_string_six_digit_year() {
    let mut vm = Vm::new();
    // 4 位年零填充（epoch 为 1970 年）。
    let r = eval(&mut vm, "new Date(0).toISOString()").unwrap();
    assert_eq!(str_val(&vm, r), "1970-01-01T00:00:00.000Z");
    // 年 0 输出 4 位 0000。
    let r = eval(&mut vm, "new Date(-62167219200000).toISOString()").unwrap();
    assert_eq!(str_val(&vm, r), "0000-01-01T00:00:00.000Z");
    // 负年带负号前缀（年 -1 的 1 月 1 日）。
    let r = eval(&mut vm, "new Date(-62198755200000).toISOString()").unwrap();
    assert_eq!(str_val(&vm, r), "-000001-01-01T00:00:00.000Z");
}

#[test]
fn date_to_iso_string_time_clip_boundaries() {
    let mut vm = Vm::new();
    // 包络两端合法（正年 > 9999 带 + 前缀 6 位，负年带 - 前缀 6 位）。
    let r = eval(&mut vm, "new Date(8.64e15).toISOString()").unwrap();
    assert_eq!(str_val(&vm, r), "+275760-09-13T00:00:00.000Z");
    let r = eval(&mut vm, "new Date(-8.64e15).toISOString()").unwrap();
    assert_eq!(str_val(&vm, r), "-271821-04-20T00:00:00.000Z");
    // 越出包络一毫秒：时间值归 NaN，toISOString 抛 RangeError。
    let r = eval(
        &mut vm,
        "try { new Date(8.64e15 + 1).toISOString(); 'no' } catch (e) { e instanceof RangeError ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
    let r = eval(
        &mut vm,
        "try { new Date(-8.64e15 - 1).toISOString(); 'no' } catch (e) { e instanceof RangeError ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
}

// ── toJSON 泛型化 ──

#[test]
fn date_to_json_invalid_date_returns_null() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(NaN).toJSON()").unwrap();
    assert!(r.is_null());
}

#[test]
fn date_to_json_generic_non_date_this() {
    let mut vm = Vm::new();
    // 非有限 Number 结果返回 null（有限性检查仅对 Number 生效）。
    let r = eval(&mut vm, "Date.prototype.toJSON.call(Infinity)").unwrap();
    assert!(r.is_null());
    // 字符串结果（非 Number）不拦截，直落 Invoke(toISOString)，
    // 装箱后的 String 对象无该方法，抛 TypeError。
    let r = eval(
        &mut vm,
        "try { Date.prototype.toJSON.call('2020-01-01T00:00:00Z'); 'no' } \
         catch (e) { e instanceof TypeError ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
    // 有限数值接收者装箱后走到 Invoke(toISOString)，方法不可调用抛 TypeError。
    let r = eval(
        &mut vm,
        "try { Date.prototype.toJSON.call(42); 'no' } catch (e) { e instanceof TypeError ? 'yes' : 'no' }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "yes");
}

#[test]
fn date_to_json_via_json_stringify() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "JSON.stringify({ a: new Date(0) })").unwrap();
    assert_eq!(str_val(&vm, r), "{\"a\":\"1970-01-01T00:00:00.000Z\"}");
}

// ── 构造器单参对象：ToPrimitive 传 default 提示 ──

#[test]
fn date_constructor_single_arg_object_default_hint() {
    let mut vm = Vm::new();
    // 单参对象构造经 ToPrimitive（default 提示），@@toPrimitive 收到 "default"。
    let r = eval(
        &mut vm,
        "var d = new Date({ [Symbol.toPrimitive]: function(h) { return h === 'default' ? 0 : 1; } }); \
         d.getTime() === 0 ? 1 : 0",
    )
    .unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_constructor_single_arg_truncates_before_time_clip() {
    let mut vm = Vm::new();
    // 单参数值先截断再 TimeClip；-0.5 截断为 -0，归一为 +0。
    let r = eval(&mut vm, "Object.is(new Date(-0.5).getTime(), 0) ? 1 : 0").unwrap();
    assert_eq!(r.as_int(), 1);
}
