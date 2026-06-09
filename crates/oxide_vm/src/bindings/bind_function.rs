use std::sync::Arc;

use oxide_kernel::builtin::FunctionMethods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::bind_global_value;

pub fn bind_function(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let function_methods = FunctionMethods {
        call: crate::builtins::function::function_call as *const (),
        apply: crate::builtins::function::function_apply as *const (),
        bind: crate::builtins::function::function_bind as *const (),
        to_string: crate::builtins::function::function_to_string as *const (),
    };
    kernel.builtin_world().bind_function_methods(
        &function_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let function_ctor = kernel.builtin_world().function_constructor.as_ptr() as *mut JsObject;
    bind_global_value(kernel, global, "Function", JsValue::from_js_object(function_ctor));
}
