use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_accessor_getter, bind_global_value, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// 把 Temporal 命名空间及其 Now/Instant/PlainDate/PlainTime 子对象绑定到 global。
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
            (
                "timeZoneId",
                oxide_builtins::temporal::now_time_zone_id::<crate::vm::Vm> as *const (),
                0,
            ),
        ],
    );

    // Temporal.Instant：构造器 + from 静态方法 + 原型 getter 与 toString/valueOf。
    let instant_ctor_ptr = world.instant_constructor.as_ptr() as *mut JsObject;
    let instant_ctor = unsafe { &mut *instant_ctor_ptr };
    configure_native_constructor(
        instant_ctor,
        oxide_builtins::temporal::instant_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    apply_binding_table(
        world,
        instant_ctor,
        core,
        &[("from", oxide_builtins::temporal::instant_from::<crate::vm::Vm> as *const (), 1)],
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
            ("toString", oxide_builtins::temporal::instant_to_string::<crate::vm::Vm> as *const (), 0),
            ("valueOf", oxide_builtins::temporal::instant_value_of::<crate::vm::Vm> as *const (), 0),
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
    apply_binding_table(
        world,
        plain_date_ctor,
        core,
        &[("from", oxide_builtins::temporal::plain_date_from::<crate::vm::Vm> as *const (), 1)],
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
    apply_binding_table(
        world,
        plain_date_proto,
        core,
        &[("toString", oxide_builtins::temporal::plain_date_to_string::<crate::vm::Vm> as *const (), 0)],
    );

    // Temporal.PlainTime：构造器 + 6 个分量 getter 与 toString。
    let plain_time_ctor_ptr = world.plain_time_constructor.as_ptr() as *mut JsObject;
    let plain_time_ctor = unsafe { &mut *plain_time_ctor_ptr };
    configure_native_constructor(
        plain_time_ctor,
        oxide_builtins::temporal::plain_time_constructor::<crate::vm::Vm> as *const (),
        6,
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
        &[("toString", oxide_builtins::temporal::plain_time_to_string::<crate::vm::Vm> as *const (), 0)],
    );

    // 把子对象挂到 Temporal 命名空间对象上，再把 Temporal 挂到 global。
    bind_global_value(core, temporal, "Now", JsValue::from_js_object(now_ptr));
    bind_global_value(core, temporal, "Instant", JsValue::from_js_object(instant_ctor_ptr));
    bind_global_value(core, temporal, "PlainDate", JsValue::from_js_object(plain_date_ctor_ptr));
    bind_global_value(core, temporal, "PlainTime", JsValue::from_js_object(plain_time_ctor_ptr));
    bind_global_value(core, global, "Temporal", JsValue::from_js_object(temporal_ptr));
}
