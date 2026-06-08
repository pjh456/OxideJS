use std::sync::Arc;

use oxide_kernel::bind_methods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_date(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().date_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().date_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();

    ctor.set_native_fn(Some(crate::builtins::date::date_constructor as *const ()));
    ctor.set_native_arg_count(7);

    bind_methods!(
        kernel.builtin_world(),
        ctor,
        sf,
        sh,
        ("now", crate::builtins::date::date_now, 0),
        ("parse", crate::builtins::date::date_parse, 1),
        ("UTC", crate::builtins::date::date_utc, 7),
    );

    bind_methods!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        ("getTime", crate::builtins::date::date_get_time, 0),
        ("getFullYear", crate::builtins::date::date_get_full_year, 0),
        ("getMonth", crate::builtins::date::date_get_month, 0),
        ("getDate", crate::builtins::date::date_get_date, 0),
        ("getDay", crate::builtins::date::date_get_day, 0),
        ("getHours", crate::builtins::date::date_get_hours, 0),
        ("getMinutes", crate::builtins::date::date_get_minutes, 0),
        ("getSeconds", crate::builtins::date::date_get_seconds, 0),
        (
            "getMilliseconds",
            crate::builtins::date::date_get_milliseconds,
            0
        ),
        (
            "getUTCFullYear",
            crate::builtins::date::date_get_utc_full_year,
            0
        ),
        ("getUTCMonth", crate::builtins::date::date_get_utc_month, 0),
        ("getUTCDate", crate::builtins::date::date_get_utc_date, 0),
        ("getUTCDay", crate::builtins::date::date_get_utc_day, 0),
        ("getUTCHours", crate::builtins::date::date_get_utc_hours, 0),
        (
            "getUTCMinutes",
            crate::builtins::date::date_get_utc_minutes,
            0
        ),
        (
            "getUTCSeconds",
            crate::builtins::date::date_get_utc_seconds,
            0
        ),
        (
            "getUTCMilliseconds",
            crate::builtins::date::date_get_utc_milliseconds,
            0
        ),
        (
            "getTimezoneOffset",
            crate::builtins::date::date_get_timezone_offset,
            0
        ),
        ("setTime", crate::builtins::date::date_set_time, 1),
        ("setFullYear", crate::builtins::date::date_set_full_year, 3),
        ("setMonth", crate::builtins::date::date_set_month, 2),
        ("setDate", crate::builtins::date::date_set_date, 1),
        ("setHours", crate::builtins::date::date_set_hours, 4),
        ("setMinutes", crate::builtins::date::date_set_minutes, 3),
        ("setSeconds", crate::builtins::date::date_set_seconds, 2),
        (
            "setMilliseconds",
            crate::builtins::date::date_set_milliseconds,
            1
        ),
        ("toISOString", crate::builtins::date::date_to_iso_string, 0),
        ("toJSON", crate::builtins::date::date_to_json, 1),
        ("toString", crate::builtins::date::date_to_string, 0),
        (
            "toDateString",
            crate::builtins::date::date_to_date_string,
            0
        ),
        (
            "toTimeString",
            crate::builtins::date::date_to_time_string,
            0
        ),
        ("toUTCString", crate::builtins::date::date_to_utc_string, 0),
        ("valueOf", crate::builtins::date::date_value_of, 0),
    );

    let si_d = kernel.string_forge().intern("Date").0;
    let d_shape = kernel.shape_forge().make_shape(global.shape_id(), si_d);
    let d_val = JsValue::from_js_object(ctor_ptr);
    global.set_shape_id(d_shape);
    global.ensure_hash_props().push(Box::new(d_val));
    global.bump_generation();
}
