use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_global_value, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 Date 构造器与原型方法绑定到 global。
pub fn bind_date(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().date_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().date_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::date::date_constructor::<crate::vm::Vm> as *const (), 7);
    // Date.length = 7，描述符 { writable:false, enumerable:false, configurable:true }。
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(ctor.shape_id(), length_si);
    ctor.set_shape_id(length_shape);
    ctor.ensure_hash_props().push(JsValue::int(7));
    let length_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[
            ("now", oxide_builtins::date::date_now::<crate::vm::Vm> as *const (), 0),
            ("parse", oxide_builtins::date::date_parse::<crate::vm::Vm> as *const (), 1),
            ("UTC", oxide_builtins::date::date_utc::<crate::vm::Vm> as *const (), 7),
        ],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("getTime", oxide_builtins::date::date_get_time::<crate::vm::Vm> as *const (), 0),
            ("getFullYear", oxide_builtins::date::date_get_full_year::<crate::vm::Vm> as *const (), 0),
            ("getMonth", oxide_builtins::date::date_get_month::<crate::vm::Vm> as *const (), 0),
            ("getDate", oxide_builtins::date::date_get_date::<crate::vm::Vm> as *const (), 0),
            ("getDay", oxide_builtins::date::date_get_day::<crate::vm::Vm> as *const (), 0),
            ("getHours", oxide_builtins::date::date_get_hours::<crate::vm::Vm> as *const (), 0),
            ("getMinutes", oxide_builtins::date::date_get_minutes::<crate::vm::Vm> as *const (), 0),
            ("getSeconds", oxide_builtins::date::date_get_seconds::<crate::vm::Vm> as *const (), 0),
            (
                "getMilliseconds",
                oxide_builtins::date::date_get_milliseconds::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "getUTCFullYear",
                oxide_builtins::date::date_get_utc_full_year::<crate::vm::Vm> as *const (),
                0,
            ),
            ("getUTCMonth", oxide_builtins::date::date_get_utc_month::<crate::vm::Vm> as *const (), 0),
            ("getUTCDate", oxide_builtins::date::date_get_utc_date::<crate::vm::Vm> as *const (), 0),
            ("getUTCDay", oxide_builtins::date::date_get_utc_day::<crate::vm::Vm> as *const (), 0),
            ("getUTCHours", oxide_builtins::date::date_get_utc_hours::<crate::vm::Vm> as *const (), 0),
            (
                "getUTCMinutes",
                oxide_builtins::date::date_get_utc_minutes::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "getUTCSeconds",
                oxide_builtins::date::date_get_utc_seconds::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "getUTCMilliseconds",
                oxide_builtins::date::date_get_utc_milliseconds::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "getTimezoneOffset",
                oxide_builtins::date::date_get_timezone_offset::<crate::vm::Vm> as *const (),
                0,
            ),
            ("setTime", oxide_builtins::date::date_set_time::<crate::vm::Vm> as *const (), 1),
            ("setFullYear", oxide_builtins::date::date_set_full_year::<crate::vm::Vm> as *const (), 3),
            ("setMonth", oxide_builtins::date::date_set_month::<crate::vm::Vm> as *const (), 2),
            ("setDate", oxide_builtins::date::date_set_date::<crate::vm::Vm> as *const (), 1),
            ("setHours", oxide_builtins::date::date_set_hours::<crate::vm::Vm> as *const (), 4),
            ("setMinutes", oxide_builtins::date::date_set_minutes::<crate::vm::Vm> as *const (), 3),
            ("setSeconds", oxide_builtins::date::date_set_seconds::<crate::vm::Vm> as *const (), 2),
            (
                "setMilliseconds",
                oxide_builtins::date::date_set_milliseconds::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "setUTCFullYear",
                oxide_builtins::date::date_set_utc_full_year::<crate::vm::Vm> as *const (),
                3,
            ),
            ("setUTCMonth", oxide_builtins::date::date_set_utc_month::<crate::vm::Vm> as *const (), 2),
            ("setUTCDate", oxide_builtins::date::date_set_utc_date::<crate::vm::Vm> as *const (), 1),
            ("setUTCHours", oxide_builtins::date::date_set_utc_hours::<crate::vm::Vm> as *const (), 4),
            (
                "setUTCMinutes",
                oxide_builtins::date::date_set_utc_minutes::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "setUTCSeconds",
                oxide_builtins::date::date_set_utc_seconds::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setUTCMilliseconds",
                oxide_builtins::date::date_set_utc_milliseconds::<crate::vm::Vm> as *const (),
                1,
            ),
            ("getYear", oxide_builtins::date::date_get_year::<crate::vm::Vm> as *const (), 0),
            ("setYear", oxide_builtins::date::date_set_year::<crate::vm::Vm> as *const (), 1),
            ("toISOString", oxide_builtins::date::date_to_iso_string::<crate::vm::Vm> as *const (), 0),
            ("toJSON", oxide_builtins::date::date_to_json::<crate::vm::Vm> as *const (), 1),
            ("toString", oxide_builtins::date::date_to_string::<crate::vm::Vm> as *const (), 0),
            ("toDateString", oxide_builtins::date::date_to_date_string::<crate::vm::Vm> as *const (), 0),
            ("toTimeString", oxide_builtins::date::date_to_time_string::<crate::vm::Vm> as *const (), 0),
            ("toUTCString", oxide_builtins::date::date_to_utc_string::<crate::vm::Vm> as *const (), 0),
            ("toGMTString", oxide_builtins::date::date_to_gmt_string::<crate::vm::Vm> as *const (), 0),
            (
                "toLocaleDateString",
                oxide_builtins::date::date_to_locale_date_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toLocaleString",
                oxide_builtins::date::date_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toLocaleTimeString",
                oxide_builtins::date::date_to_locale_time_string::<crate::vm::Vm> as *const (),
                0,
            ),
            ("valueOf", oxide_builtins::date::date_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );

    bind_global_value(core, global, "Date", JsValue::from_js_object(ctor_ptr));
}
