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

#[test]
fn plain_time_equals_same_and_different_fields() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 5).equals(new Temporal.PlainTime(12, 30, 5))").unwrap();
    assert!(r.as_bool());
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 5).equals(new Temporal.PlainTime(12, 30, 6))").unwrap();
    assert!(!r.as_bool());
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 5).equals('12:30:05')").unwrap();
    assert!(r.as_bool());
}

#[test]
fn plain_time_equals_invalid_argument_throws() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "try { new Temporal.PlainTime(12).equals(42) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
    let r = eval(
        &mut vm,
        "try { Temporal.PlainTime.prototype.equals.call({}) } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
}

#[test]
fn plain_time_compare_epoch_order() {
    let mut vm = Vm::new();
    assert_eq!(
        num(
            &mut vm,
            "Temporal.PlainTime.compare(new Temporal.PlainTime(12, 30), new Temporal.PlainTime(13, 30))"
        ),
        -1.0
    );
    assert_eq!(
        num(
            &mut vm,
            "Temporal.PlainTime.compare(new Temporal.PlainTime(12, 30), new Temporal.PlainTime(12, 30))"
        ),
        0.0
    );
    assert_eq!(
        num(
            &mut vm,
            "Temporal.PlainTime.compare(new Temporal.PlainTime(13, 30), new Temporal.PlainTime(12, 30))"
        ),
        1.0
    );
    assert_eq!(num(&mut vm, "Temporal.PlainTime.compare('23:59', '00:00')"), 1.0);
}

#[test]
fn plain_time_until_since_normal() {
    let mut vm = Vm::new();
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30).until(new Temporal.PlainTime(13, 30)).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "PT1H");
    let r = eval(&mut vm, "new Temporal.PlainTime(13, 30).since(new Temporal.PlainTime(12, 30)).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "PT1H");
    let r = eval(
        &mut vm,
        "new Temporal.PlainTime(12, 30).until(new Temporal.PlainTime(13, 30, 5, 500)).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "PT1H5.5S");
}

#[test]
fn plain_time_until_since_options_and_direction() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "new Temporal.PlainTime(12, 30).until(new Temporal.PlainTime(13, 35), { largestUnit: 'minute' }).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "PT65M");
    let r = eval(
        &mut vm,
        "new Temporal.PlainTime(12, 30).since(new Temporal.PlainTime(13, 35), { largestUnit: 'minute' }).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "-PT65M");
    let r = eval(
        &mut vm,
        "new Temporal.PlainTime(12, 30).until(new Temporal.PlainTime(13, 30), { smallestUnit: 'hour' }).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "PT1H");
}

#[test]
fn plain_time_add_subtract_normal() {
    let mut vm = Vm::new();
    // 同日内小时加/减与跨分钟进位。
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30).add({ hours: 1, minutes: 5 }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "13:35:00");
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30).subtract({ hours: 1, minutes: 5 }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "11:25:00");
    // 亚秒进位：秒进位到分钟。
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 59, 500).add({ milliseconds: 500 }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "12:31:00");
}

#[test]
fn plain_time_add_subtract_cross_midnight() {
    let mut vm = Vm::new();
    // 时间溢出跨午夜：rem_euclid 回 0-24 域。
    let r = eval(&mut vm, "new Temporal.PlainTime(23, 30).add({ hours: 1 }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "00:30:00");
    let r = eval(&mut vm, "new Temporal.PlainTime(00, 30).subtract({ hours: 1 }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "23:30:00");
    // 多天增量同样折叠回当日。
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30).add({ hours: 26 }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "14:30:00");
}

#[test]
fn plain_time_add_subtract_ignores_date_units_and_blank() {
    let mut vm = Vm::new();
    // 日期字段（days 及以上）对 PlainTime 忽略，不报错。
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30).add({ days: 3, hours: 1 }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "13:30:00");
    // blank duration（Duration 对象全零）：值不变的新对象。
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30).add(new Temporal.Duration()).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "12:30:00");
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30).subtract(new Temporal.Duration()).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "12:30:00");
    // 空对象无 duration 字段 → TypeError（符合 duration-like 字段要求）。
    let r = eval(&mut vm, "try { new Temporal.PlainTime(12, 30).add({}) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
}

#[test]
fn plain_time_add_subtract_invalid_argument() {
    let mut vm = Vm::new();
    // 非 duration-like 原始值抛 TypeError。
    let r = eval(&mut vm, "try { new Temporal.PlainTime(12).add(42) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
}

#[test]
fn plain_time_round_units() {
    let mut vm = Vm::new();
    // 亚秒舍入到秒（halfExpand 默认）。
    let r = eval(
        &mut vm,
        "new Temporal.PlainTime(12, 30, 5, 800).round({ smallestUnit: 'second' }).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "12:30:06");
    // 舍入到分钟：30:59 进位到 31。
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 59).round({ smallestUnit: 'minute' }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "12:31:00");
    // 舍入到小时：30:30 进位到 13。
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 30).round({ smallestUnit: 'hour' }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "13:00:00");
    // 字符串简写形式同效。
    let r = eval(&mut vm, "new Temporal.PlainTime(12, 30, 30).round('hour').toString()").unwrap();
    assert_eq!(str_val(&vm, r), "13:00:00");
}

#[test]
fn plain_time_round_cross_midnight() {
    let mut vm = Vm::new();
    // 23:59:59.9 舍入到秒 → 24:00:00 → 取模回 00:00:00。
    let r = eval(
        &mut vm,
        "new Temporal.PlainTime(23, 59, 59, 900).round({ smallestUnit: 'second' }).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "00:00:00");
    // 23:30 舍入到小时 → 24:00 → 00:00。
    let r = eval(&mut vm, "new Temporal.PlainTime(23, 30).round({ smallestUnit: 'hour' }).toString()").unwrap();
    assert_eq!(str_val(&vm, r), "00:00:00");
}

#[test]
fn plain_time_round_increment_and_mode() {
    let mut vm = Vm::new();
    // 15 分钟增量舍入：23:07:30 → 23:00（floor）。
    let r = eval(
        &mut vm,
        "new Temporal.PlainTime(23, 7, 30).round({ smallestUnit: 'minute', roundingIncrement: 15, roundingMode: 'floor' }).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "23:00:00");
    // 真因子校验：increment 必须整除最大增量且严格小于之。
    let r = eval(
        &mut vm,
        "try { new Temporal.PlainTime(12).round({ smallestUnit: 'minute', roundingIncrement: 60 }) } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
    let r = eval(
        &mut vm,
        "try { new Temporal.PlainTime(12).round({ smallestUnit: 'hour', roundingIncrement: 24 }) } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
}

#[test]
fn plain_time_round_invalid_round_to() {
    let mut vm = Vm::new();
    // roundTo undefined → TypeError。
    let r = eval(&mut vm, "try { new Temporal.PlainTime(12).round() } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "TypeError");
    // 对象缺 smallestUnit → RangeError。
    let r = eval(&mut vm, "try { new Temporal.PlainTime(12).round({}) } catch (e) { e.constructor.name }").unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
    // 非法最小单位 → RangeError。
    let r = eval(
        &mut vm,
        "try { new Temporal.PlainTime(12).round({ smallestUnit: 'day' }) } catch (e) { e.constructor.name }",
    )
    .unwrap();
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

#[test]
fn zoned_date_time_from_string_wall_and_exact_time() {
    let mut vm = Vm::new();
    // 无偏移注解：墙钟按注解时区换算；带偏移与注解一致：精确时刻（墙钟 - 偏移）。
    let r = eval(
        &mut vm,
        "Temporal.ZonedDateTime.from('1976-11-18T15:23:30[UTC]').toString() + '|' +
         Temporal.ZonedDateTime.from('1976-11-18T15:23:30[+01:00]').epochNanoseconds + '|' +
         Temporal.ZonedDateTime.from('1976-11-18T15:23:30+01:00[+01:00]').epochNanoseconds + '|' +
         Temporal.ZonedDateTime.from('1976-11-18T15:23:30+01:00[+01:00]').timeZoneId",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1976-11-18T15:23:30+00:00[UTC]|217175010000000000|217175010000000000|+01:00"
    );
}

#[test]
fn zoned_date_time_from_string_utc_designator_pins_exact_time() {
    let mut vm = Vm::new();
    // Z 使 offsetBehaviour 为 exact：epoch 恒为墙钟时刻，注解时区只改 timeZoneId。
    let r = eval(
        &mut vm,
        "Temporal.ZonedDateTime.from('1970-01-01T00:00Z[+01:00]').epochNanoseconds + '|' +
         Temporal.ZonedDateTime.from('1970-01-01T00:00Z[+01:00]').timeZoneId + '|' +
         Temporal.ZonedDateTime.from('1970-01-01T00:00Z[+01:00]').toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "0|+01:00|1970-01-01T01:00:00+01:00[+01:00]");
}

#[test]
fn zoned_date_time_from_string_requires_time_zone_annotation() {
    let mut vm = Vm::new();
    // 无时区注解（裸日期时间 / Z / 纯偏移）均抛 RangeError。
    let r = eval(
        &mut vm,
        "['1970-01-01T00:00', '1970-01-01T00:00Z', '1970-01-01T00:00+01:00'].every(
           s => { try { Temporal.ZonedDateTime.from(s); return false; }
                 catch (e) { return e instanceof RangeError; } })",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn zoned_date_time_from_string_offset_options() {
    let mut vm = Vm::new();
    // offset 选项覆盖 critical 标记：use 保留字符串偏移、ignore/prefer 保留墙钟。
    let r = eval(
        &mut vm,
        "const s = '2022-10-07T18:37-07:00[!UTC]';
         Temporal.ZonedDateTime.from(s, { offset: 'use' }).epochNanoseconds + '|' +
         Temporal.ZonedDateTime.from(s, { offset: 'ignore' }).epochNanoseconds + '|' +
         Temporal.ZonedDateTime.from(s, { offset: 'prefer' }).epochNanoseconds + '|' +
         (() => { try { Temporal.ZonedDateTime.from(s); return 'no-throw'; }
                 catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1665193020000000000|1665167820000000000|1665167820000000000|true");
}

#[test]
fn zoned_date_time_from_property_bag() {
    let mut vm = Vm::new();
    // 纯日期 bag 与完整分量 bag 的墙钟换算与 toString 对拍。
    let r = eval(
        &mut vm,
        "Temporal.ZonedDateTime.from({ year: 2000, month: 5, day: 2, timeZone: 'UTC' }).toString() + '|' +
         Temporal.ZonedDateTime.from({
           year: 2000, month: 5, day: 2, hour: 12, minute: 34, second: 56,
           millisecond: 987, microsecond: 654, nanosecond: 321, timeZone: 'UTC'
         }).toString() + '|' +
         Temporal.ZonedDateTime.from({ year: 2000, month: 5, day: 2, hour: 12, timeZone: '-05:00' }).toString()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2000-05-02T00:00:00+00:00[UTC]|2000-05-02T12:34:56.987654321+00:00[UTC]|2000-05-02T12:00:00-05:00[-05:00]"
    );
}

#[test]
fn zoned_date_time_from_property_bag_offset_conflict() {
    let mut vm = Vm::new();
    // bag 内 offset 与 timeZone 不一致：默认与显式 reject 均抛 RangeError。
    let r = eval(
        &mut vm,
        "const props = { year: 2021, month: 10, day: 28, offset: '-07:00', timeZone: '+01:00' };
         (() => { try { Temporal.ZonedDateTime.from(props); return 'no-throw'; }
                 catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { Temporal.ZonedDateTime.from(props, { offset: 'reject' }); return 'no-throw'; }
                 catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true");
}

#[test]
fn zoned_date_time_from_zdt_object_copies_slots() {
    let mut vm = Vm::new();
    // 复制 ZDT 对象：三槽相等且返回新对象；非法选项值校验仍执行。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(217175010123456789n, '+01:00', 'hebrew');
         const c = Temporal.ZonedDateTime.from(z);
         (c !== z) + '|' + c.epochNanoseconds + '|' + c.timeZoneId + '|' + c.calendarId + '|' +
         (() => { try { Temporal.ZonedDateTime.from(z, { offset: 'bad' }); return 'no-throw'; }
                 catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|217175010123456789|+01:00|hebrew|true");
}

#[test]
fn zoned_date_time_from_error_paths() {
    let mut vm = Vm::new();
    // 缺 timeZone 的 bag / 非字符串原始值 → TypeError；越界日期 → RangeError。
    let r = eval(
        &mut vm,
        "( () => { try { Temporal.ZonedDateTime.from({ year: 2000, month: 5, day: 2 }); return 'no-throw'; }
                   catch (e) { return e instanceof TypeError; } })() + '|' +
         [123, null, undefined, 1n].every(v => {
           try { Temporal.ZonedDateTime.from(v); return false; }
           catch (e) { return e instanceof TypeError; }
         }) + '|' +
         (() => { try { Temporal.ZonedDateTime.from({ year: -271821, month: 4, day: 19, timeZone: 'UTC' }); return 'no-throw'; }
                 catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true");
}

#[test]
fn zoned_date_time_from_options_validation() {
    let mut vm = Vm::new();
    // offset/disambiguation 非法 → RangeError；options 非对象 → TypeError。
    let r = eval(
        &mut vm,
        "const s = '1970-01-01T00:00[UTC]';
         (() => { try { Temporal.ZonedDateTime.from(s, { offset: 'garbage' }); return 'no-throw'; }
                 catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { Temporal.ZonedDateTime.from(s, { disambiguation: 'garbage' }); return 'no-throw'; }
                 catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { Temporal.ZonedDateTime.from(s, null); return 'no-throw'; }
                 catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true");
}

#[test]
fn zoned_date_time_compare_orders_by_epoch() {
    let mut vm = Vm::new();
    // 按 epoch 大小比较而非墙钟：epoch 大者胜，返回 -1/0/1。
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(217175010123456789n, 'UTC');
         const b = new Temporal.ZonedDateTime(217175010223456789n, 'UTC');
         Temporal.ZonedDateTime.compare(a, a) + '|' +
         Temporal.ZonedDateTime.compare(a, b) + '|' +
         Temporal.ZonedDateTime.compare(b, a)",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "0|-1|1");
}

#[test]
fn zoned_date_time_compare_ignores_time_zone_and_calendar() {
    let mut vm = Vm::new();
    // 同 epoch 不同时区/日历 → 0（只按 epoch 比较）。
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(0n, '+01:00', 'hebrew');
         const b = new Temporal.ZonedDateTime(0n, 'UTC');
         const c = new Temporal.ZonedDateTime(1n, '-05:00');
         Temporal.ZonedDateTime.compare(a, b) + '|' +
         Temporal.ZonedDateTime.compare(b, c)",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "0|-1");
}

#[test]
fn zoned_date_time_compare_accepts_string_and_bag() {
    let mut vm = Vm::new();
    // 字符串 / property bag 参数走 from 解析器归一为 epoch 后比较。
    let r = eval(
        &mut vm,
        "const zdt = new Temporal.ZonedDateTime(0n, 'UTC');
         Temporal.ZonedDateTime.compare('1970-01-01T00:00Z[UTC]', zdt) + '|' +
         Temporal.ZonedDateTime.compare(
           { year: 1969, month: 12, day: 31, hour: 19, minute: 0, timeZone: '-05:00' },
           zdt) + '|' +
         Temporal.ZonedDateTime.compare(zdt, '1970-01-01T00:00:01Z[UTC]')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "0|0|-1");
}

#[test]
fn zoned_date_time_compare_error_paths() {
    let mut vm = Vm::new();
    // 非法字符串 / 缺 timeZone 的 bag / 非转换原始值均抛错（RangeError/TypeError）。
    let r = eval(
        &mut vm,
        "( () => { try { Temporal.ZonedDateTime.compare('garbage', '1970-01-01T00:00Z[UTC]'); return 'no-throw'; }
                   catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { Temporal.ZonedDateTime.compare(
                          { year: 2000, month: 1, day: 1 },
                          '1970-01-01T00:00Z[UTC]'); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { Temporal.ZonedDateTime.compare(123, 456); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true");
}

#[test]
fn zoned_date_time_until_defaults_to_hours() {
    let mut vm = Vm::new();
    // 默认 largest = hour：epoch 差 217175010123456789n 分解为 60326h 23m 30.123456789s。
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(0n, 'UTC');
         const b = new Temporal.ZonedDateTime(217175010123456789n, 'UTC');
         a.until(b).toString() + '|' + a.until(b).hours + '|' +
         a.until(b, { largestUnit: 'auto' }).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "PT60326H23M30.123456789S|60326|PT60326H23M30.123456789S");
}

#[test]
fn zoned_date_time_until_since_are_negatives() {
    let mut vm = Vm::new();
    // since 为 until 的精确取反（同 tz 下对称）。
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(0n, '+01:00');
         const b = new Temporal.ZonedDateTime(217175010123456789n, '+01:00');
         b.until(a).toString() + '|' + a.since(b).toString() + '|' + b.since(a).toString()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "-PT60326H23M30.123456789S|-PT60326H23M30.123456789S|PT60326H23M30.123456789S"
    );
}

#[test]
fn zoned_date_time_until_same_epoch_blank() {
    let mut vm = Vm::new();
    // 同 epoch 不同 tz：epoch 相等快速路径返回空时长。
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(0n, 'UTC');
         const b = new Temporal.ZonedDateTime(0n, '+01:00');
         (a.until(b).toString() === 'PT0S') + '|' + a.until(b).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|PT0S");
}

#[test]
fn zoned_date_time_until_largest_unit_days() {
    let mut vm = Vm::new();
    // 显式 largestUnit：90 天 + 1 小时 1 秒按 days / years 分解。
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(0n, 'UTC');
         const b = new Temporal.ZonedDateTime(7779601000000000n, 'UTC');
         a.until(b, { largestUnit: 'days' }).toString() + '|' +
         a.until(b, { largestUnit: 'years' }).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "P90DT1H1S|P3MT1H1S");
}

#[test]
fn zoned_date_time_until_casts_argument() {
    let mut vm = Vm::new();
    // bag 缺时分秒字段默认 0；字符串带注解；均按 +01:00 墙钟换算 epoch（对拍 casts-argument.js）。
    let r = eval(
        &mut vm,
        "const zdt = Temporal.ZonedDateTime.from('1976-11-18T15:23:30.123456789+01:00[+01:00]');
         zdt.until({ year: 2019, month: 10, day: 29, hour: 10, timeZone: '+01:00' }).toString() + '|' +
         zdt.until('2019-10-29T10:46:38.271986102+01:00[+01:00]').toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "PT376434H36M29.876543211S|PT376435H23M8.148529313S");
}

#[test]
fn zoned_date_time_until_string_annotation_wall_and_exact() {
    let mut vm = Vm::new();
    // 注解无偏移 → 墙钟按注解时区；Z → exact；偏移与注解一致 → 精确时刻（对拍 zoneddatetime-string.js）。
    let r = eval(
        &mut vm,
        "const instance = new Temporal.ZonedDateTime(0n, 'UTC');
         instance.until('1970-01-01T00:00[+01:00]').toString() + '|' +
         instance.until('1970-01-01T00:00Z[+01:00]').toString() + '|' +
         instance.until('1970-01-01T00:00+01:00[+01:00]').toString() + '|' +
         (instance.until('1970-01-01T00:00[UTC]').toString() === 'PT0S')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "-PT1H|PT0S|-PT1H|true");
}

#[test]
fn zoned_date_time_until_rounding_options() {
    let mut vm = Vm::new();
    // roundingMode/roundingIncrement/smallestUnit 生效；非法增量抛 RangeError。
    let r = eval(
        &mut vm,
        "const a = new Temporal.ZonedDateTime(0n, 'UTC');
         const b = new Temporal.ZonedDateTime(3601000000000n, 'UTC');
         a.until(b, { smallestUnit: 'hours', roundingMode: 'ceil' }).toString() + '|' +
         a.until(b, { smallestUnit: 'hours', roundingMode: 'floor' }).toString() + '|' +
         a.until(b, { smallestUnit: 'hours', roundingIncrement: 2, roundingMode: 'halfExpand' }).toString() + '|' +
         (() => { try { a.until(b, { smallestUnit: 'hours', roundingIncrement: 24 }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { a.until(b, { smallestUnit: 'hours', roundingIncrement: 11 }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "PT2H|PT1H|PT2H|true|true");
}

#[test]
fn zoned_date_time_until_error_paths() {
    let mut vm = Vm::new();
    // 裸日期时间/纯偏移字符串 RangeError；空对象 TypeError；bag offset 冲突 RangeError。
    let r = eval(
        &mut vm,
        "const instance = new Temporal.ZonedDateTime(0n, 'UTC');
         ['1970-01-01T00:00', '1970-01-01T00:00Z', '1970-01-01T00:00+01:00', '-271821-04-19T23:00-01:00[-01:00]']
           .every(s => { try { instance.until(s); return false; } catch (e) { return e instanceof RangeError; } }) + '|' +
         (() => { try { instance.until({}); return 'no-throw'; } catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { instance.until({ year: 2021, month: 10, day: 28, offset: '-07:00', timeZone: '+01:00' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true");
}

#[test]
fn zoned_date_time_until_string_limits() {
    let mut vm = Vm::new();
    // 边界字符串：Instant 界内通过，墙钟日越界 / epoch 越界抛 RangeError（argument-string-limits.js）。
    let r = eval(
        &mut vm,
        "const instance = new Temporal.ZonedDateTime(0n, 'UTC');
         ['-271821-04-20T00:00Z[UTC]', '+275760-09-13T00:00Z[UTC]', '+275760-09-13T01:00+01:00[+01:00]']
           .every(s => { try { instance.until(s); return true; } catch (e) { return false; } }) + '|' +
         ['-271821-04-19T23:59:59.999999999Z[UTC]', '+275760-09-14T00:00+23:59[+23:59]', '+275760-09-13T00:00:00.000000001Z[UTC]']
           .every(s => { try { instance.until(s); return false; } catch (e) { return e instanceof RangeError; } })",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true");
}

#[test]
fn zoned_date_time_until_bag_default_time_zone() {
    let mut vm = Vm::new();
    // bag 缺 timeZone → 按 receiver 时区解释墙钟（+01:00 下 02:00 本地 = 01:00 UTC）；timeZone 非字符串 → TypeError。
    let r = eval(
        &mut vm,
        "const instance = new Temporal.ZonedDateTime(0n, '+01:00');
         instance.until({ year: 1970, month: 1, day: 1, hour: 2 }).toString() + '|' +
         (() => { try { instance.until({ year: 2021, month: 10, day: 28, timeZone: 42 }); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "PT1H|true");
}

// -- ZonedDateTime.prototype.with / withCalendar / withPlainTime --

#[test]
fn zoned_date_time_with_partial_merge() {
    let mut vm = Vm::new();
    // 单字段覆盖与多字段合并：未给字段沿用 receiver 本地分量。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         z.with({ year: 2019 }).toString() + '|' +
         z.with({ hour: 12, minute: 34, nanosecond: 5 }).toString() + '|' +
         z.with({ month: 5, second: 15 }).toString()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2019-01-01T00:00:00+00:00[UTC]|1970-01-01T12:34:00.000000005+00:00[UTC]|1970-05-01T00:00:15+00:00[UTC]"
    );
}

#[test]
fn zoned_date_time_with_undefined_fields_not_copied() {
    let mut vm = Vm::new();
    // year: undefined 不覆盖，其余字段覆盖（copy-properties-not-undefined 语义）。
    let r = eval(
        &mut vm,
        "const d1 = new Temporal.ZonedDateTime(1_000_000_000_000_000_789n, 'UTC');
         const d2 = d1.with({ day: 1, hour: 10, year: undefined });
         d2.year === 2001 && d2.month === 9 && d2.day === 1 && d2.hour === 10 &&
         d2.minute === 46 && d2.second === 40 && d2.nanosecond === 789",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn zoned_date_time_with_rejects_calendar_time_zone_and_temporal_objects() {
    let mut vm = Vm::new();
    // calendar/timeZone 字段、Temporal 实例参数、字符串参数均 TypeError。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         (() => { try { z.with({ month: 2, calendar: 'iso8601' }); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { z.with({ month: 2, timeZone: 'UTC' }); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { z.with(new Temporal.PlainDate(1976, 11, 18)); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { z.with('1976-11-18T12:00+00:00[UTC]'); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { z.with({}); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true|true");
}

#[test]
fn zoned_date_time_with_offset_option() {
    let mut vm = Vm::new();
    // offset 默认 prefer；use 用 bag 偏移；reject 冲突 RangeError；非法值各类型错误。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         const dt = new Temporal.ZonedDateTime(1572757201_000_000_000n, '-03:30');
         (dt.with({ minute: 31 }).epochNanoseconds === 1572757261_000_000_000n) + '|' +
         (dt.with({ minute: 31 }, {}).epochNanoseconds === 1572757261_000_000_000n) + '|' +
         (z.with({ offset: '+01:00' }, { offset: 'use' }).epochNanoseconds === -3_600_000_000_000n) + '|' +
         (z.with({ offset: '+01:00' }, { offset: 'prefer' }).epochNanoseconds === 0n) + '|' +
         (() => { try { z.with({ offset: '+01:00' }, { offset: 'reject' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { z.with({ offset: 0 }); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { z.with({ offset: '00:00' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true|true|true|true");
}

#[test]
fn zoned_date_time_with_overflow_constrain_and_reject() {
    let mut vm = Vm::new();
    // constrain 钳制月/日/时/亚秒；reject 越界 RangeError；monthCode 冲突/闰月 RangeError。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         z.with({ month: 29 }).toString() + '|' +
         z.with({ hour: 29 }).toString() + '|' +
         z.with({ nanosecond: 9000 }).toString() + '|' +
         (() => { try { z.with({ month: 29 }, { overflow: 'reject' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { z.with({ month: 5, monthCode: 'M06' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { z.with({ monthCode: 'M08L' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (z.with({ monthCode: 'M05' }).toString() === '1970-05-01T00:00:00+00:00[UTC]')",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
         "1970-12-01T00:00:00+00:00[UTC]|1970-01-01T23:00:00+00:00[UTC]|1970-01-01T00:00:00.000000999+00:00[UTC]|true|true|true|true"
    );
}

#[test]
fn zoned_date_time_with_month_code_constrain_day() {
    let mut vm = Vm::new();
    // monthCode 换月后日钳制：1 月 31 日 → 2 月 28 日（constrain）；reject → RangeError。
    let r = eval(
        &mut vm,
        "const z = Temporal.ZonedDateTime.from({ year: 2019, monthCode: 'M01', day: 31, hour: 12, minute: 34, timeZone: 'UTC' });
         z.with({ monthCode: 'M02' }).toString() + '|' +
         (() => { try { z.with({ monthCode: 'M02' }, { overflow: 'reject' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "2019-02-28T12:34:00+00:00[UTC]|true");
}

#[test]
fn zoned_date_time_with_range_errors_and_options_order() {
    let mut vm = Vm::new();
    // 越界墙钟 / offset use 越界 → RangeError；字段先于 options 校验。
    let r = eval(
        &mut vm,
        "( () => { try { new Temporal.ZonedDateTime(0n, 'UTC').with({ year: -271821, month: 4, day: 19, hour: 1 }); return 'no-throw'; }
                   catch (e) { return e instanceof RangeError; } })() + '|' +
         ( () => { try { new Temporal.ZonedDateTime(-864n * 10n**19n, 'UTC').with({ offset: '+01' }, { offset: 'use' }); return 'no-throw'; }
                   catch (e) { return e instanceof RangeError; } })() + '|' +
         ( () => { try { new Temporal.ZonedDateTime(0n, 'UTC').with({ day: 5 }, null); return 'no-throw'; }
                   catch (e) { return e instanceof TypeError; } })() + '|' +
         ( () => { try { new Temporal.ZonedDateTime(0n, 'UTC').with({ day: -1 }, null); return 'no-throw'; }
                   catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true");
}

#[test]
fn zoned_date_time_with_calendar_swaps_calendar_slot() {
    let mut vm = Vm::new();
    // 换日历槽：epoch/时区不变、返回新对象；大小写不敏感；ISO 串与时间串接受。
    let r = eval(
        &mut vm,
        "const c = new Temporal.ZonedDateTime(0n, 'UTC', 'hebrew');
         const w = c.withCalendar('iso8601');
         (w !== c) + '|' + w.calendarId + '|' + w.epochNanoseconds + '|' + w.timeZoneId + '|' +
         c.withCalendar('iSo8601').calendarId + '|' +
         c.withCalendar('2020-01-01').calendarId + '|' +
         c.withCalendar('15:23').calendarId + '|' +
         c.withCalendar(new Temporal.ZonedDateTime(0n, 'UTC', 'japanese')).calendarId",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|iso8601|0|UTC|iso8601|iso8601|iso8601|japanese");
}

#[test]
fn zoned_date_time_with_calendar_errors() {
    let mut vm = Vm::new();
    // 缺参/undefined/非字符串非对象 → TypeError；非法串 → RangeError。
    let r = eval(
        &mut vm,
        "const c = new Temporal.ZonedDateTime(0n, 'UTC');
         (() => { try { c.withCalendar(); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { c.withCalendar(undefined); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { c.withCalendar(42); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { c.withCalendar('notacal'); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true");
}

#[test]
fn zoned_date_time_with_plain_time_defaults_and_bag() {
    let mut vm = Vm::new();
    // undefined → 午夜；bag 缺省字段为 0；second=60 按 59 钳制。
    let r = eval(
        &mut vm,
        "const p = new Temporal.ZonedDateTime(957270896_987_654_321n, 'UTC');
         p.withPlainTime().toString() + '|' +
         p.withPlainTime(undefined).toString() + '|' +
         p.withPlainTime({ minute: 30 }).toString() + '|' +
         p.withPlainTime({ hour: 23, minute: 59, second: 60 }).toString() + '|' +
         p.withPlainTime(new Temporal.PlainTime(11, 22)).hour",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2000-05-02T00:00:00+00:00[UTC]|2000-05-02T00:00:00+00:00[UTC]|2000-05-02T00:30:00+00:00[UTC]|2000-05-02T23:59:59+00:00[UTC]|11"
    );
}

#[test]
fn zoned_date_time_with_plain_time_strings() {
    let mut vm = Vm::new();
    // 时间串 / 日期+时间 / 歧义串须 T 前缀 / Z 与纯日期拒绝 / offset 忽略。
    let r = eval(
        &mut vm,
        "const p = new Temporal.ZonedDateTime(957270896_987_654_321n, 'UTC');
         p.withPlainTime('12:34').toString() + '|' +
         p.withPlainTime('1976-11-18T15:23:30.123456789+00:00').toString() + '|' +
         p.withPlainTime('T2021-12').hour + '|' +
         (() => { try { p.withPlainTime('2021-12'); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { p.withPlainTime('09:00:00Z'); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { p.withPlainTime('2019-10-01'); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { p.withPlainTime({}); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2000-05-02T12:34:00+00:00[UTC]|2000-05-02T15:23:30.123456789+00:00[UTC]|20|true|true|true|true"
    );
}

#[test]
fn zoned_date_time_with_plain_time_zoned_date_time_argument() {
    let mut vm = Vm::new();
    // ZDT 参数用其自身时区取本地时间（负偏移平衡负时间单位）；负 epoch 模运算正确。
    let r = eval(
        &mut vm,
        "const dtz = new Temporal.ZonedDateTime(3661_001_001_001n, '-00:02');
         const r1 = new Temporal.ZonedDateTime(86400_000_000_000n, 'UTC').withPlainTime(dtz);
         (r1.hour === 0 && r1.minute === 59) + '|' +
         (new Temporal.ZonedDateTime(0n, 'UTC')
           .withPlainTime(new Temporal.ZonedDateTime(-13849764_999_999_999n, 'UTC'))
           .epochNanoseconds === 60635_000_000_001n)",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true");
}

#[test]
fn zoned_date_time_with_plain_time_out_of_range() {
    let mut vm = Vm::new();
    // 本地分量越界（±MAX 边界）→ RangeError（start-of-day 与 epoch 越界两条路径）。
    let r = eval(
        &mut vm,
        "( () => { try { new Temporal.ZonedDateTime(-864n * 10n**19n, '-01').withPlainTime(); return 'no-throw'; }
                   catch (e) { return e instanceof RangeError; } })() + '|' +
         ( () => { try { new Temporal.ZonedDateTime(-864n * 10n**19n, '+01').withPlainTime('00:00'); return 'no-throw'; }
                   catch (e) { return e instanceof RangeError; } })() + '|' +
         ( () => { try { new Temporal.ZonedDateTime(864n * 10n**19n, 'UTC').withPlainTime('01:00'); return 'no-throw'; }
                   catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true");
}

#[test]
fn zoned_date_time_with_family_branding() {
    let mut vm = Vm::new();
    // 非 ZDT receiver 调三个方法均 TypeError。
    let r = eval(
        &mut vm,
        "( () => { try { Temporal.ZonedDateTime.prototype.with.call({}, { year: 2019 }); return 'no-throw'; }
                   catch (e) { return e instanceof TypeError; } })() + '|' +
         ( () => { try { Temporal.ZonedDateTime.prototype.withCalendar.call({}, 'iso8601'); return 'no-throw'; }
                   catch (e) { return e instanceof TypeError; } })() + '|' +
         ( () => { try { Temporal.ZonedDateTime.prototype.withPlainTime.call({}); return 'no-throw'; }
                   catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true");
}

#[test]
fn zoned_date_time_add_duration_object() {
    let mut vm = Vm::new();
    // add-duration.js 对拍：240h + 800ns 叠加到负 epoch。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(-560174321098766n, 'UTC');
         z.add(new Temporal.Duration(0, 0, 0, 0, 240, 0, 0, 0, 0, 800)).epochNanoseconds === 303825678902034n",
    )
    .unwrap();
    assert!(r.as_bool());
}

#[test]
fn zoned_date_time_add_constrain_and_month_boundary() {
    let mut vm = Vm::new();
    // 月末 + 1 月 constrain 钳制到 2 月 28；reject 抛 RangeError；月份/日叠加顺序一致。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         z.with({ day: 31 }).add({ months: 1 }).toString() + '|' +
         (() => { try { z.with({ day: 31 }).add({ months: 1 }, { overflow: 'reject' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (z.add({ months: 1 }).add({ days: 1 }).epochNanoseconds === z.add({ months: 1, days: 1 }).epochNanoseconds)",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1970-02-28T00:00:00+00:00[UTC]|true|true");
}

#[test]
fn zoned_date_time_add_subtract_are_inverse() {
    let mut vm = Vm::new();
    // 同一 duration 符号反转互逆；blank duration 返回等值对象。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(1_700_000_000_123_456_789n, '+05:30');
         (z.add({ days: 5, hours: 3 }).subtract({ days: 5, hours: 3 }).epochNanoseconds === z.epochNanoseconds) + '|' +
         (z.subtract({ months: 1 }).add({ months: 1 }).epochNanoseconds === z.epochNanoseconds) + '|' +
         (z.add(new Temporal.Duration()).epochNanoseconds === z.epochNanoseconds) + '|' +
         (z.add('PT0S').toString() === z.toString())",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true");
}

#[test]
fn zoned_date_time_add_subtract_intermediate_check() {
    let mut vm = Vm::new();
    // ±MAX instant 的 {days:∓1}：中间日期越 PlainDateTime 范围 → RangeError。
    let r = eval(
        &mut vm,
        "const min = new Temporal.ZonedDateTime(-8640000000000000000000n, 'UTC');
         const max = new Temporal.ZonedDateTime(8640000000000000000000n, 'UTC');
         (() => { try { min.add({ days: -1 }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { min.subtract({ days: 1 }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { max.add({ days: 1 }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { max.subtract({ days: -1 }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true");
}

#[test]
fn zoned_date_time_add_slots_preserved() {
    let mut vm = Vm::new();
    // 运算结果保留 receiver 时区/日历槽。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC', 'gregory');
         const a = z.add({ years: 1 });
         (a.timeZoneId === 'UTC') + '|' +
         (a.calendarId === 'gregory') + '|' +
         (a.toString() === '1971-01-01T00:00:00+00:00[UTC]') + '|' +
         (z.subtract({ years: 1 }).toString() === '1969-01-01T00:00:00+00:00[UTC]')",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true");
}

#[test]
fn zoned_date_time_add_string_and_time_fields() {
    let mut vm = Vm::new();
    // duration 字符串与时间各字段（含跨天进位）合成正确。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         z.add('PT2H30M').toString() + '|' +
         z.add({ hours: 24, minutes: 30 }).toString() + '|' +
         z.add({ milliseconds: 1500, microseconds: 2, nanoseconds: 3 }).toString() + '|' +
         z.subtract('PT1H').toString()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1970-01-01T02:30:00+00:00[UTC]|1970-01-02T00:30:00+00:00[UTC]|1970-01-01T00:00:01.500002003+00:00[UTC]|1969-12-31T23:00:00+00:00[UTC]"
    );
}

#[test]
fn zoned_date_time_round_hour_increment() {
    let mut vm = Vm::new();
    // 217175010123456789n +01:00 本地 15:23:30.123456789，hour/4 舍到 16:00
    // （rounding-increments.js 期望 epoch 217177200000000000n）。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(217175010123456789n, '+01:00');
         z.round({ smallestUnit: 'hour', roundingIncrement: 4 }).toString() + '|' +
         z.round({ smallestUnit: 'hour', roundingIncrement: 4 }).epochNanoseconds",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "1976-11-18T16:00:00+01:00[+01:00]|217177200000000000");
}

#[test]
fn zoned_date_time_round_day_path_cross_midnight() {
    let mut vm = Vm::new();
    // day 双路径：23:59:59.999999999 本地舍到次日 00:00（对象与字符串简写同效）。
    let r = eval(
        &mut vm,
        "const z = Temporal.ZonedDateTime.from('1976-11-18T23:59:59.999999999+01:00[+01:00]');
         z.round({ smallestUnit: 'day' }).toString() + '|' +
         z.round('day').toString() + '|' +
         Temporal.ZonedDateTime.from('1976-11-18T11:59:59.999999999+01:00[+01:00]').round('day').toString()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1976-11-19T00:00:00+01:00[+01:00]|1976-11-19T00:00:00+01:00[+01:00]|1976-11-18T00:00:00+01:00[+01:00]"
    );
}

#[test]
fn zoned_date_time_round_time_unit_epoch_values() {
    let mut vm = Vm::new();
    // minute/15 与 nanosecond 舍入的 epoch 对拍（rounding-increments.js 合法表）。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(217175010123456789n, '+01:00');
         z.round({ smallestUnit: 'minute', roundingIncrement: 15 }).epochNanoseconds + '|' +
         z.round({ smallestUnit: 'nanosecond' }).epochNanoseconds + '|' +
         z.round({ smallestUnit: 'hour', roundingIncrement: 12 }).epochNanoseconds",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "217175400000000000|217175010123456789|217162800000000000");
}

#[test]
fn zoned_date_time_round_true_factor_increment_validation() {
    let mut vm = Vm::new();
    // 真因子校验：increment==units_per_day（hour/24、minute/60）与 day>1 均 RangeError；
    // 合法因子放行（hour/12、minute/15、day/1）。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         const tryRound = (o) => { try { z.round(o); return 'ok'; } catch (e) { return e instanceof RangeError; } };
         tryRound({ smallestUnit: 'hour', roundingIncrement: 24 }) + '|' +
         tryRound({ smallestUnit: 'minute', roundingIncrement: 60 }) + '|' +
         tryRound({ smallestUnit: 'day', roundingIncrement: 2 }) + '|' +
         tryRound({ smallestUnit: 'hour', roundingIncrement: 12 }) + '|' +
         tryRound({ smallestUnit: 'minute', roundingIncrement: 15 }) + '|' +
         tryRound({ smallestUnit: 'day' })",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|ok|ok|ok");
}

#[test]
fn zoned_date_time_round_out_of_range_errors() {
    let mut vm = Vm::new();
    // 越界三例：day 路径 startNs/endNs 越 MAX，else 路径进位越 MAX，均 RangeError。
    let r = eval(
        &mut vm,
        "const tryR = (z, o) => { try { z.round(o); return 'ok'; } catch (e) { return e instanceof RangeError; } };
         tryR(new Temporal.ZonedDateTime(-8640000000000000000000n, '-01:00'), { smallestUnit: 'days' }) + '|' +
         tryR(new Temporal.ZonedDateTime(8640000000000000000000n, 'UTC'), { smallestUnit: 'day' }) + '|' +
         tryR(new Temporal.ZonedDateTime(8640000000000000000000n, '+23:59'), { smallestUnit: 'minutes', roundingIncrement: 10, roundingMode: 'ceil' })",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true");
}

#[test]
fn zoned_date_time_round_branding_and_arg_validation() {
    let mut vm = Vm::new();
    // branding TypeError、round() 无参 TypeError、对象无 smallestUnit RangeError、
    // 非法单位串 RangeError、非法舍入模式 RangeError。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         (() => { try { Temporal.ZonedDateTime.prototype.round.call({}, { smallestUnit: 'day' }); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { z.round(); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { z.round({}); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { z.round({ smallestUnit: 'years' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { z.round({ smallestUnit: 'hour', roundingMode: 'bogus' }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true|true");
}

#[test]
fn zoned_date_time_add_error_paths() {
    let mut vm = Vm::new();
    // 缺参 TypeError、混合符号 duration RangeError、epoch 越界 RangeError、branding TypeError。
    let r = eval(
        &mut vm,
        "const z = new Temporal.ZonedDateTime(0n, 'UTC');
         (() => { try { z.add(); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { z.add({ days: 1, hours: -1 }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { new Temporal.ZonedDateTime(8640000000000000000000n, 'UTC').add({ hours: 1 }); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })() + '|' +
         (() => { try { Temporal.ZonedDateTime.prototype.add.call({}, {}); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })() + '|' +
         (() => { try { Temporal.ZonedDateTime.prototype.subtract.call({}, {}); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|true|true|true|true");
}

// -- Temporal.Duration 本体补全（69.2） --

#[test]
fn duration_compare_time_only_fast_path() {
    let mut vm = Vm::new();
    // 纯时间单位无 relativeTo：按归一纳秒比大小（days 折 24h）。
    assert_eq!(num(&mut vm, "Temporal.Duration.compare('P1D', 'PT24H')"), 0.0);
    assert_eq!(num(&mut vm, "Temporal.Duration.compare('P1D', 'PT23H')"), 1.0);
    assert_eq!(num(&mut vm, "Temporal.Duration.compare('PT23H', 'P1D')"), -1.0);
    assert_eq!(num(&mut vm, "Temporal.Duration.compare('P2D', 'PT48H')"), 0.0);
    assert_eq!(num(&mut vm, "Temporal.Duration.compare({days:1}, {hours:24})"), 0.0);
}

#[test]
fn duration_compare_calendar_units_require_relative() {
    let mut vm = Vm::new();
    // 无 relativeTo 且含日历单位 → RangeError。
    let r = eval(
        &mut vm,
        "try { Temporal.Duration.compare({years:1},{months:12}); 'no-throw' } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
}

#[test]
fn duration_compare_relative_calendar_units() {
    let mut vm = Vm::new();
    // relativeto-month 对拍：P1M vs P30D 取决于相对点所在月份长度。
    assert_eq!(num(&mut vm, "Temporal.Duration.compare('P1M','P30D',{relativeTo:'2018-04-01'})"), 0.0);
    assert_eq!(num(&mut vm, "Temporal.Duration.compare('P1M','P30D',{relativeTo:'2018-03-01'})"), 1.0);
    assert_eq!(num(&mut vm, "Temporal.Duration.compare('P1M','P30D',{relativeTo:'2018-02-01'})"), -1.0);
    // string 与 PlainDate 对象等价。
    assert_eq!(
        num(
            &mut vm,
            "Temporal.Duration.compare('P1M','P30D',{relativeTo: Temporal.PlainDate.from('2018-04-01')})"
        ),
        0.0
    );
}

#[test]
fn duration_compare_relative_zdt_and_instant() {
    let mut vm = Vm::new();
    // 1970-04-01T00:00Z：P1M（到 5/1 为 30 天）vs P30D → 0。
    assert_eq!(
        num(
            &mut vm,
            "Temporal.Duration.compare('P1M','P30D',{relativeTo: new Temporal.ZonedDateTime(7776000000000000n,'UTC')})"
        ),
        0.0
    );
    // Instant relativeTo（UTC 分解）同结果。
    assert_eq!(
        num(
            &mut vm,
            "Temporal.Duration.compare('P1M','P30D',{relativeTo: Temporal.Instant.fromEpochNanoseconds(7776000000000000n)})"
        ),
        0.0
    );
}

#[test]
fn duration_subtract_equals_add_negated() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "const a = new Temporal.Duration(0,0,0,5,3);
         const b = {days:2,hours:1};
         a.subtract(b).toString() + '|' + a.add(Temporal.Duration.from(b).negated()).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "P3DT2H|P3DT2H");
    // 含日历单位 → RangeError（继承 add 限制）。
    let r = eval(
        &mut vm,
        "try { new Temporal.Duration(1,0,0,0).subtract({days:1}); 'no-throw' } catch (e) { e.constructor.name }",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "RangeError");
}

#[test]
fn duration_add_weighted_sum_range_boundary() {
    let mut vm = Vm::new();
    // 范围判据按平衡后时间分量加权和（spec IsValidDuration），非逐分量：
    // days 和 104249991374 + 时间 27392 s 恰 = 2^53 s → RangeError（逐分量均低于上限）；
    // 同输入减 1 s（总和 2^53 s − 1 s）→ 通过并平衡到 day/hour/minute/second。
    let r = eval(
        &mut vm,
        "(() => {
           const a = new Temporal.Duration(0, 0, 0, 52083464098, 3, 53, 20);
           const b = {days: 52166527276, hours: 3, minutes: 43, seconds: 12};
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           return [
             kind(() => a.add(b)),
             kind(() => new Temporal.Duration(0, 0, 0, -52083464098, -3, -53, -20).subtract(b)),
             kind(() => new Temporal.Duration(0, 0, 0, 52083464098, 3, 53, 19).add(b)),
             new Temporal.Duration(0, 0, 0, 52083464098, 3, 53, 19).add(b).toString(),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "RangeError|RangeError|no|P104249991374DT7H36M31S");
}

#[test]
fn duration_equals_compares_all_components() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "new Temporal.Duration(1,2,3,4).equals('P1Y2M3W4D') + '|' +
         new Temporal.Duration(1,2,3,4).equals('P1Y2M3W4DT1S') + '|' +
         new Temporal.Duration().equals('PT0S') + '|' +
         (() => { try { Temporal.Duration.prototype.equals.call({}, 'P1D'); return 'no-throw'; }
                  catch (e) { return e instanceof TypeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|false|true|true");
}

#[test]
fn duration_sign_and_blank() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "new Temporal.Duration().sign + '|' + new Temporal.Duration().blank + '|' +
         new Temporal.Duration(-1,0,0,0).sign + '|' + new Temporal.Duration(-1,0,0,0).blank + '|' +
         Temporal.Duration.from('PT1H').sign",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "0|true|-1|false|1");
}

#[test]
fn duration_to_json_and_locale_string_and_tag() {
    let mut vm = Vm::new();
    let r = eval(
        &mut vm,
        "const d = new Temporal.Duration(1,2,3,4,5);
         d.toJSON() + '|' + d.toLocaleString() + '|' + d[Symbol.toStringTag] + '|' +
         (d.toJSON() === d.toString()) + '|' + (d.toLocaleString('zh', {}) === d.toString())",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "P1Y2M3W4DT5H|P1Y2M3W4DT5H|Temporal.Duration|true|true");
}

#[test]
fn duration_round_relative_calendar_units() {
    let mut vm = Vm::new();
    // P1M 相对 2018-04-01 舍到 day：4/1 + P1M = 5/1，round day → P1M。
    // P11M round {largestUnit:'year'}：smallest 默认 nanosecond，无日历舍入 → P11M。
    // 缺 relativeTo 的日历单位舍入 → RangeError。
    let r = eval(
        &mut vm,
        "Temporal.Duration.from('P1M').round({smallestUnit:'day', relativeTo:'2018-04-01'}).toString() + '|' +
         Temporal.Duration.from('P11M').round({largestUnit:'year', relativeTo:'2021-01-01'}).toString() + '|' +
         (() => { try { Temporal.Duration.from('P1M').round({smallestUnit:'day'}); return 'no-throw'; }
                  catch (e) { return e instanceof RangeError; } })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "P1M|P11M|true");
}

#[test]
fn duration_round_relative_smallest_calendar_unit() {
    let mut vm = Vm::new();
    // P11M round {largestUnit:'year', smallestUnit:'year'}：2021-01-01 下 11 个月 < 1 年，
    // halfExpand 折 334/365 → 舍到 1 年。
    let r = eval(
        &mut vm,
        "Temporal.Duration.from('P11M').round({largestUnit:'year', smallestUnit:'year', relativeTo:'2021-01-01'}).toString() + '|' +
         Temporal.Duration.from('P11M').round({largestUnit:'year', smallestUnit:'month', relativeTo:'2021-01-01'}).toString()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "P1Y|P11M");
}

#[test]
fn duration_total_relative_zdt() {
    let mut vm = Vm::new();
    // ZDT relativeTo 取本地日期：1970-04-01 下 P1M total months → 1。
    let r = eval(
        &mut vm,
        "Temporal.Duration.from('P1M').total({unit:'months', relativeTo: new Temporal.ZonedDateTime(7776000000000000n,'UTC')})",
    )
    .unwrap();
    assert_eq!(r.as_double(), 1.0);
}

// -- Temporal.PlainMonthDay / PlainYearMonth（E 批）--

#[test]
fn plain_month_day_ctor_basic_and_instanceof() {
    let mut vm = Vm::new();
    // 构造成功：instanceof / length / name / 槽值 / 参考年默认 1972 经 always 形回读。
    let r = eval(
        &mut vm,
        "(() => {
           const md = new Temporal.PlainMonthDay(5, 2);
           return [
             md instanceof Temporal.PlainMonthDay,
             Temporal.PlainMonthDay.length,
             Temporal.PlainMonthDay.name,
             Temporal.PlainYearMonth.length,
             md.monthCode,
             md.day,
             md.calendarId,
             md.toString({calendarName: 'always'}),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "true|2|PlainMonthDay|2|M05|2|iso8601|1972-05-02[u-ca=iso8601]");
}

#[test]
fn plain_month_day_ctor_rejects() {
    let mut vm = Vm::new();
    // BigInt/Symbol 分量 → TypeError；NaN/±Inf → RangeError；缺参 → RangeError；
    // 非闰参考年 2/29 → RangeError，闰参考年 1972 合法；非 new 调用 → TypeError。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           return [
             kind(() => new Temporal.PlainMonthDay(1n, 1)),
             kind(() => new Temporal.PlainMonthDay(Symbol(), 1)),
             kind(() => new Temporal.PlainMonthDay(NaN, 1)),
             kind(() => new Temporal.PlainMonthDay(1, Infinity)),
             kind(() => new Temporal.PlainMonthDay()),
             kind(() => new Temporal.PlainMonthDay(2, 29, 'iso8601', 2023)),
             new Temporal.PlainMonthDay(2, 29).day,
             kind(() => Temporal.PlainMonthDay(1, 2)),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "TypeError|TypeError|RangeError|RangeError|RangeError|RangeError|29|TypeError"
    );
}

#[test]
fn plain_month_day_ctor_calendar_argument() {
    let mut vm = Vm::new();
    // 日历参数：{} → TypeError；函数对象按缺省 iso8601；大小写不敏感白名单；
    // 未知字符串 → RangeError。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           return [
             kind(() => new Temporal.PlainMonthDay(1, 1, {})),
             new Temporal.PlainMonthDay(1, 1, () => 'iso8601').calendarId,
             new Temporal.PlainMonthDay(1, 1, 'iSo8601').calendarId,
             kind(() => new Temporal.PlainMonthDay(1, 1, 'local')),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "TypeError|iso8601|iso8601|RangeError");
}

#[test]
fn plain_year_month_ctor_limits_and_ref_day() {
    let mut vm = Vm::new();
    // 年月边界：-271821-03 与 275760-10 越界抛 RangeError，相邻月合法；
    // refISODay 缺省 1，须落在目标月长内；非 new 调用 → TypeError；
    // NaN 分量（undefined/不可解析串）→ RangeError，null → 0。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const ym = new Temporal.PlainYearMonth(-271821, 4, 'iso8601', 18);
           return [
             kind(() => new Temporal.PlainYearMonth(-271821, 3)),
             kind(() => new Temporal.PlainYearMonth(275760, 10)),
             new Temporal.PlainYearMonth(-271821, 4).monthCode,
             new Temporal.PlainYearMonth(275760, 9).monthCode,
             ym.toString({calendarName: 'always'}),
             new Temporal.PlainYearMonth(2021, 2).toString({calendarName: 'always'}),
             kind(() => new Temporal.PlainYearMonth(2021, 2, 'iso8601', 0)),
             kind(() => new Temporal.PlainYearMonth(2021, 2, 'iso8601', 29)),
             kind(() => new Temporal.PlainYearMonth(undefined, 11)),
             kind(() => new Temporal.PlainYearMonth('invalid', 11)),
             new Temporal.PlainYearMonth(null, 11).year,
             kind(() => new Temporal.PlainMonthDay(undefined, 24)),
             kind(() => Temporal.PlainYearMonth(1970, 1)),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "RangeError|RangeError|M04|M09|-271821-04-18[u-ca=iso8601]|2021-02-01[u-ca=iso8601]|RangeError|RangeError|RangeError|RangeError|0|RangeError|TypeError"
    );
}

#[test]
fn plain_year_month_getters() {
    let mut vm = Vm::new();
    // 槽值 getter + 日历直读 + ISO 日历无纪元（era/eraYear 恒 undefined）。
    let r = eval(
        &mut vm,
        "(() => {
           const ym = new Temporal.PlainYearMonth(-1, 8);
           return [
             ym.year, ym.month, ym.monthCode, ym.calendarId,
             ym.daysInMonth, ym.daysInYear, ym.monthsInYear, ym.inLeapYear,
             String(ym.era), String(ym.eraYear),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "-1|8|M08|iso8601|31|365|12|false|undefined|undefined");
}

#[test]
fn plain_month_day_to_string_and_to_json() {
    let mut vm = Vm::new();
    // 默认/auto/never 同形；critical 带 ! 注解；toJSON = 默认形且忽略参数；
    // 非法 calendarName → RangeError；options 非对象 → TypeError。
    let r = eval(
        &mut vm,
        "(() => {
           const md = new Temporal.PlainMonthDay(5, 2);
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           return [
             md.toString(),
             md.toString({calendarName: 'auto'}),
             md.toString({calendarName: 'never'}),
             md.toString({calendarName: 'critical'}),
             md.toJSON(),
             kind(() => md.toString({calendarName: 'ALWAYS'})),
             kind(() => md.toString(1)),
             md.toString({}),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "05-02|05-02|05-02|1972-05-02[!u-ca=iso8601]|05-02|RangeError|TypeError|05-02"
    );
}

#[test]
fn plain_year_month_to_string_year_format() {
    let mut vm = Vm::new();
    // 年份格式化：-1 → -000001（6 位带符号）；always 形补参考日。
    let r = eval(
        &mut vm,
        "(() => {
           const ym = new Temporal.PlainYearMonth(-1, 8);
           return [ym.toString(), ym.toString({calendarName: 'always'}), ym.toJSON()].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "-000001-08|-000001-08-01[u-ca=iso8601]|-000001-08");
}

#[test]
fn plain_month_day_and_year_month_branding() {
    let mut vm = Vm::new();
    // receiver 非实例 → TypeError（toString / getter 均校验品牌）。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           return [
             kind(() => Temporal.PlainMonthDay.prototype.toString.call(1)),
             kind(() => Temporal.PlainMonthDay.prototype.day.call({})),
             kind(() => Temporal.PlainYearMonth.prototype.toString.call('x')),
             kind(() => Temporal.PlainYearMonth.prototype.year.call(null)),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "TypeError|TypeError|TypeError|TypeError");
}

#[test]
fn plain_month_day_and_year_month_to_locale_string_value_of_to_string_tag() {
    let mut vm = Vm::new();
    // toLocaleString 忽略 locale/options，恒默认形串；valueOf 恒 TypeError
    // （`<` 走 ToPrimitive→valueOf 同抛）；toStringTag 经 Object.prototype.toString 读回。
    let r = eval(
        &mut vm,
        "(() => {
           const pmd = new Temporal.PlainMonthDay(5, 2);
           const pym = new Temporal.PlainYearMonth(2021, 2);
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           return [
             pmd.toLocaleString('en-US', {calendar: 'iso8601'}),
             pym.toLocaleString(),
             kind(() => pmd.valueOf()),
             kind(() => pym.valueOf()),
             kind(() => pmd < pym),
             Object.prototype.toString.call(pmd),
             Object.prototype.toString.call(pym),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "05-02|2021-02|TypeError|TypeError|TypeError|[object Temporal.PlainMonthDay]|[object Temporal.PlainYearMonth]"
    );
}

#[test]
fn plain_month_day_from_string_matrix() {
    let mut vm = Vm::new();
    // 串形态矩阵：MM-DD / MMDD / --前缀 / 完整日期（年份丢弃，参考年恒 1972）/
    // 完整 datetime（leap second 接受、小数时分拒、小数秒 ≤9 位）/ 注解规则。
    let r = eval(
        &mut vm,
        "(() => {
           const run = (s) => { try { const m = Temporal.PlainMonthDay.from(s); return m.monthCode + '-' + m.day; } catch (e) { return e.constructor.name; } };
           return [
             run('10-01'), run('1001'), run('--10-01'), run('--1001'),
             run('1965-10-01'), run('+001976-10-01'), run('-999999-10-01'),
             run('1976-10-01T15:23:30.1+00:00'), run('19761001T15:23:30.1+0000'),
             run('2000-05-02T00+00:00'), run('1976-05-02 15:23'),
             run('2016-12-31T23:59:60'),
             run('1976-05-02T15:23[u-ca=iso8601]'),
             run('11-18[u-ca=ISO8601][u-ca=discord]'),
             Temporal.PlainMonthDay.from('2019-10-31').toString({calendarName: 'always'}),
             run('11-18junk'), run('11-18[u-ca=gregory]'), run('11-18[U-CA=iso8601]'),
             run('11-18[FOO=bar]'), run('11-18[!foo=bar]'),
             run('-000000-08-24'), run('-000000-08-24T15:43:27'),
             run('2019-10-01T09:00:00Z'), run('11-18Z'),
             run('13-34'), run('2025-04-03T12.5'),
             run('1970-01-01T00:00:00.1234567891'), run('2022-09-15+00:00'),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "M10-1|M10-1|M10-1|M10-1|M10-1|M10-1|M10-1|M10-1|M10-1|M05-2|M05-2|M12-31|M05-2|M11-18|1972-10-31[u-ca=iso8601]|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError"
    );
}

#[test]
fn plain_month_day_from_bag_matrix() {
    let mut vm = Vm::new();
    // bag 矩阵：字段序/必填/冲突、monthCode 两段校验、era 双现、year 仅 overflow、
    // constrain/reject、自定义日历 ID（ISO 串原样保留）、错误类型。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const run = (f, o) => { const m = Temporal.PlainMonthDay.from(f, o); return m.monthCode + '-' + m.day; };
           const always = (f, o) => Temporal.PlainMonthDay.from(f, o).toString({calendarName: 'always'});
           return [
             run({month: 10, day: 1}), run({monthCode: 'M12', day: 31}),
             always({year: 2021, month: 7, day: 3}),
             run({month: 10, day: 1, days: 31}),
             kind(() => Temporal.PlainMonthDay.from({})),
             kind(() => Temporal.PlainMonthDay.from({day: 15})),
             kind(() => Temporal.PlainMonthDay.from({month: 12})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M12'})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: undefined, day: 15})),
             kind(() => Temporal.PlainMonthDay.from({months: 12, day: 31})),
             kind(() => Temporal.PlainMonthDay.from({month: 12, monthCode: 'M11', day: 17})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M99L'})),
             kind(() => Temporal.PlainMonthDay.from({day: 32})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'm01', day: 17})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M00', day: 17})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M13', day: 17})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M05L', day: 17})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M99L', day: 1})),
             run({year: 2000, month: 5, day: 2, era: 'ce'}),
             run({year: 2000, month: 5, day: 2, eraYear: 1}),
             kind(() => Temporal.PlainMonthDay.from({year: 2000, month: 5, day: 2, era: 'ce', eraYear: 1})),
             run({year: -999999, month: 1, day: 1}),
             run({year: -999999, monthCode: 'M02', day: 29}),
             run({year: -1000000, monthCode: 'M02', day: 29}),
             kind(() => Temporal.PlainMonthDay.from({year: -999999, monthCode: 'M02', day: 29}, {overflow: 'reject'})),
             run({monthCode: 'M02', day: 30}, {overflow: 'constrain'}),
             run({month: 2, day: 31}),
             run({year: 2000, month: 13, day: 34}, {overflow: 'constrain'}),
             kind(() => Temporal.PlainMonthDay.from({year: 2000, month: 13, day: 34}, {overflow: 'reject'})),
             kind(() => Temporal.PlainMonthDay.from({day: 1, month: -1})),
             kind(() => Temporal.PlainMonthDay.from({month: 1, day: -1})),
             kind(() => Temporal.PlainMonthDay.from({year: Infinity, month: 5, day: 2})),
             kind(() => Temporal.PlainMonthDay.from({year: 2000, month: 5, day: 1n})),
             kind(() => Temporal.PlainMonthDay.from(1)),
             kind(() => Temporal.PlainMonthDay.from(1n)),
             kind(() => Temporal.PlainMonthDay.from()),
             Temporal.PlainMonthDay.from({monthCode: 'M11', day: 18, calendar: 'iso8601'}).calendarId,
             Temporal.PlainMonthDay.from({monthCode: 'M11', day: 18, calendar: 'IsO8601'}).calendarId,
             Temporal.PlainMonthDay.from({monthCode: 'M11', day: 18, calendar: '2020-01-01'}).calendarId,
             Temporal.PlainMonthDay.from({monthCode: 'M11', day: 18, calendar: '2020-01'}).monthCode,
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M11', day: 18, calendar: 'notacal'})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M11', day: 18, calendar: ''})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M11', day: 18, calendar: {}})),
             kind(() => Temporal.PlainMonthDay.from({monthCode: 'M11', day: 18, calendar: new Temporal.Duration()})),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "M10-1|M12-31|1972-07-03[u-ca=iso8601]|M10-1|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|RangeError|TypeError|TypeError|RangeError|RangeError|RangeError|RangeError|RangeError|M05-2|M05-2|RangeError|M01-1|M02-28|M02-29|RangeError|M02-29|M02-29|M12-31|RangeError|RangeError|RangeError|RangeError|TypeError|TypeError|TypeError|TypeError|iso8601|iso8601|iso8601|M11|RangeError|RangeError|TypeError|TypeError"
    );
}

#[test]
fn plain_month_day_from_instances_and_options() {
    let mut vm = Vm::new();
    // 实例复制（refYear/日历保留、非同一对象）/ PD 实例 / options 形态与读取序
    // （options.overflow 在所有字段 Get 之后；number 参数与坏串在读 options 前抛）。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const orig = new Temporal.PlainMonthDay(5, 2, undefined, 2000);
           const copy = Temporal.PlainMonthDay.from(orig);
           let pdSeen = [];
           let pd = Temporal.PlainDate.from('2019-10-01');
           let pdCopy = Temporal.PlainMonthDay.from(pd);
           let optsSeen = [];
           const options = { get overflow() { optsSeen.push(1); return 'constrain'; } };
           const a = Temporal.PlainMonthDay.from({month: 2, day: 3}, options);
           let numSeen = [];
           const numOptions = { get overflow() { numSeen.push(1); return 'constrain'; } };
           kind(() => Temporal.PlainMonthDay.from(7, numOptions));
           let strSeen = [];
           const strOptions = { get overflow() { strSeen.push(1); return 'constrain'; } };
           kind(() => Temporal.PlainMonthDay.from('13-34', strOptions));
           return [
             copy.monthCode + '-' + copy.day,
             copy.toString({calendarName: 'always'}),
             copy.calendarId === orig.calendarId,
             copy !== orig,
             Temporal.PlainMonthDay.from(orig, {overflow: 'reject'}).day,
             pdCopy.monthCode + '-' + pdCopy.day + ':' + pdCopy.toString({calendarName: 'always'}),
             a.monthCode + '-' + a.day + ':' + optsSeen.length,
             numSeen.length + ':' + strSeen.length,
             kind(() => Temporal.PlainMonthDay.from({month: 2, day: 3}, null)),
             kind(() => Temporal.PlainMonthDay.from({month: 2, day: 3}, true)),
             kind(() => Temporal.PlainMonthDay.from({month: 2, day: 3}, 1n)),
             kind(() => Temporal.PlainMonthDay.from({month: 2, day: 3}, Symbol())),
             kind(() => Temporal.PlainMonthDay.from({month: 2, day: 3}, {overflow: 'CONSTRAIN'})),
             kind(() => Temporal.PlainMonthDay.from({month: 2, day: 3}, {overflow: 'reject\\0'})),
             Temporal.PlainMonthDay.from({month: 2, day: 3}, () => {}).monthCode + '-' +
               Temporal.PlainMonthDay.from('05-02', {overflow: 'reject'}).day,
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "M05-2|2000-05-02[u-ca=iso8601]|true|true|2|M10-1:1972-10-01[u-ca=iso8601]|M02-3:1|0:0|TypeError|TypeError|TypeError|TypeError|RangeError|RangeError|M02-2"
    );
}

#[test]
fn plain_month_day_equals_matrix() {
    let mut vm = Vm::new();
    // equals 比较 (月, 日, 参考年) 三元与日历；other 走 ToTemporalMonthDay
    // （实例/串/bag/日历 ISO 串归一），坏参 TypeError/RangeError。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const pmd = new Temporal.PlainMonthDay(5, 2);
           return [
             pmd.equals(new Temporal.PlainMonthDay(5, 2)),
             pmd.equals(new Temporal.PlainMonthDay(5, 2, 'iso8601', 2000)),
             pmd.equals(Temporal.PlainMonthDay.from('05-02')),
             pmd.equals({monthCode: 'M05', day: 2}),
             pmd.equals({month: 5, day: 2}),
             pmd.equals({monthCode: 'M05', day: 2, calendar: '2020-01-01'}),
             pmd.equals('05-02'),
             pmd.equals('05-03'),
             pmd.equals(new Temporal.PlainMonthDay(5, 3, 'iso8601', 1972)),
             kind(() => pmd.equals()),
             kind(() => pmd.equals(1)),
             kind(() => pmd.equals(1n)),
             kind(() => pmd.equals('junk')),
             kind(() => pmd.equals({monthCode: 'M99', day: 2})),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "true|false|true|true|true|true|true|false|false|TypeError|TypeError|TypeError|RangeError|RangeError"
    );
}

#[test]
fn plain_month_day_with_matrix() {
    let mut vm = Vm::new();
    // with 覆盖 day/month/monthCode；year 仅供 overflow 月长；undefined 不覆盖；
    // 空 bag/无识别字段/非对象/Temporal 实例/calendar/timeZone 键 TypeError；
    // constrain 按 bag year（缺省 1972）钳日；结果参考年恒 1972、保留 receiver 日历。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const md = Temporal.PlainMonthDay.from('01-22');
           const f = (m) => m.monthCode + '-' + m.day;
           const leap = new Temporal.PlainMonthDay(2, 29, 'iso8601', 1972);
           const r2000 = new Temporal.PlainMonthDay(5, 2, 'iso8601', 2000);
           return [
             f(md.with({day: 22})), f(md.with({month: 12})), f(md.with({monthCode: 'M12'})),
             f(md.with({month: 12, monthCode: 'M12'})), f(md.with({monthCode: 'M12', days: 1})),
             f(md.with({year: 2000})), f(md.with({day: 1, monthCode: undefined})),
             kind(() => md.with({month: 12, monthCode: 'M11'})),
             kind(() => md.with({})), kind(() => md.with({months: 12})),
             kind(() => md.with(7)), kind(() => md.with(Temporal.PlainMonthDay.from('05-02'))),
             kind(() => md.with({day: 1, calendar: 'iso8601'})),
             kind(() => md.with({day: 1, timeZone: 'UTC'})),
             kind(() => md.with({day: Infinity})), kind(() => md.with({month: 13}, {overflow: 'reject'})),
             f(leap.with({year: -999999})), f(leap.with({year: -1000000})),
             kind(() => leap.with({year: -999999}, {overflow: 'reject'})),
             r2000.with({day: 3}).toString({calendarName: 'always'}),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "M01-22|M12-22|M12-22|M12-22|M12-22|M01-22|M01-1|RangeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|RangeError|RangeError|M02-28|M02-29|RangeError|1972-05-03[u-ca=iso8601]"
    );
}

#[test]
fn plain_month_day_to_plain_date_matrix() {
    let mut vm = Vm::new();
    // toPlainDate：yearLike number | {year}（缺 year/非对象 TypeError）；
    // options.overflow 绝不读取；constrain 钳日；day 级表示范围 ±100_000_00x。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const md = Temporal.PlainMonthDay.from('01-22');
           const leap = Temporal.PlainMonthDay.from('02-29');
           return [
             md.toPlainDate({year: 2002}).toString(),
             kind(() => md.toPlainDate({something: 'nothing'})),
             kind(() => md.toPlainDate(12)),
             md.toPlainDate({year: 12}).toString(),
             kind(() => md.toPlainDate({year: Infinity})),
             leap.toPlainDate({year: 2020}).toString(),
             leap.toPlainDate({year: 2023}).toString(),
             leap.toPlainDate({year: 2020}, {get overflow() { throw new Error('nope'); }}).toString(),
             Temporal.PlainMonthDay.from('04-19').toPlainDate({year: -271821}).toString(),
             kind(() => Temporal.PlainMonthDay.from('01-01').toPlainDate({year: -271821})),
             Temporal.PlainMonthDay.from('09-13').toPlainDate({year: 275760}).toString(),
             kind(() => Temporal.PlainMonthDay.from('09-14').toPlainDate({year: 275760})),
             md.toPlainDate({year: 2002}).calendarId,
             kind(() => md.toPlainDate(2002)),
             md.toPlainDate({year: '2002'}).toString(),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2002-01-22|TypeError|TypeError|0012-01-22|RangeError|2020-02-29|2023-02-28|2020-02-29|-271821-04-19|RangeError|275760-09-13|RangeError|iso8601|TypeError|2002-01-22"
    );
}

#[test]
fn plain_year_month_from_string_matrix() {
    let mut vm = Vm::new();
    // 串形态矩阵：年-月 / 年-月-日 / 紧凑形 / 带符号 6 位年 / 完整 datetime（T/t/空格、
    // leap second 接受、偏移逗号小数）/ 注解规则 / 语法级月日静态检查 /
    // (年, 月) 表示范围 / Z 与变体减号拒绝。
    let r = eval(
        &mut vm,
        "(() => {
           const run = (s) => { try { const m = Temporal.PlainYearMonth.from(s); return m.year + '-' + m.month; } catch (e) { return e.constructor.name; } };
           return [
             run('1976-11'), run('1976-11-10'), run('197611'), run('19761110'),
             run('+00197611'), run('-009999-11'), run('-009999-11-18T15:23:30.1'),
             run('1976-11-01T09:00:00+00:00'), run('1976-11-18T152330.1+0000'),
             run('1976-11-18t15:23'), run('1976-11-18 15:23'),
             run('2019-12-15T15:23[UTC][u-ca=iso8601]'),
             run('1976-11[u-ca=ISO8601]'),
             run('2019-12-15T15:23[foo=bar]'),
             run('2016-12-31T23:59:60'),
             Temporal.PlainYearMonth.from('1976-11-10').toString({calendarName: 'always'}),
             run('1976-11junk'), run('1976-11[u-ca=gregory]'), run('1976-11[U-CA=iso8601]'),
             run('1976-11[FOO=bar]'), run('1976-11[!foo=bar]'),
             run('2020-13'), run('1976-00'), run('1976-11-00'), run('2020-02-31'), run('2020-02-30'),
             run('2020-02-29'),
             run('+999999-01'), run('-999999-01'), run('-271821-03'), run('+275760-10'),
             run('-271821-04'), run('+275760-09'), run('-271821-03-31'), run('+275760-09-30'),
             run('-000000-06'), run('1976-11-18T15:23:30.12\\u221202:00'),
             run('2019-10-01T09:00:00Z'), run('2022-09Z'), run('2022-09+01:00'), run('2022-09-15Z'),
             run('1970-01-01T00:00:00.1234567891'), run('2019-12-15T00+00:00:00,0'),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1976-11|1976-11|1976-11|1976-11|1976-11|-9999-11|-9999-11|1976-11|1976-11|1976-11|1976-11|2019-12|1976-11|2019-12|2016-12|1976-11-01[u-ca=iso8601]|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|2020-2|RangeError|RangeError|RangeError|RangeError|-271821-4|275760-9|RangeError|275760-9|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|2019-12"
    );
}

#[test]
fn plain_year_month_from_bag_matrix() {
    let mut vm = Vm::new();
    // bag 矩阵：year 必填 / day 从不读（参考日恒 1）/ 未知键忽略 / month 与 monthCode 等值并存、
    // monthCode ToPrimitive 语义（原始值 TypeError，对象经 toString）/ 语法先于 year 转换、
    // 适配在 year 转换后 / era 双现 / constrain 钳 12、<1 恒 RangeError / (年, 月) 范围 / 日历解析。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const run = (f, o) => { const m = Temporal.PlainYearMonth.from(f, o); return m.year + '-' + m.month; };
           return [
             run({year: 2021, month: 7}), run({year: 2021, monthCode: 'M07'}),
             run({year: 2021, month: 7, months: 12, day: 31}),
             run({year: 2021, month: 11, monthCode: 'M11'}),
             Temporal.PlainYearMonth.from({year: 2021, monthCode: 'M07', day: 50}, {overflow: 'reject'}).month,
             Temporal.PlainYearMonth.from({year: 2021, monthCode: 'M02', day: 50}).toString({calendarName: 'always'}),
             kind(() => Temporal.PlainYearMonth.from({})),
             kind(() => Temporal.PlainYearMonth.from({month: 1})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021})),
             kind(() => Temporal.PlainYearMonth.from({year: undefined, month: 6})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, months: 6})),
             kind(() => Temporal.PlainYearMonth.from({year: 2026, monthCode: 5})),
             kind(() => Temporal.PlainYearMonth.from({year: 2026, monthCode: 5n})),
             kind(() => Temporal.PlainYearMonth.from({year: 2026, monthCode: false})),
             kind(() => Temporal.PlainYearMonth.from({year: 2026, monthCode: Symbol()})),
             kind(() => Temporal.PlainYearMonth.from({year: 2026, monthCode: null})),
             kind(() => Temporal.PlainYearMonth.from({year: 2026, monthCode: {toString: () => 5}})),
             run({year: 2026, monthCode: {toString: () => 'M07'}}),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, monthCode: 'm1'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, monthCode: 'M1'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, monthCode: 'm01'})),
             kind(() => Temporal.PlainYearMonth.from({monthCode: 'L99M', year: Symbol()})),
             kind(() => Temporal.PlainYearMonth.from({monthCode: 'M99L', year: Symbol()})),
             kind(() => Temporal.PlainYearMonth.from({monthCode: 'M99L'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 12, monthCode: 'M11'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, monthCode: 'M00'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, monthCode: 'M13'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, monthCode: 'M05L'})),
             run({year: 2021, month: 13}, {overflow: 'constrain'}),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 13}, {overflow: 'reject'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 0})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: -1}, {overflow: 'constrain'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 0.5})),
             run({year: 2000, month: 5, era: 'ce'}),
             run({year: 2000, month: 5, eraYear: 1}),
             kind(() => Temporal.PlainYearMonth.from({year: 2000, month: 5, era: 'ce', eraYear: 1})),
             run({year: -271821, month: 4}),
             run({year: 275760, month: 9}),
             kind(() => Temporal.PlainYearMonth.from({year: -271821, month: 3})),
             kind(() => Temporal.PlainYearMonth.from({year: 275760, month: 10})),
             kind(() => Temporal.PlainYearMonth.from({year: 999999, month: 5})),
             kind(() => Temporal.PlainYearMonth.from({year: -999999, month: 5})),
             Temporal.PlainYearMonth.from({year: 2019, monthCode: 'M06', calendar: '2020-01'}).calendarId,
             kind(() => Temporal.PlainYearMonth.from({year: 2019, monthCode: 'M06', calendar: 'notacal'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2019, monthCode: 'M06', calendar: null})),
             kind(() => Temporal.PlainYearMonth.from({year: Symbol(), month: 5})),
             kind(() => Temporal.PlainYearMonth.from({year: NaN, month: 5})),
             kind(() => Temporal.PlainYearMonth.from({year: Infinity, month: 5})),
             kind(() => Temporal.PlainYearMonth.from(1)),
             kind(() => Temporal.PlainYearMonth.from()),
             Temporal.PlainYearMonth.from.length,
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2021-7|2021-7|2021-7|2021-11|7|2021-02-01[u-ca=iso8601]|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|2026-7|RangeError|RangeError|RangeError|RangeError|TypeError|TypeError|RangeError|RangeError|RangeError|RangeError|2021-12|RangeError|RangeError|RangeError|RangeError|2000-5|2000-5|RangeError|-271821-4|275760-9|RangeError|RangeError|RangeError|RangeError|iso8601|RangeError|TypeError|TypeError|RangeError|RangeError|TypeError|TypeError|1"
    );
}

#[test]
fn plain_year_month_from_instances_and_options() {
    let mut vm = Vm::new();
    // PYM 实例复制（参考日/日历保留、非同一对象、overflow 先于复制读取）/ PD 实例
    // （参考日恒 1，日不读）/ 坏 options 形态（null/true/1n/Symbol TypeError、
    // 坏 overflow RangeError）/ 函数与 undefined options 归默认 constrain /
    // 坏串与原始值在读 options 前抛。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const orig = new Temporal.PlainYearMonth(2000, 5, 'iso8601', 7);
           const copy = Temporal.PlainYearMonth.from(orig);
           const pdCopy = Temporal.PlainYearMonth.from(Temporal.PlainDate.from('1976-11-18'));
           let optsSeen = [];
           const options = { get overflow() { optsSeen.push(1); return 'constrain'; } };
           const a = Temporal.PlainYearMonth.from({year: 2021, monthCode: 'M01'}, options);
           let numSeen = [];
           const numOptions = { get overflow() { numSeen.push(1); return 'constrain'; } };
           kind(() => Temporal.PlainYearMonth.from(7, numOptions));
           let strSeen = [];
           const strOptions = { get overflow() { strSeen.push(1); return 'constrain'; } };
           kind(() => Temporal.PlainYearMonth.from('2020-13', strOptions));
           let instSeen = [];
           const instOptions = { get overflow() { instSeen.push(1); return 'constrain'; } };
           kind(() => Temporal.PlainYearMonth.from(orig, instOptions));
           return [
             copy.year + '-' + copy.month + ':' + copy.toString({calendarName: 'always'}),
             copy !== orig,
             Temporal.PlainYearMonth.from(orig, {overflow: 'reject'}).year,
             pdCopy.year + '-' + pdCopy.month + ':' + pdCopy.toString({calendarName: 'always'}),
             a.year + '-' + a.month + ':' + optsSeen.length,
             numSeen.length + ':' + strSeen.length + ':' + instSeen.length,
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 7}, null)),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 7}, true)),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 7}, 1n)),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 7}, Symbol())),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 7}, {overflow: 'CONSTRAIN'})),
             kind(() => Temporal.PlainYearMonth.from({year: 2021, month: 7}, {overflow: 'reject\\0'})),
             Temporal.PlainYearMonth.from({year: 2000, month: 13}, () => {}).month,
             Temporal.PlainYearMonth.from({year: 2000, month: 13}, undefined).month,
             Temporal.PlainYearMonth.from({year: 2000, month: 13}, {overflow: undefined}).month,
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2000-5:2000-05-07[u-ca=iso8601]|true|2000|1976-11:1976-11-01[u-ca=iso8601]|2021-1:1|0:0:1|TypeError|TypeError|TypeError|TypeError|RangeError|RangeError|12|12|12"
    );
}

#[test]
fn plain_year_month_compare_matrix() {
    let mut vm = Vm::new();
    // compare 比较 (年, 月, 参考日) 三元字典序（参考日参与）；两参均经
    // ToTemporalYearMonth（串/bag/PD 实例/PYM 实例）；缺字段 TypeError；
    // 子类实例走槽直读不触发 getter。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const c = Temporal.PlainYearMonth.compare;
           const ym = (y, m, rd) => new Temporal.PlainYearMonth(y, m, 'iso8601', rd);
           class AvoidGetters extends Temporal.PlainYearMonth {
             get year() { throw new Error('no'); }
             get month() { throw new Error('no'); }
           }
           return [
             c(ym(2000, 5, 1), ym(2000, 5, 1)),
             c(ym(2000, 5, 1), ym(2000, 5, 2)),
             c(ym(2000, 5, 2), ym(2000, 5, 1)),
             c(ym(1999, 12, 1), ym(2000, 1, 1)),
             c(ym(2000, 5, 1), ym(2000, 6, 1)),
             c('1994-11', ym(1994, 11, 1)),
             c({year: 1994, month: 11}, ym(1994, 11, 1)),
             c(Temporal.PlainDate.from('1994-11-20'), ym(1994, 11, 1)),
             c(ym(2013, 6, 1), {year: 2013, month: 6}),
             kind(() => c({year: 1994}, ym(1994, 11, 1))),
             kind(() => c(1, ym(1994, 11, 1))),
             kind(() => c(ym(1994, 11, 1))),
             c(new AvoidGetters(2000, 5), new AvoidGetters(2006, 3)),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(str_val(&vm, r), "0|-1|1|-1|-1|0|0|0|0|TypeError|TypeError|TypeError|-1");
}

#[test]
fn plain_year_month_equals_matrix() {
    let mut vm = Vm::new();
    // equals 比较 (年, 月, 参考日) 三元与日历；other 经 ToTemporalYearMonth
    // （串/bag/PD/PYM 实例，day 不读）；receiver branding TypeError。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const ym = (y, m, rd) => new Temporal.PlainYearMonth(y, m, 'iso8601', rd);
           const a = ym(2000, 5, 1);
           return [
             a.equals(ym(2000, 5, 1)),
             a.equals(ym(2000, 5, 2)),
             a.equals(ym(2000, 6, 1)),
             a.equals(ym(1999, 5, 1)),
             a.equals('2000-05'),
             a.equals('2000-05-15'),
             a.equals({year: 2000, month: 5}),
             a.equals({year: 2000, monthCode: 'M05'}),
             a.equals({year: 2000, month: 5, day: 20}),
             a.equals(Temporal.PlainDate.from('2000-05-15')),
             kind(() => a.equals()),
             kind(() => a.equals(1)),
             kind(() => a.equals(1n)),
             kind(() => a.equals('junk')),
             kind(() => a.equals({year: 2000})),
             kind(() => a.equals({year: 2000, month: Infinity})),
             kind(() => Temporal.PlainYearMonth.prototype.equals.call({}, a)),
             a.equals(a),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "true|false|false|false|true|true|true|true|true|true|TypeError|TypeError|TypeError|RangeError|TypeError|RangeError|TypeError|true"
    );
}

#[test]
fn plain_year_month_add_matrix() {
    let mut vm = Vm::new();
    // add：ToTemporalDuration（串/bag/实例）→ 读 overflow → weeks/days/时间分量
    // 非零 RangeError；(年,月) 折绝对月数平衡；接收方与结果均按日 1 查可表示性
    // （-271821-04 加零时长也 RangeError）；结果参考日恒 1。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const ym = (s) => Temporal.PlainYearMonth.from(s);
           return [
             ym('2019-11').add({months: 2}).toString(),
             ym('2019-11').add({years: 1}).toString(),
             ym('2019-11').add({months: -2}).toString(),
             ym('2000-05').add('P3M').toString(),
             ym('1994-11').add(Temporal.Duration.from('P18Y7M')).toString(),
             new Temporal.PlainYearMonth(2025, 8).add(new Temporal.Duration()).toString(),
             ym('2019-11').add({years: 1, months: 2}).toString(),
             ym('1970-01').add('-P273790Y8M').toString(),
             ym('1970-01').add('P3285488M').toString(),
             kind(() => ym('2019-11').add({days: 1})),
             kind(() => ym('2019-11').add({hours: 1})),
             kind(() => ym('2019-11').add(new Temporal.Duration(0, 0, 0, -1))),
             kind(() => ym('2019-11').add({years: 1, months: -3})),
             kind(() => ym('2019-11').add({year: 1})),
             kind(() => ym('2019-11').add({})),
             kind(() => ym('2019-11').add(7)),
             kind(() => ym('2019-11').add('')),
             kind(() => ym('2019-11').add({months: 1.5})),
             kind(() => ym('2019-11').add({months: Infinity})),
             kind(() => ym('2019-11').add('P4294967296M')),
             kind(() => ym('+275760-09').add({months: 1})),
             kind(() => ym('-271821-04').add(new Temporal.Duration())),
             kind(() => ym('2019-11').add({months: 1}, null)),
             ym('2023-05').add({months: 12}, {overflow: 'reject'}).toString(),
             ym('2023-05').add({months: 12}, {overflow: 'constrain'}).toString(),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2020-01|2020-11|2019-09|2000-08|2013-06|2025-08|2021-01|-271821-05|+275760-09|RangeError|RangeError|RangeError|RangeError|TypeError|TypeError|TypeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|TypeError|2024-05|2024-05"
    );
}

#[test]
fn plain_year_month_subtract_matrix() {
    let mut vm = Vm::new();
    // subtract 与 add 同核（sign 取反）；日历保留；低阶单位与非零时间分量
    // RangeError；单数键 TypeError；接收方/结果按日 1 可表示性。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const ym = (s) => Temporal.PlainYearMonth.from(s);
           return [
             ym('2013-06').subtract(Temporal.Duration.from('P18Y7M')).toString(),
             ym('2000-05').subtract('P3M').toString(),
             ym('2019-11').subtract({years: 1, months: 2}).toString(),
             ym('+275760-09').subtract({months: 1}).toString(),
             ym('+275760-09').subtract({years: 1}).toString(),
             ym('1970-01').subtract('P273790Y8M').toString(),
             kind(() => ym('2019-11').subtract({weeks: 1})),
             kind(() => ym('2019-11').subtract({seconds: 1})),
             kind(() => ym('2019-11').subtract({months: -1, years: 1})),
             kind(() => ym('2019-11').subtract({month: 1})),
             kind(() => ym('-271821-04').subtract({months: 1})),
             kind(() => ym('-271821-04').subtract(new Temporal.Duration())),
             kind(() => ym('2019-11').subtract({months: 1}, 1)),
             kind(() => ym('2019-11').subtract('junk')),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1994-11|2000-02|2018-09|+275760-08|+275759-09|-271821-05|RangeError|RangeError|RangeError|TypeError|RangeError|RangeError|TypeError|RangeError"
    );
}

#[test]
fn plain_year_month_with_matrix() {
    let mut vm = Vm::new();
    // with：calendar/timeZone 键与实例拒绝；Get 序 month→monthCode→year；
    // 字段转换先于 options；monthCode 两段校验；constrain 钳 month>12；
    // 结果参考日恒 1；年月级范围检查。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const ym = (s) => Temporal.PlainYearMonth.from(s);
           return [
             ym('2019-10').with({ year: 2020 }).toString(),
             ym('2019-10').with({ month: 9 }).toString(),
             ym('2019-10').with({ monthCode: 'M09' }).toString(),
             kind(() => ym('2019-10').with({ month: 9, monthCode: 'M10' })),
             ym('2019-10').with({ month: 1, years: 2020 }).toString(),
             ym('2019-10').with({ month: 11, year: undefined }).toString(),
             ym('2019-10').with({ month: 1.7 }).toString(),
             ym('2000-05').with({ month: 13 }).month,
             ym('2000-05').with({ month: 15 }, { overflow: undefined }).month,
             kind(() => ym('2000-05').with({ month: 15 }, { overflow: 'reject' })),
             kind(() => ym('2000-05').with({ month: 8 }, { overflow: 'CONSTRAIN' })),
             kind(() => ym('2000-05').with({ month: 8 }, { overflow: 'balance' })),
             kind(() => ym('2019-10').with({ month: -1 }, null)),
             kind(() => ym('2019-10').with({ year: 2020 }, null)),
             kind(() => ym('2019-10').with({})),
             kind(() => ym('2019-10').with({ months: 12 })),
             kind(() => ym('2019-10').with({ year: 2021, calendar: 'iso8601' })),
             kind(() => ym('2019-10').with({ year: 2021, timeZone: 'UTC' })),
             kind(() => ym('2019-10').with(42)),
             kind(() => ym('2019-10').with(new Temporal.PlainDate(2019, 5, 17))),
             kind(() => ym('2019-10').with({ month: Infinity })),
             kind(() => ym('2019-10').with({ monthCode: 'M08L' })),
             kind(() => ym('2019-10').with({ monthCode: 'M13' })),
             new Temporal.PlainYearMonth(2019, 10, 'iso8601', 5).with({ month: 3 }).toString({ calendarName: 'always' }),
             ym('2019-10').with({ year: 2020 }, () => {}).toString(),
             ym('-271821-05').with({ month: 4 }).toString(),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2020-10|2019-09|2019-09|RangeError|2019-01|2019-11|2019-01|12|12|RangeError|RangeError|RangeError|RangeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|RangeError|RangeError|RangeError|2019-03-01[u-ca=iso8601]|2020-10|-271821-04"
    );
}

#[test]
fn plain_year_month_to_plain_date_matrix() {
    let mut vm = Vm::new();
    // toPlainDate：item 须对象且只读 day；options.overflow 绝不读取；
    // constrain 钳 day 至 [1, 月长]；day 级范围检查（年月级合法仍可抛）。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const ym = (s) => Temporal.PlainYearMonth.from(s);
           return [
             ym('2002-01').toPlainDate({ day: 22 }).toString(),
             kind(() => ym('2002-01').toPlainDate({ something: 'nothing' })),
             kind(() => ym('2002-01').toPlainDate(7)),
             kind(() => ym('2002-01').toPlainDate(null)),
             kind(() => ym('2002-01').toPlainDate({})),
             ym('2023-02').toPlainDate({ day: 29 }).toString(),
             ym('1998-06').toPlainDate({ day: 31 }).toString(),
             ym('2000-05').toPlainDate({ day: 1.9 }).toString(),
             ym('2000-05').toPlainDate({ day: 0 }).toString(),
             ym('-271821-04').toPlainDate({ day: 19 }).toString(),
             kind(() => ym('-271821-04').toPlainDate({ day: 18 })),
             ym('+275760-09').toPlainDate({ day: 13 }).toString(),
             kind(() => ym('+275760-09').toPlainDate({ day: 14 })),
             kind(() => ym('2000-05').toPlainDate({ day: Infinity })),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "2002-01-22|TypeError|TypeError|TypeError|TypeError|2023-02-28|1998-06-30|2000-05-01|2000-05-01|-271821-04-19|RangeError|275760-09-13|RangeError|RangeError"
    );
}

#[test]
fn plain_year_month_until_since_matrix() {
    let mut vm = Vm::new();
    // until/since：other 四分支（串/bag/PD/PYM 实例）；日历相等（RangeError）；
    // 单位表仅 year/month（缺省 smallest month / largest year）；参考日三元全等
    // 先于可表示性检查返回空 Duration；两端按日 1 的 day 级范围；nudge 舍入
    // （跨单位进位、half 边界、窗口端点范围）；since 整体取反。
    let r = eval(
        &mut vm,
        "(() => {
           const kind = (fn) => { try { fn(); return 'no'; } catch (e) { return e.constructor.name; } };
           const ym = (y, m) => new Temporal.PlainYearMonth(y, m);
           const d = (x) => x.years + ',' + x.months;
           return [
             d(ym(2000, 5).until(ym(2001, 6))),
             d(ym(2001, 6).until(ym(2000, 5))),
             d(ym(2000, 5).until(ym(2001, 6), { largestUnit: 'months' })),
             d(ym(2000, 5).until(ym(2001, 6), { largestUnit: 'years' })),
             d(ym(2000, 5).until(ym(2001, 6), { largestUnit: 'auto' })),
             d(ym(2000, 5).until(ym(2001, 6), { largestUnit: 'years', smallestUnit: 'months' })),
             kind(() => ym(2000, 5).until(ym(2001, 6), { largestUnit: 'months', smallestUnit: 'years' })),
             d(ym(1976, 11).until('1976-11-18T15:23:30.1+00:00')),
             d(ym(2016, 12).since('2016-12-31T23:59:60')),
             d(ym(-9999, 11).until('-009999-11')),
             d(ym(1994, 11).until(ym(2013, 6))),
             d(ym(1994, 11).until({ year: 2013, month: 6 })),
             d(ym(1994, 11).until('2013-06')),
             d(ym(1994, 11).until(Temporal.PlainDate.from('2013-06-15'))),
             d(ym(2013, 6).since(ym(1994, 11))),
             d(ym(2013, 6).since(ym(2013, 6))),
             kind(() => ym(2019, 10).until(ym(1976, 11), null)),
             d(ym(2019, 10).until(ym(1976, 11), {})),
             d(ym(2019, 10).until(ym(1976, 11), () => {})),
             kind(() => ym(2000, 5).until(1)),
             kind(() => ym(2000, 5).until('')),
             kind(() => ym(2000, 5).until({})),
             kind(() => ym(2000, 5).until({ month: 6 })),
             kind(() => ym(2000, 5).until({ year: 2000, month: Infinity })),
             kind(() => ym(2000, 5).until({ year: 2000, month: 6, calendar: 'hebrew' })),
             d(ym(2000, 5).until({ year: 2005, month: 6, calendar: 'iso8601' })),
             kind(() => ym(2019, 1).until(ym(2021, 9), { largestUnit: 'weeks' })),
             kind(() => ym(2019, 1).until(ym(2021, 9), { smallestUnit: 'days' })),
             kind(() => ym(2019, 1).until(ym(2021, 9), { smallestUnit: 'hours' })),
             kind(() => ym(2000, 5).until(ym(2001, 6), { largestUnit: 'bogus' })),
             kind(() => ym(2000, 5).until(ym(2001, 6), { smallestUnit: 'bogus' })),
             kind(() => ym(2000, 5).until(ym(2001, 6), { roundingMode: 'bogus' })),
             kind(() => ym(2000, 5).until(ym(2001, 6), { roundingIncrement: NaN })),
             kind(() => ym(2000, 5).until(ym(2001, 6), { roundingIncrement: 0 })),
             kind(() => ym(2000, 5).until(ym(2001, 6), { roundingIncrement: 1e9 + 1 })),
             ym(2000, 5).until(ym(2001, 6), { roundingIncrement: '1' }).years,
             d(ym(1970, 1).since('-271821-05')),
             kind(() => ym(1970, 1).since('-271821-04')),
             kind(() => ym(1970, 1).since('+275760-10')),
             d(ym(1970, 1).since('+275760-09')),
             d(ym(-271821, 4).until(ym(-271821, 4))),
             kind(() => ym(-271821, 4).until(ym(1970, 1))),
             kind(() => ym(2019, 10).until('-000000-06')),
             d(ym(2022, 1).until(ym(2023, 12), { largestUnit: 'years', smallestUnit: 'months', roundingIncrement: 3, roundingMode: 'expand' })),
             d(ym(2012, 1).until(ym(2013, 1), { smallestUnit: 'months', largestUnit: 'years', roundingIncrement: 2, roundingMode: 'halfEven' })),
             d(ym(2012, 1).until(ym(2011, 1), { smallestUnit: 'months', largestUnit: 'years', roundingIncrement: 2, roundingMode: 'halfEven' })),
             d(ym(2018, 6).until(ym(2019, 12), { smallestUnit: 'years', roundingMode: 'trunc' })),
             d(ym(2018, 6).until(ym(2019, 12), { smallestUnit: 'years', roundingMode: 'halfEven' })),
             d(ym(2017, 6).until(ym(2019, 12), { smallestUnit: 'years', roundingMode: 'halfEven' })),
             d(ym(2017, 6).until(ym(2019, 12), { smallestUnit: 'years', roundingMode: 'halfExpand' })),
             d(ym(2019, 1).until(ym(2021, 9), { smallestUnit: 'years', roundingIncrement: 4, roundingMode: 'halfExpand' })),
             d(ym(2019, 1).until(ym(2021, 9), { smallestUnit: 'months', roundingIncrement: 5 })),
             d(ym(2019, 1).until(ym(2021, 9), { largestUnit: 'months', smallestUnit: 'months', roundingIncrement: 10 })),
             kind(() => ym(1970, 1).until(ym(1971, 1), { roundingIncrement: 100000000 })),
             d(ym(1970, 1).until(ym(1971, 1))),
           ].join('|');
         })()",
    )
    .unwrap();
    assert_eq!(
        str_val(&vm, r),
        "1,1|-1,-1|0,13|1,1|1,1|1,1|RangeError|0,0|0,0|0,0|18,7|18,7|18,7|18,7|18,7|0,0|TypeError|-42,-11|-42,-11|TypeError|RangeError|TypeError|TypeError|RangeError|RangeError|5,1|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|RangeError|1|273790,8|RangeError|RangeError|-273790,-8|0,0|RangeError|RangeError|2,0|1,0|-1,0|1,0|2,0|2,0|3,0|4,0|2,5|0,30|RangeError|1,0"
    );
}
