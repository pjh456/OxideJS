use crate::bindings::{apply_binding_table, bind_global_value, BindEnv};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// 把 JSON 单例对象及其方法（parse/stringify 等）绑定到 global。
pub fn bind_json(env: BindEnv<'_>, global: &mut JsObject) {
    let json_ptr = env.world().json_object.as_ptr() as *mut JsObject;
    let json = unsafe { &mut *json_ptr };

    apply_binding_table(
        env,
        json,
        &[
            ("parse", oxide_builtins::json::json_parse::<crate::vm::Vm> as *const (), 2),
            ("stringify", oxide_builtins::json::json_stringify::<crate::vm::Vm> as *const (), 3),
            ("rawJSON", oxide_builtins::json::json_raw_json::<crate::vm::Vm> as *const (), 1),
            ("isRawJSON", oxide_builtins::json::json_is_raw_json::<crate::vm::Vm> as *const (), 1),
        ],
    );

    bind_global_value(
        env.core,
        global,
        "JSON",
        JsValue::from_js_object(env.world().json_object.as_ptr() as *mut JsObject),
    );
}
