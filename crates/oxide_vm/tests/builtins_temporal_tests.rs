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

fn num(vm: &mut Vm, source: &str) -> f64 {
    let r = eval(vm, source).unwrap();
    if r.is_double() {
        r.as_double()
    } else {
        r.as_int() as f64
    }
}

// -- Temporal 命名空间 --

#[test]
fn temporal_is_object() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "typeof Temporal").unwrap();
    assert_eq!(str_val(&vm, r), "object");
}

#[test]
fn temporal_now_time_zone_id_is_utc() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Temporal.Now.timeZoneId()").unwrap();
    assert_eq!(str_val(&vm, r), "UTC");
}

#[test]
fn temporal_now_instant_returns_instant() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Temporal.Now.instant() instanceof Temporal.Instant").unwrap();
    assert!(r.as_bool());
}

#[test]
fn temporal_now_instant_epoch_is_positive() {
    let mut vm = Vm::new();
    let r = num(&mut vm, "Temporal.Now.instant().epochMilliseconds");
    assert!(r > 1_500_000_000_000.0);
}

#[test]
fn temporal_now_is_namespace_not_constructor() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "typeof Temporal.Now").unwrap();
    assert_eq!(str_val(&vm, r), "object");
}

// -- Temporal.Instant --

#[test]
fn instant_from_iso_string() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Temporal.Instant.from('2024-01-01T00:00:00Z').toString()").unwrap();
    assert_eq!(str_val(&vm, r), "2024-01-01T00:00:00Z");
}

#[test]
fn instant_epoch_seconds() {
    let mut vm = Vm::new();
    assert_eq!(num(&mut vm, "Temporal.Instant.from('2024-01-01T00:00:00Z').epochSeconds"), 1704067200.0);
}

#[test]
fn instant_epoch_milliseconds() {
    let mut vm = Vm::new();
    assert_eq!(
        num(&mut vm, "Temporal.Instant.from('2024-01-01T00:00:00Z').epochMilliseconds"),
        1704067200000.0
    );
}

#[test]
fn instant_epoch_nanoseconds() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "Temporal.Instant.from('2024-01-01T00:00:00Z').epochNanoseconds === 1704067200000000000n",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_from_epoch_milliseconds_is_exact() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "Temporal.Instant.fromEpochMilliseconds(-217175010876).epochNanoseconds === -217175010876000000n",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_from_epoch_nanoseconds_is_exact() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "Temporal.Instant.fromEpochNanoseconds(217175010123456789n).epochNanoseconds === 217175010123456789n
            && Temporal.Instant.fromEpochNanoseconds(-217175010876543211n).epochMilliseconds === -217175010877",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_equals_compares_exact_epoch_nanoseconds() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "Temporal.Instant.fromEpochNanoseconds(217175010123456789n)
            .equals(Temporal.Instant.fromEpochMilliseconds(217175010123))",
    )
    .unwrap();
    assert!(!r.as_bool());
}

#[test]
fn instant_compare_accepts_instances_strings_and_annotations() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "Temporal.Instant.compare('1970-01-01T00:00Z[UTC]', new Temporal.Instant(0n)) === 0
            && Temporal.Instant.compare('1969-12-31T23:00+00:00', new Temporal.Instant(0n)) === -1
            && Temporal.Instant.compare('1970-01-01T01:00+00:00', new Temporal.Instant(0n)) === 1",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_epoch_factories_validate_input_and_range() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(() => { try { Temporal.Instant.fromEpochMilliseconds(1.5); return false } catch (e) { return e instanceof RangeError } })()
        && (() => { try { Temporal.Instant.fromEpochMilliseconds(1n); return false } catch (e) { return e instanceof TypeError } })()
        && (() => { try { Temporal.Instant.fromEpochNanoseconds(1); return false } catch (e) { return e instanceof TypeError } })()
        && (() => { try { Temporal.Instant.fromEpochNanoseconds(8640000000000000000001n); return false } catch (e) { return e instanceof RangeError } })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_constructor_requires_new() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "try { Temporal.Instant(0); 'no' } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
}

#[test]
fn instant_value_of_throws() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "try { Temporal.Instant.prototype.valueOf.call(Temporal.Now.instant()) } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
}

#[test]
fn instant_method_on_non_instant_throws() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "try { Temporal.Instant.prototype.toString.call({}) } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
}

#[test]
fn instant_from_invalid_throws_range_error() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "try { Temporal.Instant.from('nope') } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
}

// -- Temporal.PlainDate --

#[test]
fn plain_date_from_string() {
    let mut vm = Vm::new();
    assert_eq!(num(&mut vm, "Temporal.PlainDate.from('2024-01-15').year"), 2024.0);
    assert_eq!(num(&mut vm, "Temporal.PlainDate.from('2024-01-15').month"), 1.0);
    assert_eq!(num(&mut vm, "Temporal.PlainDate.from('2024-01-15').day"), 15.0);
}

#[test]
fn plain_date_from_object() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "Temporal.PlainDate.from({year:2023, month:3, day:5}).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "2023-03-05");
}

#[test]
fn plain_date_constructor_month_day() {
    let mut vm = Vm::new();
    assert_eq!(num(&mut vm, "new Temporal.PlainDate(2024, 2, 29).month"), 2.0);
    assert_eq!(num(&mut vm, "new Temporal.PlainDate(2024, 2, 29).day"), 29.0);
    assert_eq!(num(&mut vm, "new Temporal.PlainDate(2024, 2, 29).year"), 2024.0);
}

#[test]
fn plain_date_leap_year_validation() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "try { new Temporal.PlainDate(2024, 2, 30) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
}

#[test]
fn plain_date_invalid_month() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "try { new Temporal.PlainDate(2024, 13, 1) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
}

#[test]
fn plain_date_to_string() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainDate(2024, 1, 5).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "2024-01-05");
}

#[test]
fn plain_date_requires_new() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "try { Temporal.PlainDate(2024, 1, 1); 'no' } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
}

#[test]
fn plain_date_method_on_non_date_throws() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "try { Temporal.PlainDate.prototype.year.get.call({}) } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
}

#[test]
fn plain_date_add_and_subtract_support_negative_offsets() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainDate(2024, 1, 31).add({months:1}).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "2024-02-29");
    let r = eval(&mut vm, "new Temporal.PlainDate(2024, 1, 5).subtract({days:10}).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "2023-12-26");
}

#[test]
fn plain_date_until_and_since_return_duration() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainDate(2024, 1, 1).until('2024-01-06').toString()").unwrap();
    assert_eq!(str_val(&vm, r), "P5D");
    assert_eq!(
        num(
            &mut vm,
            "new Temporal.PlainDate(2024, 1, 6).since(new Temporal.PlainDate(2024, 1, 11)).days",
        ),
        -5.0
    );
    assert_eq!(
        num(
            &mut vm,
            "new Temporal.PlainDate(1997, 7, 16).since(new Temporal.PlainDate(2021, 7, 15), {largestUnit:'years'}).days",
        ),
        -29.0
    );
}

#[test]
fn duration_constructor_exposes_components() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.Duration(1,2,3,4,5,6,7,8,9,10).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "P1Y2M3W4DT5H6M7.00800901S");
    assert_eq!(num(&mut vm, "new Temporal.Duration(1,2,3,4,5,6,7,8,9,10).nanoseconds"), 10.0);
    let r = eval(&mut vm, "Temporal.Duration.from('-PT24.567890123H').toString()").unwrap();
    assert_eq!(str_val(&vm, r), "-PT24H34M4.4044428S");
    assert_eq!(num(&mut vm, "Temporal.Duration.from('P43Y').years"), 43.0);
    let r = eval(&mut vm, "new Temporal.Duration(1,2,3,4).negated().abs().toString()").unwrap();
    assert_eq!(str_val(&vm, r), "P1Y2M3W4D");
}

#[test]
fn duration_from_validates_property_bags_and_preserves_precision() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "try { Temporal.Duration.from({}) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
    let r = eval(
        &mut vm,
        "try { Temporal.Duration.from({hours:1,minutes:-1}) } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
    let r = eval(
        &mut vm,
        "Temporal.Duration.from({milliseconds:4503599627370497000,microseconds:4503599627370495000000}).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "PT9007199254740991.975424S");
}

#[test]
fn duration_from_propagates_getter_errors_and_checks_range() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "try { Temporal.Duration.from({get years(){throw new TypeError('sentinel')}}) } catch (e) { e.message }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "sentinel");
    let r = eval(
        &mut vm,
        "try { Temporal.Duration.from({seconds:9007199254740992}) } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
    let r = eval(&mut vm, "Temporal.Duration.from('p1y1m1dt1h1m1s').toString()").unwrap();
    assert_eq!(str_val(&vm, r), "P1Y1M1DT1H1M1S");
}

#[test]
fn plain_date_balances_duration_time_units_into_days() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainDate(2000,5,2).add('P1DT24H1440M86400S').toString()").unwrap();
    assert_eq!(str_val(&vm, r), "2000-05-06");
    let r = eval(&mut vm, "new Temporal.PlainDate(2000,5,2).add('-PT24.567890123H').toString()").unwrap();
    assert_eq!(str_val(&vm, r), "2000-05-01");
}

// -- Temporal.PlainTime --

#[test]
fn plain_time_constructor_components() {
    let mut vm = Vm::new();
    assert_eq!(num(&mut vm, "new Temporal.PlainTime(12, 30, 5).hour"), 12.0);
    assert_eq!(num(&mut vm, "new Temporal.PlainTime(12, 30, 5).minute"), 30.0);
    assert_eq!(num(&mut vm, "new Temporal.PlainTime(12, 30, 5).second"), 5.0);
}

#[test]
fn plain_time_full_precision_components() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let t = new Temporal.PlainTime(23, 59, 59, 999, 999, 999); t.hour + ',' + t.minute + ',' + t.second + ',' + t.millisecond + ',' + t.microsecond + ',' + t.nanosecond",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "23,59,59,999,999,999");
}

#[test]
fn plain_time_to_string() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 5).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "12:30:05");
}

#[test]
fn plain_time_to_string_with_subseconds() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 5, 500, 250, 1).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "12:30:05.500250001");
}

#[test]
fn plain_time_missing_components_default_zero() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainTime().toString()").unwrap();
    assert_eq!(str_val(&vm, r), "00:00:00");
    let r = eval(&mut vm, "new Temporal.PlainTime(12).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "12:00:00");
}

#[test]
fn plain_time_range_validation() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "try { new Temporal.PlainTime(24, 0, 0) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
    let r = eval(&mut vm, "try { new Temporal.PlainTime(0, 60, 0) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
}
