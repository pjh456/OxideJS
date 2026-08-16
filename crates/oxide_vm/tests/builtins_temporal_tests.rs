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
fn instant_add_and_subtract_are_exact() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let i = Temporal.Instant.fromEpochNanoseconds(1582966647747612578n);
         i.add({hours: 1, microseconds: 9007199254740991}).epochNanoseconds === 10590169502488603578n
           && i.subtract('PT1.03125H').epochNanoseconds === 1582962935247612578n",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_add_ignores_receiver_subclass_for_result() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "class Sub extends Temporal.Instant { constructor(value) { super(value); } }
         let result = new Sub(10n).add({nanoseconds: 5});
         result instanceof Temporal.Instant && !(result instanceof Sub) && result.epochNanoseconds === 15n",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_arithmetic_rejects_date_units_and_out_of_range_results() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let max = Temporal.Instant.fromEpochNanoseconds(8640000000000000000000n);
         (() => { try { max.add({nanoseconds: 1}); return false } catch (e) { return e instanceof RangeError } })()
           && (() => { try { max.subtract({days: 1}); return false } catch (e) { return e instanceof RangeError } })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_round_supports_all_rounding_directions() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let i = new Temporal.Instant(-1500n);
         i.round({smallestUnit:'microsecond', roundingMode:'floor'}).epochNanoseconds === -2000n
           && i.round({smallestUnit:'microsecond', roundingMode:'ceil'}).epochNanoseconds === -1000n
           && i.round({smallestUnit:'microsecond', roundingMode:'halfFloor'}).epochNanoseconds === -2000n
           && i.round({smallestUnit:'microsecond', roundingMode:'halfCeil'}).epochNanoseconds === -1000n
           && i.round({smallestUnit:'microsecond', roundingMode:'halfEven'}).epochNanoseconds === -2000n",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_round_validates_increment_and_accepts_shorthand() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let i = new Temporal.Instant(123456789n);
         i.round('microseconds').epochNanoseconds === 123457000n
           && i.round({smallestUnit:'nanosecond', roundingIncrement:2.5, roundingMode:'expand'}).epochNanoseconds === 123456790n
           && (() => { try { i.round({smallestUnit:'hour', roundingIncrement:7}); return false } catch (e) { return e instanceof RangeError } })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_round_accepts_plural_units_inside_callbacks() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let i = new Temporal.Instant(1000000000123456789n), ok = true;
         ['hour','minute','second','millisecond','microsecond','nanosecond'].forEach(unit => {
           ok = ok && i.round({smallestUnit:unit}).equals(i.round({smallestUnit:`${unit}s`}));
         });
         ok",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_round_handles_shorthand_callback_parameters() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let i = new Temporal.Instant(1n), shorthand = true;
         ['hour'].forEach(smallestUnit => {
           shorthand = i.round({smallestUnit}).equals(i.round(smallestUnit));
         });
         shorthand",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_round_handles_nested_destructured_callback_captures() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let i = new Temporal.Instant(1n);
         let pairs = [['hour', [1, 2]]], captured = true;
         pairs.forEach(([unit, increments]) => {
           increments.forEach(increment => {
             captured = captured && i.round({smallestUnit: unit, roundingIncrement: increment}) instanceof Temporal.Instant;
           });
         });
         captured",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_until_and_since_balance_exact_differences() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let earlier = new Temporal.Instant(1000000000000000000n);
         let later = new Temporal.Instant(1000090061987654321n);
         let exact = earlier.until(later);
         let hours = later.since(earlier, { largestUnit: 'hours' });
         exact.seconds === 90061 && exact.milliseconds === 987
           && exact.microseconds === 654 && exact.nanoseconds === 321
           && hours.hours === 25 && hours.minutes === 1 && hours.seconds === 1
           && hours.milliseconds === 987 && hours.microseconds === 654 && hours.nanoseconds === 321",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_difference_rounds_signed_values_and_validates_units() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let zero = new Temporal.Instant(0n), value = new Temporal.Instant(7199000000000n);
         let positive = zero.until(value, { largestUnit: 'hours', smallestUnit: 'minutes', roundingMode: 'expand' });
         let negative = zero.since(value, { largestUnit: 'hours', smallestUnit: 'minutes', roundingMode: 'expand' });
         positive.hours === 2 && positive.minutes === 0
           && negative.hours === -2 && negative.minutes === 0
           && (() => { try { zero.until(value, { largestUnit: 'seconds', smallestUnit: 'hours' }); return false }
                        catch (e) { return e instanceof RangeError } })()
           && (() => { try { zero.until(value, { smallestUnit: 'minutes', roundingIncrement: 60 }); return false }
                        catch (e) { return e instanceof RangeError } })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_to_string_supports_precision_rounding_and_offsets() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let instant = new Temporal.Instant(1000000000123987500n);
         instant.toString({smallestUnit:'microsecond', roundingMode:'halfEven'}) === '2001-09-09T01:46:40.123988Z'
           && instant.toString({fractionalSecondDigits:3}) === '2001-09-09T01:46:40.123Z'
           && new Temporal.Instant(0n).toString({timeZone:'-05:00'}) === '1969-12-31T19:00:00-05:00'
           && new Temporal.Instant(999999960000000000n).toString({smallestUnit:'minute'}) === '2001-09-09T01:46Z'",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_json_and_locale_string_use_default_iso_output() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let instant = new Temporal.Instant(30123400000n);
         instant.toJSON({get ignored(){throw new Error()}}) === '1970-01-01T00:00:30.1234Z'
           && typeof instant.toLocaleString('en', {dateStyle:'short'}) === 'string'",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_to_string_tag_has_temporal_descriptor() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let descriptor = Object.getOwnPropertyDescriptor(Temporal.Instant.prototype, Symbol.toStringTag);
         Object.prototype.toString.call(Temporal.Instant.prototype) === '[object Temporal.Instant]'
           && descriptor.value === 'Temporal.Instant'
           && descriptor.writable === false
           && descriptor.enumerable === false
           && descriptor.configurable === true",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn instant_to_zoned_date_time_iso_preserves_epoch_and_time_zone() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let instant = new Temporal.Instant(1000000000000000000n);
         let utc = instant.toZonedDateTimeISO('uTc');
         let offset = instant.toZonedDateTimeISO('2021-08-19T17:30-07:00');
         let constructed = new Temporal.ZonedDateTime(0n, '+01:30');
         utc.epochNanoseconds === instant.epochNanoseconds
           && utc.timeZoneId === 'UTC'
           && utc.calendarId === 'iso8601'
           && offset.timeZoneId === '-07:00'
           && constructed.epochNanoseconds === 0n
           && constructed.timeZoneId === '+01:30'
           && Object.prototype.toString.call(constructed) === '[object Temporal.ZonedDateTime]'",
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

// -- Temporal.PlainDateTime --

#[test]
fn plain_date_time_constructor_and_components() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let dt = new Temporal.PlainDateTime(2024, 2, 29, 23, 58, 57, 987, 654, 321);
         dt.year + ',' + dt.month + ',' + dt.day + ',' + dt.hour + ',' + dt.minute + ',' +
         dt.second + ',' + dt.millisecond + ',' + dt.microsecond + ',' + dt.nanosecond + ',' + dt.calendarId",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2024,2,29,23,58,57,987,654,321,iso8601");
}

#[test]
fn plain_date_time_defaults_time_to_midnight() {
    let mut vm = Vm::new();
    assert_eq!(
        num(
            &mut vm,
            "let dt = new Temporal.PlainDateTime(2024, 7, 6); dt.hour + dt.minute + dt.second + dt.nanosecond",
        ),
        0.0
    );
}

#[test]
fn plain_date_time_splits_into_plain_date_and_time() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let dt = new Temporal.PlainDateTime(2000, 5, 2, 12, 34, 56, 789, 123, 456);
         dt.toPlainDate().toString() + 'T' + dt.toPlainTime().toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2000-05-02T12:34:56.789123456");
}

#[test]
fn plain_date_time_validates_brand_and_ranges() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let getter = Object.getOwnPropertyDescriptor(Temporal.PlainDateTime.prototype, 'year').get;
         let a; let b; let c;
         try { getter.call({}) } catch (e) { a = e.constructor.name }
         try { new Temporal.PlainDateTime(2023, 2, 29) } catch (e) { b = e.constructor.name }
         try { new Temporal.PlainDateTime(2024, 1, 1, 24) } catch (e) { c = e.constructor.name }
         a + ',' + b + ',' + c",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "TypeError,RangeError,RangeError");
}

#[test]
fn plain_date_time_metadata_and_value_of() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let dt = new Temporal.PlainDateTime(2024, 1, 2);
         let error;
         try { dt.valueOf() } catch (e) { error = e.constructor.name }
         Temporal.PlainDateTime.length + ',' + (dt instanceof Temporal.PlainDateTime) + ',' +
         Object.prototype.toString.call(dt) + ',' + error",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "3,true,[object Temporal.PlainDateTime],TypeError");
}

#[test]
fn plain_date_time_from_instances_strings_and_objects() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let original = new Temporal.PlainDateTime(1976, 11, 18, 15, 23, 30, 1, 123, 456);
         let copy = Temporal.PlainDateTime.from(original);
         let parsed = Temporal.PlainDateTime.from('1976-11-18T15:23:30.001123456');
         let bag = Temporal.PlainDateTime.from({year: 1976, monthCode: 'M11', day: 18, hour: 15});
         (copy !== original) + ',' + parsed.toString() + ',' + bag.toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true,1976-11-18T15:23:30.001123456,1976-11-18T15:00:00");
}

#[test]
fn plain_date_time_compare_uses_internal_slots() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "let earlier = new Temporal.PlainDateTime(2000, 5, 2, 12, 34, 56);
         let later = new Temporal.PlainDateTime(2000, 5, 2, 12, 34, 57);
         Object.defineProperty(earlier, 'year', {get() { throw new Error('getter') }});
         Object.defineProperty(later, 'year', {get() { throw new Error('getter') }});
         Temporal.PlainDateTime.compare(earlier, later) + ',' +
         Temporal.PlainDateTime.compare(later, earlier) + ',' +
         Temporal.PlainDateTime.compare(earlier, earlier)",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "-1,1,0");
}

#[test]
fn plain_date_time_from_accepts_variants_and_full_range() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "const lower = Temporal.PlainDateTime.from('1976-11-18t15:23').toString();
         const space = Temporal.PlainDateTime.from('1976-11-18 15:23').toString();
         const offset = Temporal.PlainDateTime.from('1976-11-18T15:23+00:00:00,0[UTC]').toString();
         const criticalZone = Temporal.PlainDateTime.from('1976-11-18T15:23[!Europe/Vienna]').toString();
         const secondCalendar = Temporal.PlainDateTime.from('1976-11-18T15:23[u-ca=iso8601][u-ca=discord]').toString();
         const minOk = Temporal.PlainDateTime.from('-271821-04-19T00:00:00.000000001').toString();
         const maxOk = Temporal.PlainDateTime.from('+275760-09-13').toString();
         const minBad = (() => { try { Temporal.PlainDateTime.from('-271821-04-19T00:00'); return false }
             catch (e) { return e instanceof RangeError } })();
         const maxBad = (() => { try { Temporal.PlainDateTime.from('+275760-09-14'); return false }
             catch (e) { return e instanceof RangeError } })();
         const dateOffsetBad = (() => { try { Temporal.PlainDateTime.from('2022-09-15+00:00'); return false }
             catch (e) { return e instanceof RangeError } })();
         const badCalendar = (() => { try { Temporal.PlainDateTime.from('1997-12-04[u-ca=notacal]'); return false }
             catch (e) { return e instanceof RangeError } })();
         const duplicateCritical = (() => { try { Temporal.PlainDateTime.from('1970-01-01[u-ca=iso8601][!u-ca=iso8601]'); return false }
             catch (e) { return e instanceof RangeError } })();
         [lower, space, offset, criticalZone, secondCalendar, minOk, maxOk,
           minBad, maxBad, dateOffsetBad, badCalendar, duplicateCritical].join('|')",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        concat!(
            "1976-11-18T15:23:00|1976-11-18T15:23:00|1976-11-18T15:23:00|",
            "1976-11-18T15:23:00|1976-11-18T15:23:00|-271821-04-19T00:00:00.000000001|",
            "+275760-09-13T00:00:00|true|true|true|true|true"
        )
    );
}
#[test]
fn plain_date_time_to_string_honors_options() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(() => {
           const dt = new Temporal.PlainDateTime(2000, 5, 2, 12, 34, 56, 987, 650, 0);
           const minute = dt.toString({ smallestUnit: 'minute' });
           const seconds = dt.toString({ smallestUnit: 'second' });
           const micros = dt.toString({ smallestUnit: 'microsecond' });
           const two = dt.toString({ fractionalSecondDigits: 2.5 });
           const nine = dt.toString({ fractionalSecondDigits: 9.7 });
           const midnight = new Temporal.PlainDateTime(1999, 12, 31, 23, 59, 59, 999, 999, 999)
             .toString({ fractionalSecondDigits: 8, roundingMode: 'ceil' });
           const always = dt.toString({ calendarName: 'always' });
           const critical = dt.toString({ calendarName: 'critical' });
           const never = dt.toString({ calendarName: 'never' });
           const badCalendar = (() => { try { dt.toString({ calendarName: 'ALWAYS' }); return false }
             catch (e) { return e instanceof RangeError } })();
           const badUnit = (() => { try { dt.toString({ smallestUnit: 'hour' }); return false }
             catch (e) { return e instanceof RangeError } })();
           const badDigits = (() => { try { dt.toString({ fractionalSecondDigits: -0.6 }); return false }
             catch (e) { return e instanceof RangeError } })();
           const badOptions = (() => { try { dt.toString(null); return false }
             catch (e) { return e instanceof TypeError } })();
           return [minute, seconds, micros, two, nine, midnight, always, critical, never,
             badCalendar, badUnit, badDigits, badOptions].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        concat!(
            "2000-05-02T12:34|2000-05-02T12:34:56|2000-05-02T12:34:56.987650|",
            "2000-05-02T12:34:56.98|2000-05-02T12:34:56.987650000|2000-01-01T00:00:00.00000000|",
            "2000-05-02T12:34:56.98765[u-ca=iso8601]|2000-05-02T12:34:56.98765[!u-ca=iso8601]|",
            "2000-05-02T12:34:56.98765|true|true|true|true"
        )
    );
}
#[test]
fn plain_date_time_serializes_extended_years() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "new Temporal.PlainDateTime(-1, 8, 7, 6, 54, 32, 100).toString() + ',' +
         new Temporal.PlainDateTime(10000, 6, 7, 8, 9, 10, 987).toJSON()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "-000001-08-07T06:54:32.1,+010000-06-07T08:09:10.987");
}

#[test]
fn zdt_subclass_compare_debug() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "class AvoidGettersDateTime extends Temporal.PlainDateTime {
           get year() { throw new Error('getter'); }
         }
         const one = new AvoidGettersDateTime(2000, 5, 2, 12, 34, 56, 987, 654, 321);
         const two = new AvoidGettersDateTime(2006, 3, 25, 6, 54, 32, 123, 456, 789);
         typeof Temporal.PlainDateTime.compare(one, two) + ':' + Temporal.PlainDateTime.compare(one, two)",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "number:-1");
}

#[test]
fn plain_date_time_add_balances_months_days_and_time() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "const a = Temporal.PlainDateTime.from('1997-12-01T12:34');
         const b = a.add(new Temporal.Duration(3, 6, 0, 17));
         const c = a.add(new Temporal.Duration(0, 1, 0, 0, 36));
         const d = a.subtract(new Temporal.Duration(0, 0, 0, 1));
         const constrained = new Temporal.PlainDateTime(2020, 1, 31, 15, 0).add({ months: 1 });
         const rejected = (() => { try {
           new Temporal.PlainDateTime(2020, 1, 31, 15, 0).add({ months: 1 }, { overflow: 'reject' });
           return false;
         } catch (e) { return e instanceof RangeError; } })();
         [b.year, b.month, b.day, b.hour, b.minute, b.second,
          c.day, c.hour,
          d.year, d.month, d.day,
          constrained.month, constrained.day, rejected].join(':')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2001:6:18:12:34:0:3:0:1997:11:30:2:29:true");
}

#[test]
fn plain_date_constructor_stores_real_calendar() {
    let mut vm = Vm::new();
    // 规范日历 ID 存槽：大小写折叠为规范小写。
    let r = eval(
        &mut vm,
        "new Temporal.PlainDate(2000, 5, 2, 'gregory').calendarId + ',' +
         new Temporal.PlainDate(2000, 5, 2, 'iSo8601').calendarId + ',' +
         new Temporal.PlainDate(2000, 5, 2).calendarId",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "gregory,iso8601,iso8601");
}

#[test]
fn plain_date_constructor_rejects_invalid_calendar() {
    let mut vm = Vm::new();
    // 未在白名单 → RangeError；ISO 串拒绝；对象（非 Temporal 实例）→ TypeError。
    let r = eval(
        &mut vm,
        "(() => { try { new Temporal.PlainDate(2000, 5, 2, 'notacal'); return false; }
           catch (e) { return e instanceof RangeError; } })() + ',' +
         (() => { try { new Temporal.PlainDate(2000, 5, 2, '1997-12-04[u-ca=iso8601]'); return false; }
           catch (e) { return e instanceof RangeError; } })() + ',' +
         (() => { try { new Temporal.PlainDate(2000, 5, 2, {}); return false; }
           catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true,true,true");
}

#[test]
fn plain_date_time_constructor_stores_real_calendar() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "new Temporal.PlainDateTime(2000, 5, 2, 12, 0, 0, 0, 0, 0, 'hebrew').calendarId + ',' +
         new Temporal.PlainDateTime(2000, 5, 2).calendarId",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "hebrew,iso8601");
}

#[test]
fn plain_date_time_constructor_wrong_calendar_is_type_error() {
    let mut vm = Vm::new();
    // 规范要求 `{}` 日历 → TypeError（strict 走日历 ID 校验而非宽松 to-option-string）。
    let r = eval(
        &mut vm,
        "(() => { try { new Temporal.PlainDateTime(2000, 5, 2, 12, 0, 0, 0, 0, 0, {}); return false; }
           catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn zoned_date_time_constructor_stores_real_calendar() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "new Temporal.ZonedDateTime(0n, 'UTC', 'hebrew').calendarId + ',' +
         new Temporal.ZonedDateTime(0n, 'UTC').calendarId",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "hebrew,iso8601");
}

#[test]
fn zoned_date_time_constructor_wrong_calendar_is_type_error() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "(() => { try { new Temporal.ZonedDateTime(0n, 'UTC', {}); return false; }
           catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn zoned_date_time_epoch_getters() {
    let mut vm = Vm::new();
    // 正负 epoch 的秒/毫秒/微秒除法：负值向下取整（floor）。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(217175010123456789n, 'UTC');
         const n = new Temporal.ZonedDateTime(-217175010876543211n, 'UTC');
         z.epochSeconds + ',' + z.epochMilliseconds + ',' + z.epochMicroseconds + ',' +
         n.epochSeconds + ',' + n.epochMilliseconds + ',' + n.epochMicroseconds",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "217175010,217175010123,217175010123456,-217175011,-217175010877,-217175010876544"
    );
}

#[test]
fn zoned_date_time_calendar_getters() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         z.year + ',' + z.month + ',' + z.day + ',' + z.hour + ',' + z.minute + ',' +
         z.second + ',' + z.millisecond + ',' + z.microsecond + ',' + z.nanosecond",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1970,1,1,0,0,0,0,0,0");
}

#[test]
fn zoned_date_time_offset_balance_negative_time_units() {
    let mut vm = Vm::new();
    // 负偏移跨日：60_000_000_001n + "-00:02" → 本地 23:59:00.000000001。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(60_000_000_001n, '-00:02');
         z.minute + ',' + z.second + ',' + z.millisecond + ',' + z.microsecond + ',' + z.nanosecond",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "59,0,0,0,1");
}

#[test]
fn zoned_date_time_balance_negative_day() {
    let mut vm = Vm::new();
    // 负偏移跨日到前一天：86_400_000_000_001n + "-00:02" → day 1（1970-01-01）/ 23:58。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(86_400_000_000_001n, '-00:02');
         z.day + ',' + z.hour + ',' + z.minute",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1,23,58");
}

#[test]
fn zoned_date_time_negative_epoch_millisecond() {
    let mut vm = Vm::new();
    // 负 epoch floor 语义（镜像 epochMilliseconds/basic.js）→ 本地时分使 ms 为 0。
    let r = eval(&mut vm, "new Temporal.ZonedDateTime(-13_849_764_999_999_999n, 'UTC').millisecond === 0").unwrap();
    assert!(r.as_bool());
}

#[test]
fn zoned_date_time_week_fields() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(217178610123456789n, 'UTC');
         const z = new Temporal.ZonedDateTime(0n, 'UTC');
         const y = new Temporal.ZonedDateTime(-4n*864000000000000n, 'UTC');
         a.dayOfWeek + ',' + a.dayOfYear + ',' + a.daysInMonth + ',' + a.inLeapYear + ',' +
         z.weekOfYear + ',' + z.yearOfWeek + ',' + y.yearOfWeek",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "4,323,30,true,1,1970,1969");
}

#[test]
fn zoned_date_time_offset_and_offset_nanoseconds() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "new Temporal.ZonedDateTime(0n, '+01:00').offset + ',' +
         new Temporal.ZonedDateTime(0n, 'UTC').offset + ',' +
         new Temporal.ZonedDateTime(0n, '-05:00').offset + ',' +
         new Temporal.ZonedDateTime(0n, '+01:00').offsetNanoseconds + ',' +
         new Temporal.ZonedDateTime(0n, '-05:00').offsetNanoseconds",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "+01:00,+00:00,-05:00,3600000000000,-18000000000000");
}

#[test]
fn zoned_date_time_constants_and_month_code() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         z.era + ',' + z.eraYear + ',' + z.monthCode + ',' + z.daysInWeek + ',' + z.monthsInYear",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "undefined,undefined,M01,7,12");
}

#[test]
fn zoned_date_time_m0_plus_hh_offset_and_hours_in_day() {
    let mut vm = Vm::new();
    // M0：+01 基本偏移可构造，且 offset 仍规范化、hoursInDay 恒 24。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, '+01');
         z.timeZoneId + ',' + z.offset + ',' + z.hoursInDay",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "+01,+01:00,24");
}

#[test]
fn zoned_date_time_hours_in_day_out_of_range() {
    let mut vm = Vm::new();
    // 今日/明日当地午夜越 Instant 界 → hoursInDay 抛 RangeError。
    let r = eval(
        &mut vm,
        "(() => { try { new Temporal.ZonedDateTime(-864n*10n**19n, '-01').hoursInDay; return false; }
           catch (e) { return e instanceof RangeError; } })()
         && (() => { try { new Temporal.ZonedDateTime(864n*10n**19n, 'UTC').hoursInDay; return false; }
           catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn zoned_date_time_getters_branding_type_error() {
    let mut vm = Vm::new();
    // 非 ZDT receiver 调 getter 一律抛 TypeError（branding）。
    let r = eval(
        &mut vm,
        "(() => { try { Temporal.ZonedDateTime.prototype.year.call({}); return false; }
           catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn temporal_instance_as_calendar_fast_path_reads_slot() {
    let mut vm = Vm::new();
    // Temporal 实例作 property bag 的 calendar：直接读内部槽，不触发 calendar 属性 getter。
    let r = eval(
        &mut vm,
        "const pd = new Temporal.PlainDate(2000, 5, 2, 'gregory');
         const pdt = new Temporal.PlainDateTime(2000, 5, 2, 12, 0, 0, 0, 0, 0, 'hebrew');
         const zdt = new Temporal.ZonedDateTime(0n, 'UTC', 'japanese');
         Object.defineProperty(pd, 'calendar', { get() { throw new Error('getter'); } });
         Object.defineProperty(pdt, 'calendar', { get() { throw new Error('getter'); } });
         Object.defineProperty(zdt, 'calendar', { get() { throw new Error('getter'); } });
         const a = Temporal.PlainDate.from({ year: 2000, month: 5, day: 2, calendar: pd }).calendarId;
         const b = Temporal.PlainDateTime.from({ year: 2000, month: 5, day: 2, hour: 12, calendar: pdt }).calendarId;
         const c = Temporal.PlainDateTime.from({ year: 2000, month: 5, day: 2, hour: 12, calendar: zdt }).calendarId;
         a + ',' + b + ',' + c",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "gregory,hebrew,japanese");
}

#[test]
fn plain_date_from_instance_preserves_calendar() {
    let mut vm = Vm::new();
    // from(实例) 复制日历槽；add/subtract 与 toPlainDate 传播日历。
    let r = eval(
        &mut vm,
        "const pd = new Temporal.PlainDate(2000, 5, 2, 'gregory');
         const pdt = new Temporal.PlainDateTime(2000, 5, 2, 12, 0, 0, 0, 0, 0, 'hebrew');
         Temporal.PlainDate.from(pd).calendarId + ',' +
         Temporal.PlainDateTime.from(pd).calendarId + ',' +
         Temporal.PlainDateTime.from(pdt).calendarId + ',' +
         pdt.toPlainDate().calendarId + ',' +
         pd.add({ days: 1 }).calendarId + ',' +
         pd.subtract({ days: 1 }).calendarId + ',' +
         pdt.add({ days: 1 }).calendarId",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "gregory,gregory,hebrew,hebrew,gregory,gregory,hebrew");
}

#[test]
fn plain_date_brand_only_getters_require_plain_date_receiver() {
    let mut vm = Vm::new();
    // 4 个品牌-only getter 对普通对象 receiver 必须抛 TypeError。
    // 经 property descriptor 取真实 getter 函数（原型上的 `.get` 访问不可靠，
    // 会拿到 undefined），修复前只查 is_object 不查品牌 → 返回常量不抛，断言失败。
    for getter in ["daysInWeek", "monthsInYear", "era", "eraYear"] {
        let r = eval(
            &mut vm,
            &format!(
                "var g = Object.getOwnPropertyDescriptor(Temporal.PlainDate.prototype, '{getter}').get; \
                 try {{ g.call({{}}); 'no-throw' }} catch (e) {{ e.constructor.name }}"
            ),
        )
        .unwrap();
        assert_eq!(str_val(&vm, r), "TypeError", "{getter} on plain object must throw");
    }
    // 正常 PlainDate receiver 不回归。
    let r = eval(
        &mut vm,
        "var p = new Temporal.PlainDate(2000, 5, 2); \
         [p.daysInWeek, p.monthsInYear, p.era === undefined, p.eraYear === undefined].join(',')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "7,12,true,true");
}

#[test]
fn zoned_date_time_to_string_default() {
    let mut vm = Vm::new();
    // 默认无参输出：offset 恒显示 + timeZoneName auto 显示，calendarName auto 省略。
    let r = eval(
        &mut vm,
        "new Temporal.ZonedDateTime(0n, 'UTC').toString() + ',' +
         new Temporal.ZonedDateTime(0n, '+01:00').toString() + ',' +
         new Temporal.ZonedDateTime(0n, '-05:00').toString()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1970-01-01T00:00:00+00:00[UTC],1970-01-01T01:00:00+01:00[+01:00],1969-12-31T19:00:00-05:00[-05:00]"
    );
}

#[test]
fn zoned_date_time_to_string_options() {
    let mut vm = Vm::new();
    // 各 options 组合：offset 省略 / calendarName always / timeZoneName never / critical 前缀。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, '+01:00');
         z.toString({timeZoneName:'never'}) + '|' +
         z.toString({calendarName:'always'}) + '|' +
         z.toString({timeZoneName:'critical'}) + '|' +
         z.toString({calendarName:'critical'}) + '|' +
         z.toString({offset:'never'}) + '|' +
         z.toString({offset:'critical'}) + '|' +
         z.toString({timeZoneName:'never', calendarName:'never', offset:'never'})",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1970-01-01T01:00:00+01:00|1970-01-01T01:00:00+01:00[+01:00][u-ca=iso8601]|1970-01-01T01:00:00+01:00[!+01:00]|1970-01-01T01:00:00+01:00[+01:00][!u-ca=iso8601]|1970-01-01T01:00:00[+01:00]|1970-01-01T01:00:00!+01:00[+01:00]|1970-01-01T01:00:00"
    );
}

#[test]
fn zoned_date_time_to_string_epoch_rounding_cross_midnight() {
    let mut vm = Vm::new();
    // 舍入在 epoch 域：2000-01-01 前 1ns + fractionalSecondDigits:8 + halfExpand → 跨午夜进位。
    let r = eval(
        &mut vm,
        "new Temporal.ZonedDateTime(946_684_799_999_999_999n, 'UTC')
            .toString({fractionalSecondDigits:8, roundingMode:'halfExpand'})",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2000-01-01T00:00:00.00000000+00:00[UTC]");
}

#[test]
fn zoned_date_time_to_string_negative_epoch_rounding() {
    let mut vm = Vm::new();
    // 负 epoch 舍入：halfCeil 上取整到毫秒。
    let r = eval(
        &mut vm,
        "new Temporal.ZonedDateTime(-999999999999999990n, 'UTC')
            .toString({smallestUnit:'millisecond', roundingMode:'halfCeil'})",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1938-04-24T22:13:20.000+00:00[UTC]");
}

#[test]
fn zoned_date_time_to_string_smallest_unit() {
    let mut vm = Vm::new();
    // smallestUnit minute 省略秒段。
    let r = eval(
        &mut vm,
        "new Temporal.ZonedDateTime(3661_000_000_000n, 'UTC').toString({smallestUnit:'minute'})",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1970-01-01T01:01+00:00[UTC]");
}

#[test]
fn zoned_date_time_to_json_and_locale_and_value_of() {
    let mut vm = Vm::new();
    // toJSON === toString()；toLocaleString === toString()；valueOf 抛 TypeError。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, '+01:00');
         z.toJSON() + '|' + z.toLocaleString() + '|' +
         (z.toJSON() === z.toString()) + '|' +
         (() => { try { z.valueOf(); return 'no-throw'; } catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1970-01-01T01:00:00+01:00[+01:00]|1970-01-01T01:00:00+01:00[+01:00]|true|true"
    );
}

#[test]
fn zoned_date_time_to_string_tag_and_branding() {
    let mut vm = Vm::new();
    // @@toStringTag 数据属性 + 非 ZDT receiver 调 toString 抛 TypeError。
    let r = eval(
        &mut vm,
        "Object.prototype.toString.call(new Temporal.ZonedDateTime(0n, 'UTC')) + '|' +
         (() => { try { Temporal.ZonedDateTime.prototype.toString.call({}); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "[object Temporal.ZonedDateTime]|true");
}

#[test]
fn zoned_date_time_to_string_options_validation() {
    let mut vm = Vm::new();
    // 非法 options 值抛 RangeError；options 非对象抛 TypeError。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         (() => { try { z.toString({offset:'bad'}); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { z.toString({timeZoneName:'always'}); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { z.toString({calendarName:'bad'}); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { z.toString(42); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true");
}

#[test]
fn zoned_date_time_with_time_zone_swaps_zone_keeps_instant() {
    let mut vm = Vm::new();
    // 换时区后 instant 保留、返回新对象、timeZoneId 更新；大小写不敏感 + ±HH 均可用。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         const w = z.withTimeZone('+01:00');
         w.timeZoneId + '|' + w.epochNanoseconds + '|' +
         (w !== z) + '|' + z.timeZoneId + '|' +
         new Temporal.ZonedDateTime(0n, 'uTc').withTimeZone('+01').timeZoneId",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "+01:00|0|true|UTC|+01");
}

#[test]
fn zoned_date_time_with_time_zone_bad_receiver_or_zone() {
    let mut vm = Vm::new();
    // 非 ZDT receiver 抛 TypeError；非法时区串抛 RangeError；非字符串抛 TypeError。
    let r = eval(
        &mut vm,
        "( () => { try { Temporal.ZonedDateTime.prototype.withTimeZone.call({}, 'UTC'); return 'no-throw'; }
                   catch (e) { return e instanceof TypeError; } })() + '|' +
         ( () => { try { new Temporal.ZonedDateTime(0n, 'UTC').withTimeZone('Not/AZone'); return 'no-throw'; }
                   catch (e) { return e instanceof RangeError; } })() + '|' +
         ( () => { try { new Temporal.ZonedDateTime(0n, 'UTC').withTimeZone(42); return 'no-throw'; }
                   catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true");
}

#[test]
fn zoned_date_time_equals_compares_epoch_zone_calendar() {
    let mut vm = Vm::new();
    // 同 instant/时区/日历 true；异 instant false；异时区 false；异日历 false。
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(0n, 'UTC');
         const b = new Temporal.ZonedDateTime(0n, 'UTC');
         const c = new Temporal.ZonedDateTime(1n, 'UTC');
         const d = new Temporal.ZonedDateTime(0n, '-05:00');
         const e = new Temporal.ZonedDateTime(0n, 'UTC', 'japanese');
         a.equals(b) + '|' + a.equals(c) + '|' + a.equals(d) + '|' + a.equals(e)",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|false|false|false");
}

#[test]
fn zoned_date_time_equals_receiver_branding_and_non_zoned_arg() {
    let mut vm = Vm::new();
    // 非 ZDT receiver 抛 TypeError；非 ZDT 对象参数返回 false。
    let r = eval(
        &mut vm,
        "( () => { try { Temporal.ZonedDateTime.prototype.equals.call({}, new Temporal.ZonedDateTime(0n, 'UTC')); return 'no-throw'; }
                   catch (e) { return e instanceof TypeError; } })() + '|' +
         new Temporal.ZonedDateTime(0n, 'UTC').equals({}) + '|' +
         new Temporal.ZonedDateTime(0n, 'UTC').equals('2021-01-01T00:00:00+00:00[UTC]')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|false|false");
}
