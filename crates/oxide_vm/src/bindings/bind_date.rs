use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_global_value, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_date(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().date_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().date_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, crate::builtins::date::date_constructor as *const (), 7);

    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[
            ("now", crate::builtins::date::date_now as *const (), 0),
            ("parse", crate::builtins::date::date_parse as *const (), 1),
            ("UTC", crate::builtins::date::date_utc as *const (), 7),
        ],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("getTime", crate::builtins::date::date_get_time as *const (), 0),
            ("getFullYear", crate::builtins::date::date_get_full_year as *const (), 0),
            ("getMonth", crate::builtins::date::date_get_month as *const (), 0),
            ("getDate", crate::builtins::date::date_get_date as *const (), 0),
            ("getDay", crate::builtins::date::date_get_day as *const (), 0),
            ("getHours", crate::builtins::date::date_get_hours as *const (), 0),
            ("getMinutes", crate::builtins::date::date_get_minutes as *const (), 0),
            ("getSeconds", crate::builtins::date::date_get_seconds as *const (), 0),
            ("getMilliseconds", crate::builtins::date::date_get_milliseconds as *const (), 0),
            ("getUTCFullYear", crate::builtins::date::date_get_utc_full_year as *const (), 0),
            ("getUTCMonth", crate::builtins::date::date_get_utc_month as *const (), 0),
            ("getUTCDate", crate::builtins::date::date_get_utc_date as *const (), 0),
            ("getUTCDay", crate::builtins::date::date_get_utc_day as *const (), 0),
            ("getUTCHours", crate::builtins::date::date_get_utc_hours as *const (), 0),
            ("getUTCMinutes", crate::builtins::date::date_get_utc_minutes as *const (), 0),
            ("getUTCSeconds", crate::builtins::date::date_get_utc_seconds as *const (), 0),
            ("getUTCMilliseconds", crate::builtins::date::date_get_utc_milliseconds as *const (), 0),
            ("getTimezoneOffset", crate::builtins::date::date_get_timezone_offset as *const (), 0),
            ("setTime", crate::builtins::date::date_set_time as *const (), 1),
            ("setFullYear", crate::builtins::date::date_set_full_year as *const (), 3),
            ("setMonth", crate::builtins::date::date_set_month as *const (), 2),
            ("setDate", crate::builtins::date::date_set_date as *const (), 1),
            ("setHours", crate::builtins::date::date_set_hours as *const (), 4),
            ("setMinutes", crate::builtins::date::date_set_minutes as *const (), 3),
            ("setSeconds", crate::builtins::date::date_set_seconds as *const (), 2),
            ("setMilliseconds", crate::builtins::date::date_set_milliseconds as *const (), 1),
            ("toISOString", crate::builtins::date::date_to_iso_string as *const (), 0),
            ("toJSON", crate::builtins::date::date_to_json as *const (), 1),
            ("toString", crate::builtins::date::date_to_string as *const (), 0),
            ("toDateString", crate::builtins::date::date_to_date_string as *const (), 0),
            ("toTimeString", crate::builtins::date::date_to_time_string as *const (), 0),
            ("toUTCString", crate::builtins::date::date_to_utc_string as *const (), 0),
            ("valueOf", crate::builtins::date::date_value_of as *const (), 0),
        ],
    );

    bind_global_value(core, global, "Date", JsValue::from_js_object(ctor_ptr));
}
