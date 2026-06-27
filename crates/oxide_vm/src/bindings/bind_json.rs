use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_global_value};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_json(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let json_ptr = session.builtin_world().json_object.as_ptr() as *mut JsObject;
    let json = unsafe { &mut *json_ptr };

    apply_binding_table(
        session.builtin_world(),
        json,
        core,
        &[
            ("parse", oxide_builtins::json::json_parse::<crate::vm::Vm> as *const (), 2),
            ("stringify", oxide_builtins::json::json_stringify::<crate::vm::Vm> as *const (), 3),
        ],
    );

    bind_global_value(
        core,
        global,
        "JSON",
        JsValue::from_js_object(session.builtin_world().json_object.as_ptr() as *mut JsObject),
    );
}
