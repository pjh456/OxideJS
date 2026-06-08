use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_global_value};
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_json(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let json_ptr = kernel.builtin_world().json_object.as_ptr() as *mut JsObject;
    let json = unsafe { &mut *json_ptr };

    apply_binding_table(
        kernel.builtin_world(),
        json,
        kernel,
        &[
            ("parse", crate::builtins::json::json_parse as *const (), 1),
            ("stringify", crate::builtins::json::json_stringify as *const (), 1),
        ],
    );

    bind_global_value(
        kernel,
        global,
        "JSON",
        JsValue::from_js_object(kernel.builtin_world().json_object.as_ptr() as *mut JsObject),
    );
}
