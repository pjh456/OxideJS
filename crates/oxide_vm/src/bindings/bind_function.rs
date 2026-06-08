use std::sync::Arc;

use oxide_kernel::builtin::FunctionMethods;
use oxide_kernel::kernel::OxideKernel;

pub fn bind_function(kernel: &Arc<OxideKernel>) {
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
}
