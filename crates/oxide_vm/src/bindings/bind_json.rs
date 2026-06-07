use std::sync::Arc;

use oxide_kernel::bind_method;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_json(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let json_ptr = kernel.builtin_world().json_object.as_ptr() as *mut JsObject;
    let json = unsafe { &mut *json_ptr };

    bind_method!(
        kernel.builtin_world(),
        json,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "parse",
        crate::builtins::json::json_parse,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        json,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "stringify",
        crate::builtins::json::json_stringify,
        1
    );

    let si_j = kernel.string_forge().intern("JSON").0;
    let j_shape = kernel.shape_forge().make_shape(global.shape_id(), si_j);
    let j_val =
        JsValue::from_js_object(kernel.builtin_world().json_object.as_ptr() as *mut JsObject);
    global.set_shape_id(j_shape);
    global.ensure_hash_props().push(Box::new(j_val));
    global.bump_generation();
}
