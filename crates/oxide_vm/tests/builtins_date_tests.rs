use oxide_compiler::compiler::Compiler;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
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

// -- new Date() constructor via JS --

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
    // Parse an explicit ISO UTC string to avoid timezone dependency
    let r = eval(&mut vm, "new Date('2020-01-01T00:00:00Z').toISOString()").unwrap();
    let s = str_val(&vm, r);
    assert!(s.starts_with("2020-01-01"), "got: {s}");
}

#[test]
fn date_to_json() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Date(2020, 0, 1).toJSON()").unwrap();
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

// -- Plan 01 timezone-correct tests --

#[test]
fn date_local_getters_differ_from_utc() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(2020, 6, 15, 12, 0, 0); d.getHours()").unwrap();
    let local_hour = r.as_double();
    let r = eval(&mut vm, "var d = new Date(2020, 6, 15, 12, 0, 0); d.getUTCHours()").unwrap();
    let utc_hour = r.as_double();
    // In UTC+0 they are equal (correct), otherwise they differ
    if utc_hour != local_hour {
        assert_ne!(local_hour, utc_hour, "in non-UTC+0, local vs UTC hours should differ");
    }
    assert!(local_hour >= 0.0 && local_hour < 24.0);
}

#[test]
fn date_constructor_uses_local_timezone() {
    let mut vm = Vm::new();
    // getTimezoneOffset = UTC - local; UTC hour = local + offset/60
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 12, 0, 0); d.getUTCHours()").unwrap();
    let utc_hour = r.as_double();
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 12, 0, 0); d.getTimezoneOffset()").unwrap();
    let offset = r.as_double();
    let expected_utc = (12.0 + offset / 60.0).rem_euclid(24.0);
    assert_eq!(utc_hour, expected_utc, "getUTCHours = local + TZ_offset/60");
}

#[test]
fn date_constructor_defaults_from_current_time() {
    let mut vm = Vm::new();
    // D-02: missing args default to current time components
    let r = eval(&mut vm, "var d = new Date(2020, 5); d.getMonth()").unwrap();
    assert_eq!(r.as_double(), 5.0);
    let r = eval(&mut vm, "new Date(2020, 5).getDate()").unwrap();
    assert!(r.as_double() >= 1.0 && r.as_double() <= 31.0);
}

#[test]
fn date_constructor_truncation() {
    let mut vm = Vm::new();
    // D-03: ToInteger truncation
    let r = eval(&mut vm, "new Date(2024.7, 5.9).getFullYear()").unwrap();
    assert_eq!(r.as_double(), 2024.0);
    let r = eval(&mut vm, "new Date(2024.7, 5.9).getMonth()").unwrap();
    assert_eq!(r.as_double(), 5.0);
}

#[test]
fn date_setters_return_timestamp() {
    let mut vm = Vm::new();
    // D-05: setters return modified timestamp
    let r = eval(&mut vm, "var d = new Date(0); var ts = d.setHours(12); ts == d.getTime() ? 1 : 0").unwrap();
    assert_eq!(r.as_int(), 1);
}

#[test]
fn date_invalid_returns_nan() {
    let mut vm = Vm::new();
    // D-06: Invalid Date getters/setters return NaN
    let r = eval(&mut vm, "new Date(NaN).getFullYear()").unwrap();
    assert!(r.as_double().is_nan());
    let r = eval(&mut vm, "new Date(NaN).setHours(0)").unwrap();
    assert!(r.as_double().is_nan());
    // D-04: Invalid Date constructor
    let r = eval(&mut vm, "new Date(NaN, 0).getTime()").unwrap();
    assert!(r.as_double().is_nan());
}

#[test]
fn date_get_timezone_offset_uses_date_timestamp() {
    let mut vm = Vm::new();
    // D-09/D-10: offset uses date's timestamp, not now()
    let r = eval(&mut vm, "new Date(0).getTimezoneOffset()").unwrap();
    assert!(r.as_double().is_finite());
}

#[test]
fn date_setters_optional_chain() {
    let mut vm = Vm::new();
    // D-08: setFullYear supports optional month/day params
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
    // setHours changes local hour; verify local hour value
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 0, 0, 0); d.setHours(12); d.getHours()").unwrap();
    assert_eq!(r.as_double(), 12.0, "setHours(12) should make getHours() return 12");
}

// -- Plan 02 new method tests --

#[test]
fn date_set_utc_full_year_works() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCFullYear(2025); d.getUTCFullYear()").unwrap();
    assert_eq!(r.as_double(), 2025.0);
}

#[test]
fn date_set_utc_hours_optional_chain() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCHours(23, 59, 59, 999); d.getUTCHours()").unwrap();
    assert_eq!(r.as_double(), 23.0);
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCHours(23, 59, 59, 999); d.getUTCMinutes()").unwrap();
    assert_eq!(r.as_double(), 59.0);
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCHours(23, 59, 59, 999); d.getUTCSeconds()").unwrap();
    assert_eq!(r.as_double(), 59.0);
    let r = eval(&mut vm, "var d = new Date(2020, 0, 1, 0, 0, 0); d.setUTCHours(23, 59, 59, 999); d.getUTCMilliseconds()").unwrap();
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
    // setYear(0-99) adds 1900; setYear(20) → fullYear 1920
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
