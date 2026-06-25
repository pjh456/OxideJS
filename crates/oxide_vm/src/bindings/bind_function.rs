use std::sync::Arc;

use oxide_kernel::builtin::FunctionMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::bind_global_value;

pub fn bind_function(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let function_methods = FunctionMethods {
        call: oxide_builtins::function::function_call::<crate::vm::Vm> as *const (),
        apply: oxide_builtins::function::function_apply::<crate::vm::Vm> as *const (),
        bind: oxide_builtins::function::function_bind::<crate::vm::Vm> as *const (),
        to_string: oxide_builtins::function::function_to_string::<crate::vm::Vm> as *const (),
    };
    session.builtin_world().bind_function_methods(
        &function_methods,
        core.perm_interner().as_ref(),
        core.shape_forge().as_ref(),
    );

    let function_ctor = session.builtin_world().function_constructor.as_ptr() as *mut JsObject;
    bind_global_value(core, global, "Function", JsValue::from_js_object(function_ctor));
}
