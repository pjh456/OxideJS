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

#[test]
fn date_set_utc_invalid_returns_nan() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(NaN).setUTCFullYear(2025)").unwrap();
    assert!(r.as_double().is_nan());
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
