use std::sync::Arc;

use crate::bind_constructor_hash;
use oxide_kernel::builtin::ErrorMethods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_error(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let error_methods = ErrorMethods {
        error: crate::builtins::error::error_constructor as *const (),
        type_error: crate::builtins::error::type_error_constructor as *const (),
        reference_error: crate::builtins::error::reference_error_constructor as *const (),
        range_error: crate::builtins::error::range_error_constructor as *const (),
        syntax_error: crate::builtins::error::syntax_error_constructor as *const (),
        uri_error: crate::builtins::error::uri_error_constructor as *const (),
        eval_error: crate::builtins::error::eval_error_constructor as *const (),
        to_string: crate::builtins::error::error_to_string as *const (),
        stack: crate::builtins::error::error_stack_getter as *const (),
    };
    kernel.builtin_world().bind_error_methods(
        &error_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_err = kernel.string_forge().intern("Error").0;
    let err_shape = kernel.shape_forge().make_shape(global.shape_id(), si_err);
    let err_val =
        JsValue::from_js_object(kernel.builtin_world().error_constructor.as_ptr() as *mut JsObject);
    global.set_shape_id(err_shape);
    global.ensure_hash_props().push(Box::new(err_val));
    global.bump_generation();

    bind_constructor_hash!(kernel, global, "TypeError", kernel.builtin_world().type_error_proto.as_ptr() as *mut JsObject, crate::builtins::error::type_error_constructor, 1);
    bind_constructor_hash!(kernel, global, "ReferenceError", kernel.builtin_world().reference_error_proto.as_ptr() as *mut JsObject, crate::builtins::error::reference_error_constructor, 1);
    bind_constructor_hash!(kernel, global, "RangeError", kernel.builtin_world().range_error_proto.as_ptr() as *mut JsObject, crate::builtins::error::range_error_constructor, 1);
    bind_constructor_hash!(kernel, global, "SyntaxError", kernel.builtin_world().syntax_error_proto.as_ptr() as *mut JsObject, crate::builtins::error::syntax_error_constructor, 1);
    bind_constructor_hash!(kernel, global, "URIError", kernel.builtin_world().uri_error_proto.as_ptr() as *mut JsObject, crate::builtins::error::uri_error_constructor, 1);
    bind_constructor_hash!(kernel, global, "EvalError", kernel.builtin_world().eval_error_proto.as_ptr() as *mut JsObject, crate::builtins::error::eval_error_constructor, 1);

    {
        let err_ctor_ptr = kernel.builtin_world().error_constructor.as_ptr() as *mut JsObject;
        let err_ctor = unsafe { &mut *err_ctor_ptr };
        err_ctor.set_native_fn(Some(crate::builtins::error::error_constructor as *const ()));
    }
}
