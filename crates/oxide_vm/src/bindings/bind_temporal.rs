use std::sync::Arc;

use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_global_value, bind_well_known_data_property,
    configure_native_constructor,
};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 Temporal 命名空间及其子对象绑定到 global。
pub fn bind_temporal(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let world = session.builtin_world();
    let temporal_ptr = world.temporal_object.as_ptr() as *mut JsObject;
    let temporal = unsafe { &mut *temporal_ptr };

    // Temporal.Now：非构造器命名空间，方法直接绑定。
    let now_ptr = world.temporal_now_object.as_ptr() as *mut JsObject;
    let now = unsafe { &mut *now_ptr };
    apply_binding_table(
        world,
        now,
        core,
        &[
            ("instant", oxide_builtins::temporal::now_instant::<crate::vm::Vm> as *const (), 0),
            ("timeZoneId", oxide_builtins::temporal::now_time_zone_id::<crate::vm::Vm> as *const (), 0),
        ],
    );

    // Temporal.Instant：构造器 + 静态方法 + 原型 getter 与 toString/valueOf。
    let instant_ctor_ptr = world.instant_constructor.as_ptr() as *mut JsObject;
    let instant_ctor = unsafe { &mut *instant_ctor_ptr };
    configure_native_constructor(
        instant_ctor,
        oxide_builtins::temporal::instant_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(instant_ctor.shape_id(), length_si);
    instant_ctor.set_shape_id(length_shape);
    instant_ctor.ensure_hash_props().push(JsValue::int(1));
    let length_pos = instant_ctor
        .hash_props_vec()
        .map_or(0, |props| props.len() as u32)
        .saturating_sub(1);
    instant_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    apply_binding_table(
        world,
        instant_ctor,
        core,
        &[
            ("from", oxide_builtins::temporal::instant_from::<crate::vm::Vm> as *const (), 1),
            ("compare", oxide_builtins::temporal::instant_compare::<crate::vm::Vm> as *const (), 2),
            (
                "fromEpochMilliseconds",
                oxide_builtins::temporal::instant_from_epoch_milliseconds::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "fromEpochNanoseconds",
                oxide_builtins::temporal::instant_from_epoch_nanoseconds::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );
    let instant_proto_ptr = world.instant_proto.as_ptr() as *mut JsObject;
    let instant_proto = unsafe { &mut *instant_proto_ptr };
    bind_accessor_getter(
        core,
        session,
        instant_proto,
        "epochSeconds",
        oxide_builtins::temporal::instant_epoch_seconds::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        instant_proto,
        "epochMilliseconds",
        oxide_builtins::temporal::instant_epoch_milliseconds::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        instant_proto,
        "epochMicroseconds",
        oxide_builtins::temporal::instant_epoch_microseconds::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        instant_proto,
        "epochNanoseconds",
        oxide_builtins::temporal::instant_epoch_nanoseconds::<crate::vm::Vm> as *const (),
    );
    apply_binding_table(
        world,
        instant_proto,
        core,
        &[
            ("add", oxide_builtins::temporal::instant_add::<crate::vm::Vm> as *const (), 1),
            ("equals", oxide_builtins::temporal::instant_equals::<crate::vm::Vm> as *const (), 1),
            ("round", oxide_builtins::temporal::instant_round::<crate::vm::Vm> as *const (), 1),
            ("since", oxide_builtins::temporal::instant_since::<crate::vm::Vm> as *const (), 1),
            ("subtract", oxide_builtins::temporal::instant_subtract::<crate::vm::Vm> as *const (), 1),
            ("toJSON", oxide_builtins::temporal::instant_to_json::<crate::vm::Vm> as *const (), 0),
            (
                "toLocaleString",
                oxide_builtins::temporal::instant_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            ("toString", oxide_builtins::temporal::instant_to_string::<crate::vm::Vm> as *const (), 0),
            (
                "toZonedDateTimeISO",
                oxide_builtins::temporal::instant_to_zoned_date_time_iso::<crate::vm::Vm> as *const (),
                1,
            ),
            ("until", oxide_builtins::temporal::instant_until::<crate::vm::Vm> as *const (), 1),
            ("valueOf", oxide_builtins::temporal::instant_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );
    bind_well_known_data_property(
        core,
        instant_proto,
        9,
        JsValue::perm_string(
            core.perm_interner()
                .string_ptr(core.perm_interner().intern("Temporal.Instant").0),
        ),
        PropAttributes::new(false, false, true),
    );

    // Temporal.ZonedDateTime：最小构造器与三个稳定内部槽 getter。
    let zoned_date_time_ctor_ptr = world.zoned_date_time_constructor.as_ptr() as *mut JsObject;
    let zoned_date_time_ctor = unsafe { &mut *zoned_date_time_ctor_ptr };
    configure_native_constructor(
        zoned_date_time_ctor,
        oxide_builtins::temporal::zoned_date_time_constructor::<crate::vm::Vm> as *const (),
        2,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(zoned_date_time_ctor.shape_id(), length_si);
    zoned_date_time_ctor.set_shape_id(length_shape);
    zoned_date_time_ctor.ensure_hash_props().push(JsValue::int(2));
    let length_pos = zoned_date_time_ctor
        .hash_props_vec()
        .map_or(0, |props| props.len() as u32)
        .saturating_sub(1);
    zoned_date_time_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));
    apply_binding_table(
        world,
        zoned_date_time_ctor,
        core,
        &[
            ("from", oxide_builtins::temporal::zoned_date_time_from::<crate::vm::Vm> as *const (), 1),
            (
                "compare",
                oxide_builtins::temporal::zoned_date_time_compare::<crate::vm::Vm> as *const (),
                2,
            ),
        ],
    );

    let zoned_date_time_proto_ptr = world.zoned_date_time_proto.as_ptr() as *mut JsObject;
    let zoned_date_time_proto = unsafe { &mut *zoned_date_time_proto_ptr };
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "epochNanoseconds",
        oxide_builtins::temporal::zoned_date_time_epoch_nanoseconds::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "timeZoneId",
        oxide_builtins::temporal::zoned_date_time_time_zone_id::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "calendarId",
        oxide_builtins::temporal::zoned_date_time_calendar_id::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "epochSeconds",
        oxide_builtins::temporal::zoned_date_time_epoch_seconds::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "epochMilliseconds",
        oxide_builtins::temporal::zoned_date_time_epoch_milliseconds::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "epochMicroseconds",
        oxide_builtins::temporal::zoned_date_time_epoch_microseconds::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "offset",
        oxide_builtins::temporal::zoned_date_time_offset::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "offsetNanoseconds",
        oxide_builtins::temporal::zoned_date_time_offset_nanoseconds::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "year",
        oxide_builtins::temporal::zoned_date_time_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "month",
        oxide_builtins::temporal::zoned_date_time_month::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "day",
        oxide_builtins::temporal::zoned_date_time_day::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "hour",
        oxide_builtins::temporal::zoned_date_time_hour::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "minute",
        oxide_builtins::temporal::zoned_date_time_minute::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "second",
        oxide_builtins::temporal::zoned_date_time_second::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "millisecond",
        oxide_builtins::temporal::zoned_date_time_millisecond::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "microsecond",
        oxide_builtins::temporal::zoned_date_time_microsecond::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "nanosecond",
        oxide_builtins::temporal::zoned_date_time_nanosecond::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "dayOfWeek",
        oxide_builtins::temporal::zoned_date_time_day_of_week::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "dayOfYear",
        oxide_builtins::temporal::zoned_date_time_day_of_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "weekOfYear",
        oxide_builtins::temporal::zoned_date_time_week_of_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "yearOfWeek",
        oxide_builtins::temporal::zoned_date_time_year_of_week::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "monthCode",
        oxide_builtins::temporal::zoned_date_time_month_code::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "daysInMonth",
        oxide_builtins::temporal::zoned_date_time_days_in_month::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "daysInWeek",
        oxide_builtins::temporal::zoned_date_time_days_in_week::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "daysInYear",
        oxide_builtins::temporal::zoned_date_time_days_in_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "monthsInYear",
        oxide_builtins::temporal::zoned_date_time_months_in_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "inLeapYear",
        oxide_builtins::temporal::zoned_date_time_in_leap_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "era",
        oxide_builtins::temporal::zoned_date_time_era::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "eraYear",
        oxide_builtins::temporal::zoned_date_time_era_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        zoned_date_time_proto,
        "hoursInDay",
        oxide_builtins::temporal::zoned_date_time_hours_in_day::<crate::vm::Vm> as *const (),
    );
    bind_well_known_data_property(
        core,
        zoned_date_time_proto,
        9,
        JsValue::perm_string(
            core.perm_interner()
                .string_ptr(core.perm_interner().intern("Temporal.ZonedDateTime").0),
        ),
        PropAttributes::new(false, false, true),
    );
    apply_binding_table(
        world,
        zoned_date_time_proto,
        core,
        &[
            (
                "toJSON",
                oxide_builtins::temporal::zoned_date_time_to_json::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toLocaleString",
                oxide_builtins::temporal::zoned_date_time_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toString",
                oxide_builtins::temporal::zoned_date_time_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "valueOf",
                oxide_builtins::temporal::zoned_date_time_value_of::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "withTimeZone",
                oxide_builtins::temporal::zoned_date_time_with_time_zone::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "equals",
                oxide_builtins::temporal::zoned_date_time_equals::<crate::vm::Vm> as *const (),
                1,
            ),
            ("round", oxide_builtins::temporal::zoned_date_time_round::<crate::vm::Vm> as *const (), 1),
            ("until", oxide_builtins::temporal::zoned_date_time_until::<crate::vm::Vm> as *const (), 1),
            ("since", oxide_builtins::temporal::zoned_date_time_since::<crate::vm::Vm> as *const (), 1),
            ("with", oxide_builtins::temporal::zoned_date_time_with::<crate::vm::Vm> as *const (), 1),
            (
                "withCalendar",
                oxide_builtins::temporal::zoned_date_time_with_calendar::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "withPlainTime",
                oxide_builtins::temporal::zoned_date_time_with_plain_time::<crate::vm::Vm> as *const (),
                1,
            ),
            ("add", oxide_builtins::temporal::zoned_date_time_add::<crate::vm::Vm> as *const (), 1),
            (
                "subtract",
                oxide_builtins::temporal::zoned_date_time_subtract::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );

    // Temporal.PlainDate：构造器 + from 静态方法 + year/month/day getter 与 toString。
    let plain_date_ctor_ptr = world.plain_date_constructor.as_ptr() as *mut JsObject;
    let plain_date_ctor = unsafe { &mut *plain_date_ctor_ptr };
    configure_native_constructor(
        plain_date_ctor,
        oxide_builtins::temporal::plain_date_constructor::<crate::vm::Vm> as *const (),
        3,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(plain_date_ctor.shape_id(), length_si);
    plain_date_ctor.set_shape_id(length_shape);
    plain_date_ctor.ensure_hash_props().push(JsValue::int(3));
    let length_pos = plain_date_ctor
        .hash_props_vec()
        .map_or(0, |props| props.len() as u32)
        .saturating_sub(1);
    plain_date_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    apply_binding_table(
        world,
        plain_date_ctor,
        core,
        &[
            ("from", oxide_builtins::temporal::plain_date_from::<crate::vm::Vm> as *const (), 1),
            ("compare", oxide_builtins::temporal::plain_date_compare::<crate::vm::Vm> as *const (), 2),
        ],
    );
    let plain_date_proto_ptr = world.plain_date_proto.as_ptr() as *mut JsObject;
    let plain_date_proto = unsafe { &mut *plain_date_proto_ptr };
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "year",
        oxide_builtins::temporal::plain_date_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "month",
        oxide_builtins::temporal::plain_date_month::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "day",
        oxide_builtins::temporal::plain_date_day::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "dayOfWeek",
        oxide_builtins::temporal::plain_date_day_of_week::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "dayOfYear",
        oxide_builtins::temporal::plain_date_day_of_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "daysInMonth",
        oxide_builtins::temporal::plain_date_days_in_month::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "daysInWeek",
        oxide_builtins::temporal::plain_date_days_in_week::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "daysInYear",
        oxide_builtins::temporal::plain_date_days_in_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "monthsInYear",
        oxide_builtins::temporal::plain_date_months_in_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "inLeapYear",
        oxide_builtins::temporal::plain_date_in_leap_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "weekOfYear",
        oxide_builtins::temporal::plain_date_week_of_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "yearOfWeek",
        oxide_builtins::temporal::plain_date_year_of_week::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "monthCode",
        oxide_builtins::temporal::plain_date_month_code::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "era",
        oxide_builtins::temporal::plain_date_era::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "eraYear",
        oxide_builtins::temporal::plain_date_era_year::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_date_proto,
        "calendarId",
        oxide_builtins::temporal::plain_date_calendar_id::<crate::vm::Vm> as *const (),
    );
    apply_binding_table(
        world,
        plain_date_proto,
        core,
        &[
            (
                "toString",
                oxide_builtins::temporal::plain_date_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            ("toJSON", oxide_builtins::temporal::plain_date_to_json::<crate::vm::Vm> as *const (), 0),
            ("valueOf", oxide_builtins::temporal::plain_date_value_of::<crate::vm::Vm> as *const (), 0),
            ("equals", oxide_builtins::temporal::plain_date_equals::<crate::vm::Vm> as *const (), 1),
            ("add", oxide_builtins::temporal::plain_date_add::<crate::vm::Vm> as *const (), 1),
            ("subtract", oxide_builtins::temporal::plain_date_subtract::<crate::vm::Vm> as *const (), 1),
            ("until", oxide_builtins::temporal::plain_date_until::<crate::vm::Vm> as *const (), 1),
            ("since", oxide_builtins::temporal::plain_date_since::<crate::vm::Vm> as *const (), 1),
        ],
    );

    // Temporal.PlainTime：构造器 + 6 个分量 getter 与 toString。
    let plain_time_ctor_ptr = world.plain_time_constructor.as_ptr() as *mut JsObject;
    let plain_time_ctor = unsafe { &mut *plain_time_ctor_ptr };
    configure_native_constructor(
        plain_time_ctor,
        oxide_builtins::temporal::plain_time_constructor::<crate::vm::Vm> as *const (),
        6,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(plain_time_ctor.shape_id(), length_si);
    plain_time_ctor.set_shape_id(length_shape);
    plain_time_ctor.ensure_hash_props().push(JsValue::int(0));
    let length_pos = plain_time_ctor
        .hash_props_vec()
        .map_or(0, |props| props.len() as u32)
        .saturating_sub(1);
    plain_time_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));
    apply_binding_table(
        world,
        plain_time_ctor,
        core,
        &[
            ("from", oxide_builtins::temporal::plain_time_from::<crate::vm::Vm> as *const (), 1),
            ("compare", oxide_builtins::temporal::plain_time_compare::<crate::vm::Vm> as *const (), 2),
        ],
    );

    let plain_time_proto_ptr = world.plain_time_proto.as_ptr() as *mut JsObject;
    let plain_time_proto = unsafe { &mut *plain_time_proto_ptr };
    bind_accessor_getter(
        core,
        session,
        plain_time_proto,
        "hour",
        oxide_builtins::temporal::plain_time_hour::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_time_proto,
        "minute",
        oxide_builtins::temporal::plain_time_minute::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_time_proto,
        "second",
        oxide_builtins::temporal::plain_time_second::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_time_proto,
        "millisecond",
        oxide_builtins::temporal::plain_time_millisecond::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_time_proto,
        "microsecond",
        oxide_builtins::temporal::plain_time_microsecond::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        core,
        session,
        plain_time_proto,
        "nanosecond",
        oxide_builtins::temporal::plain_time_nanosecond::<crate::vm::Vm> as *const (),
    );
    apply_binding_table(
        world,
        plain_time_proto,
        core,
        &[
            (
                "toString",
                oxide_builtins::temporal::plain_time_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            ("toJSON", oxide_builtins::temporal::plain_time_to_json::<crate::vm::Vm> as *const (), 0),
            (
                "toLocaleString",
                oxide_builtins::temporal::plain_time_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            ("valueOf", oxide_builtins::temporal::plain_time_value_of::<crate::vm::Vm> as *const (), 0),
            ("equals", oxide_builtins::temporal::plain_time_equals::<crate::vm::Vm> as *const (), 1),
            ("until", oxide_builtins::temporal::plain_time_until::<crate::vm::Vm> as *const (), 1),
            ("since", oxide_builtins::temporal::plain_time_since::<crate::vm::Vm> as *const (), 1),
            ("add", oxide_builtins::temporal::plain_time_add::<crate::vm::Vm> as *const (), 1),
            ("subtract", oxide_builtins::temporal::plain_time_subtract::<crate::vm::Vm> as *const (), 1),
            ("round", oxide_builtins::temporal::plain_time_round::<crate::vm::Vm> as *const (), 1),
        ],
    );
    bind_well_known_data_property(
        core,
        plain_time_proto,
        9,
        JsValue::perm_string(
            core.perm_interner()
                .string_ptr(core.perm_interner().intern("Temporal.PlainTime").0),
        ),
        PropAttributes::new(false, false, true),
    );

    // Temporal.PlainDateTime：ISO 日期时间分量、拆分转换与禁止隐式原始值转换。
    let plain_date_time_ctor_ptr = world.plain_date_time_constructor.as_ptr() as *mut JsObject;
    let plain_date_time_ctor = unsafe { &mut *plain_date_time_ctor_ptr };
    configure_native_constructor(
        plain_date_time_ctor,
        oxide_builtins::temporal::plain_date_time_constructor::<crate::vm::Vm> as *const (),
        3,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(plain_date_time_ctor.shape_id(), length_si);
    plain_date_time_ctor.set_shape_id(length_shape);
    plain_date_time_ctor.ensure_hash_props().push(JsValue::int(3));
    let length_pos = plain_date_time_ctor
        .hash_props_vec()
        .map_or(0, |props| props.len() as u32)
        .saturating_sub(1);
    plain_date_time_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));
    apply_binding_table(
        world,
        plain_date_time_ctor,
        core,
        &[
            ("from", oxide_builtins::temporal::plain_date_time_from::<crate::vm::Vm> as *const (), 1),
            (
                "compare",
                oxide_builtins::temporal::plain_date_time_compare::<crate::vm::Vm> as *const (),
                2,
            ),
        ],
    );
    let plain_date_time_proto_ptr = world.plain_date_time_proto.as_ptr() as *mut JsObject;
    let plain_date_time_proto = unsafe { &mut *plain_date_time_proto_ptr };
    for (name, getter) in [
        ("year", oxide_builtins::temporal::plain_date_time_year::<crate::vm::Vm> as *const ()),
        ("month", oxide_builtins::temporal::plain_date_time_month::<crate::vm::Vm> as *const ()),
        ("day", oxide_builtins::temporal::plain_date_time_day::<crate::vm::Vm> as *const ()),
        (
            "dayOfWeek",
            oxide_builtins::temporal::plain_date_time_day_of_week::<crate::vm::Vm> as *const (),
        ),
        (
            "dayOfYear",
            oxide_builtins::temporal::plain_date_time_day_of_year::<crate::vm::Vm> as *const (),
        ),
        (
            "daysInMonth",
            oxide_builtins::temporal::plain_date_time_days_in_month::<crate::vm::Vm> as *const (),
        ),
        (
            "daysInWeek",
            oxide_builtins::temporal::plain_date_time_days_in_week::<crate::vm::Vm> as *const (),
        ),
        (
            "daysInYear",
            oxide_builtins::temporal::plain_date_time_days_in_year::<crate::vm::Vm> as *const (),
        ),
        (
            "monthsInYear",
            oxide_builtins::temporal::plain_date_time_months_in_year::<crate::vm::Vm> as *const (),
        ),
        (
            "inLeapYear",
            oxide_builtins::temporal::plain_date_time_in_leap_year::<crate::vm::Vm> as *const (),
        ),
        (
            "weekOfYear",
            oxide_builtins::temporal::plain_date_time_week_of_year::<crate::vm::Vm> as *const (),
        ),
        (
            "yearOfWeek",
            oxide_builtins::temporal::plain_date_time_year_of_week::<crate::vm::Vm> as *const (),
        ),
        (
            "monthCode",
            oxide_builtins::temporal::plain_date_time_month_code::<crate::vm::Vm> as *const (),
        ),
        ("era", oxide_builtins::temporal::plain_date_time_era::<crate::vm::Vm> as *const ()),
        (
            "eraYear",
            oxide_builtins::temporal::plain_date_time_era_year::<crate::vm::Vm> as *const (),
        ),
        ("hour", oxide_builtins::temporal::plain_date_time_hour::<crate::vm::Vm> as *const ()),
        ("minute", oxide_builtins::temporal::plain_date_time_minute::<crate::vm::Vm> as *const ()),
        ("second", oxide_builtins::temporal::plain_date_time_second::<crate::vm::Vm> as *const ()),
        (
            "millisecond",
            oxide_builtins::temporal::plain_date_time_millisecond::<crate::vm::Vm> as *const (),
        ),
        (
            "microsecond",
            oxide_builtins::temporal::plain_date_time_microsecond::<crate::vm::Vm> as *const (),
        ),
        (
            "nanosecond",
            oxide_builtins::temporal::plain_date_time_nanosecond::<crate::vm::Vm> as *const (),
        ),
        (
            "calendarId",
            oxide_builtins::temporal::plain_date_time_calendar_id::<crate::vm::Vm> as *const (),
        ),
    ] {
        bind_accessor_getter(core, session, plain_date_time_proto, name, getter);
    }
    apply_binding_table(
        world,
        plain_date_time_proto,
        core,
        &[
            (
                "toPlainDate",
                oxide_builtins::temporal::plain_date_time_to_plain_date::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toPlainTime",
                oxide_builtins::temporal::plain_date_time_to_plain_time::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "valueOf",
                oxide_builtins::temporal::plain_date_time_value_of::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toString",
                oxide_builtins::temporal::plain_date_time_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toJSON",
                oxide_builtins::temporal::plain_date_time_to_json::<crate::vm::Vm> as *const (),
                0,
            ),
            ("add", oxide_builtins::temporal::plain_date_time_add::<crate::vm::Vm> as *const (), 1),
            (
                "subtract",
                oxide_builtins::temporal::plain_date_time_subtract::<crate::vm::Vm> as *const (),
                1,
            ),
            ("until", oxide_builtins::temporal::plain_date_time_until::<crate::vm::Vm> as *const (), 1),
            ("since", oxide_builtins::temporal::plain_date_time_since::<crate::vm::Vm> as *const (), 1),
            (
                "equals",
                oxide_builtins::temporal::plain_date_time_equals::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );
    bind_well_known_data_property(
        core,
        plain_date_time_proto,
        9,
        JsValue::perm_string(
            core.perm_interner()
                .string_ptr(core.perm_interner().intern("Temporal.PlainDateTime").0),
        ),
        PropAttributes::new(false, false, true),
    );

    // Temporal.Duration：构造器、from、分量 getter 与 ISO 字符串转换。
    let duration_ctor_ptr = world.duration_constructor.as_ptr() as *mut JsObject;
    let duration_ctor = unsafe { &mut *duration_ctor_ptr };
    configure_native_constructor(
        duration_ctor,
        oxide_builtins::temporal::duration_constructor::<crate::vm::Vm> as *const (),
        10,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(duration_ctor.shape_id(), length_si);
    duration_ctor.set_shape_id(length_shape);
    duration_ctor.ensure_hash_props().push(JsValue::int(0));
    let length_pos = duration_ctor
        .hash_props_vec()
        .map_or(0, |props| props.len() as u32)
        .saturating_sub(1);
    duration_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    apply_binding_table(
        world,
        duration_ctor,
        core,
        &[
            ("from", oxide_builtins::temporal::duration_from::<crate::vm::Vm> as *const (), 1),
            ("compare", oxide_builtins::temporal::duration_compare::<crate::vm::Vm> as *const (), 2),
        ],
    );
    let duration_proto_ptr = world.duration_proto.as_ptr() as *mut JsObject;
    let duration_proto = unsafe { &mut *duration_proto_ptr };
    for (name, getter) in [
        ("years", oxide_builtins::temporal::duration_years::<crate::vm::Vm> as *const ()),
        ("months", oxide_builtins::temporal::duration_months::<crate::vm::Vm> as *const ()),
        ("weeks", oxide_builtins::temporal::duration_weeks::<crate::vm::Vm> as *const ()),
        ("days", oxide_builtins::temporal::duration_days::<crate::vm::Vm> as *const ()),
        ("hours", oxide_builtins::temporal::duration_hours::<crate::vm::Vm> as *const ()),
        ("minutes", oxide_builtins::temporal::duration_minutes::<crate::vm::Vm> as *const ()),
        ("seconds", oxide_builtins::temporal::duration_seconds::<crate::vm::Vm> as *const ()),
        (
            "milliseconds",
            oxide_builtins::temporal::duration_milliseconds::<crate::vm::Vm> as *const (),
        ),
        (
            "microseconds",
            oxide_builtins::temporal::duration_microseconds::<crate::vm::Vm> as *const (),
        ),
        (
            "nanoseconds",
            oxide_builtins::temporal::duration_nanoseconds::<crate::vm::Vm> as *const (),
        ),
        ("sign", oxide_builtins::temporal::duration_sign::<crate::vm::Vm> as *const ()),
        ("blank", oxide_builtins::temporal::duration_blank::<crate::vm::Vm> as *const ()),
    ] {
        bind_accessor_getter(core, session, duration_proto, name, getter);
    }
    apply_binding_table(
        world,
        duration_proto,
        core,
        &[
            ("abs", oxide_builtins::temporal::duration_abs::<crate::vm::Vm> as *const (), 0),
            ("negated", oxide_builtins::temporal::duration_negated::<crate::vm::Vm> as *const (), 0),
            ("add", oxide_builtins::temporal::duration_add::<crate::vm::Vm> as *const (), 1),
            ("subtract", oxide_builtins::temporal::duration_subtract::<crate::vm::Vm> as *const (), 1),
            ("equals", oxide_builtins::temporal::duration_equals::<crate::vm::Vm> as *const (), 1),
            ("round", oxide_builtins::temporal::duration_round::<crate::vm::Vm> as *const (), 1),
            ("with", oxide_builtins::temporal::duration_with::<crate::vm::Vm> as *const (), 1),
            ("total", oxide_builtins::temporal::duration_total::<crate::vm::Vm> as *const (), 1),
            ("toString", oxide_builtins::temporal::duration_to_string::<crate::vm::Vm> as *const (), 0),
            ("toJSON", oxide_builtins::temporal::duration_to_json::<crate::vm::Vm> as *const (), 0),
            (
                "toLocaleString",
                oxide_builtins::temporal::duration_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            ("valueOf", oxide_builtins::temporal::duration_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );
    bind_well_known_data_property(
        core,
        duration_proto,
        9,
        JsValue::perm_string(
            core.perm_interner()
                .string_ptr(core.perm_interner().intern("Temporal.Duration").0),
        ),
        PropAttributes::new(false, false, true),
    );

    // Temporal.PlainMonthDay：构造器、3 个分量 getter 与 toString/toJSON。
    let plain_month_day_ctor_ptr = world.plain_month_day_constructor.as_ptr() as *mut JsObject;
    let plain_month_day_ctor = unsafe { &mut *plain_month_day_ctor_ptr };
    configure_native_constructor(
        plain_month_day_ctor,
        oxide_builtins::temporal::plain_month_day_constructor::<crate::vm::Vm> as *const (),
        2,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(plain_month_day_ctor.shape_id(), length_si);
    plain_month_day_ctor.set_shape_id(length_shape);
    plain_month_day_ctor.ensure_hash_props().push(JsValue::int(2));
    let length_pos = plain_month_day_ctor
        .hash_props_vec()
        .map_or(0, |props| props.len() as u32)
        .saturating_sub(1);
    plain_month_day_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));
    apply_binding_table(
        world,
        plain_month_day_ctor,
        core,
        &[("from", oxide_builtins::temporal::plain_month_day_from::<crate::vm::Vm> as *const (), 1)],
    );

    let plain_month_day_proto_ptr = world.plain_month_day_proto.as_ptr() as *mut JsObject;
    let plain_month_day_proto = unsafe { &mut *plain_month_day_proto_ptr };
    for (name, getter) in [
        ("day", oxide_builtins::temporal::plain_month_day_day::<crate::vm::Vm> as *const ()),
        (
            "monthCode",
            oxide_builtins::temporal::plain_month_day_month_code::<crate::vm::Vm> as *const (),
        ),
        (
            "calendarId",
            oxide_builtins::temporal::plain_month_day_calendar_id::<crate::vm::Vm> as *const (),
        ),
    ] {
        bind_accessor_getter(core, session, plain_month_day_proto, name, getter);
    }
    apply_binding_table(
        world,
        plain_month_day_proto,
        core,
        &[
            (
                "toString",
                oxide_builtins::temporal::plain_month_day_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toJSON",
                oxide_builtins::temporal::plain_month_day_to_json::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toLocaleString",
                oxide_builtins::temporal::plain_month_day_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "valueOf",
                oxide_builtins::temporal::plain_month_day_value_of::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "equals",
                oxide_builtins::temporal::plain_month_day_equals::<crate::vm::Vm> as *const (),
                1,
            ),
            ("with", oxide_builtins::temporal::plain_month_day_with::<crate::vm::Vm> as *const (), 1),
            (
                "toPlainDate",
                oxide_builtins::temporal::plain_month_day_to_plain_date::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );
    bind_well_known_data_property(
        core,
        plain_month_day_proto,
        9,
        JsValue::perm_string(
            core.perm_interner()
                .string_ptr(core.perm_interner().intern("Temporal.PlainMonthDay").0),
        ),
        PropAttributes::new(false, false, true),
    );

    // Temporal.PlainYearMonth：构造器、10 个 getter 与 toString/toJSON。
    let plain_year_month_ctor_ptr = world.plain_year_month_constructor.as_ptr() as *mut JsObject;
    let plain_year_month_ctor = unsafe { &mut *plain_year_month_ctor_ptr };
    configure_native_constructor(
        plain_year_month_ctor,
        oxide_builtins::temporal::plain_year_month_constructor::<crate::vm::Vm> as *const (),
        2,
    );
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(plain_year_month_ctor.shape_id(), length_si);
    plain_year_month_ctor.set_shape_id(length_shape);
    plain_year_month_ctor.ensure_hash_props().push(JsValue::int(2));
    let length_pos = plain_year_month_ctor
        .hash_props_vec()
        .map_or(0, |props| props.len() as u32)
        .saturating_sub(1);
    plain_year_month_ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));
    apply_binding_table(
        world,
        plain_year_month_ctor,
        core,
        &[
            ("from", oxide_builtins::temporal::plain_year_month_from::<crate::vm::Vm> as *const (), 1),
            (
                "compare",
                oxide_builtins::temporal::plain_year_month_compare::<crate::vm::Vm> as *const (),
                2,
            ),
        ],
    );

    let plain_year_month_proto_ptr = world.plain_year_month_proto.as_ptr() as *mut JsObject;
    let plain_year_month_proto = unsafe { &mut *plain_year_month_proto_ptr };
    for (name, getter) in [
        ("year", oxide_builtins::temporal::plain_year_month_year::<crate::vm::Vm> as *const ()),
        ("month", oxide_builtins::temporal::plain_year_month_month::<crate::vm::Vm> as *const ()),
        (
            "monthCode",
            oxide_builtins::temporal::plain_year_month_month_code::<crate::vm::Vm> as *const (),
        ),
        (
            "calendarId",
            oxide_builtins::temporal::plain_year_month_calendar_id::<crate::vm::Vm> as *const (),
        ),
        (
            "daysInMonth",
            oxide_builtins::temporal::plain_year_month_days_in_month::<crate::vm::Vm> as *const (),
        ),
        (
            "daysInYear",
            oxide_builtins::temporal::plain_year_month_days_in_year::<crate::vm::Vm> as *const (),
        ),
        (
            "monthsInYear",
            oxide_builtins::temporal::plain_year_month_months_in_year::<crate::vm::Vm> as *const (),
        ),
        (
            "inLeapYear",
            oxide_builtins::temporal::plain_year_month_in_leap_year::<crate::vm::Vm> as *const (),
        ),
        ("era", oxide_builtins::temporal::plain_year_month_era::<crate::vm::Vm> as *const ()),
        (
            "eraYear",
            oxide_builtins::temporal::plain_year_month_era_year::<crate::vm::Vm> as *const (),
        ),
    ] {
        bind_accessor_getter(core, session, plain_year_month_proto, name, getter);
    }
    apply_binding_table(
        world,
        plain_year_month_proto,
        core,
        &[
            (
                "toString",
                oxide_builtins::temporal::plain_year_month_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toJSON",
                oxide_builtins::temporal::plain_year_month_to_json::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toLocaleString",
                oxide_builtins::temporal::plain_year_month_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "valueOf",
                oxide_builtins::temporal::plain_year_month_value_of::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "equals",
                oxide_builtins::temporal::plain_year_month_equals::<crate::vm::Vm> as *const (),
                1,
            ),
            ("add", oxide_builtins::temporal::plain_year_month_add::<crate::vm::Vm> as *const (), 1),
            (
                "subtract",
                oxide_builtins::temporal::plain_year_month_subtract::<crate::vm::Vm> as *const (),
                1,
            ),
            ("with", oxide_builtins::temporal::plain_year_month_with::<crate::vm::Vm> as *const (), 1),
            (
                "toPlainDate",
                oxide_builtins::temporal::plain_year_month_to_plain_date::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );
    bind_well_known_data_property(
        core,
        plain_year_month_proto,
        9,
        JsValue::perm_string(
            core.perm_interner()
                .string_ptr(core.perm_interner().intern("Temporal.PlainYearMonth").0),
        ),
        PropAttributes::new(false, false, true),
    );

    // 把子对象挂到 Temporal 命名空间对象上，再把 Temporal 挂到 global。
    bind_global_value(core, temporal, "Now", JsValue::from_js_object(now_ptr));
    bind_global_value(core, temporal, "Instant", JsValue::from_js_object(instant_ctor_ptr));
    bind_global_value(core, temporal, "PlainDate", JsValue::from_js_object(plain_date_ctor_ptr));
    bind_global_value(core, temporal, "PlainTime", JsValue::from_js_object(plain_time_ctor_ptr));
    bind_global_value(core, temporal, "PlainDateTime", JsValue::from_js_object(plain_date_time_ctor_ptr));
    bind_global_value(core, temporal, "Duration", JsValue::from_js_object(duration_ctor_ptr));
    bind_global_value(core, temporal, "ZonedDateTime", JsValue::from_js_object(zoned_date_time_ctor_ptr));
    bind_global_value(core, temporal, "PlainMonthDay", JsValue::from_js_object(plain_month_day_ctor_ptr));
    bind_global_value(core, temporal, "PlainYearMonth", JsValue::from_js_object(plain_year_month_ctor_ptr));
    bind_global_value(core, global, "Temporal", JsValue::from_js_object(temporal_ptr));
}
