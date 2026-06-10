use std::sync::Arc;

use oxide_kernel::builtin::ErrorMethods;
use oxide_kernel::kernel::OxideKernel;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

fn bind_error_subtype_constructor(
    kernel: &Arc<OxideKernel>, global: &mut JsObject, name: &str, proto_ptr: *mut JsObject, ctor_fn: *const (),
) {
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();
    let function_proto_ptr = kernel.builtin_world().function_proto.as_ptr() as *mut JsObject;

    let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto_ptr)));
    ctor.set_function(true);
    // SAFETY: ctor_fn is a NativeFn fn-item pointer cast to *const () by callers.
    ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(ctor_fn) }));
    ctor.set_native_arg_count(1);

    let si_prototype = sf.intern("prototype").0;
    let si_name = sf.intern("name").0;
    let name_si = sf.intern(name).0;

    let ctor_shape1 = sh.make_shape(EMPTY_SHAPE_ID, si_prototype);
    let ctor_shape2 = sh.make_shape(ctor_shape1, si_name);
    ctor.set_shape_id(ctor_shape2);
    ctor.ensure_hash_props().push(Box::new(JsValue::from_js_object(proto_ptr)));
    ctor.ensure_hash_props().push(Box::new(JsValue::string(name_si, 0)));

    let ctor_ptr = Box::into_raw(ctor);

    let proto = unsafe { &mut *proto_ptr };
    let proto_ctor_shape = sh.make_shape(proto.shape_id(), sf.intern("constructor").0);
    proto.set_shape_id(proto_ctor_shape);
    proto.ensure_hash_props().push(Box::new(JsValue::from_js_object(ctor_ptr)));

    let global_shape = sh.make_shape(global.shape_id(), name_si);
    global.set_shape_id(global_shape);
    global.ensure_hash_props().push(Box::new(JsValue::from_js_object(ctor_ptr)));
    global.bump_generation();
}

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
    let err_val = JsValue::from_js_object(kernel.builtin_world().error_constructor.as_ptr() as *mut JsObject);
    global.set_shape_id(err_shape);
    global.ensure_hash_props().push(Box::new(err_val));
    global.bump_generation();

    bind_error_subtype_constructor(
        kernel,
        global,
        "TypeError",
        kernel.builtin_world().type_error_proto.as_ptr() as *mut JsObject,
        crate::builtins::error::type_error_constructor as *const (),
    );
    bind_error_subtype_constructor(
        kernel,
        global,
        "ReferenceError",
        kernel.builtin_world().reference_error_proto.as_ptr() as *mut JsObject,
        crate::builtins::error::reference_error_constructor as *const (),
    );
    bind_error_subtype_constructor(
        kernel,
        global,
        "RangeError",
        kernel.builtin_world().range_error_proto.as_ptr() as *mut JsObject,
        crate::builtins::error::range_error_constructor as *const (),
    );
    bind_error_subtype_constructor(
        kernel,
        global,
        "SyntaxError",
        kernel.builtin_world().syntax_error_proto.as_ptr() as *mut JsObject,
        crate::builtins::error::syntax_error_constructor as *const (),
    );
    bind_error_subtype_constructor(
        kernel,
        global,
        "URIError",
        kernel.builtin_world().uri_error_proto.as_ptr() as *mut JsObject,
        crate::builtins::error::uri_error_constructor as *const (),
    );
    bind_error_subtype_constructor(
        kernel,
        global,
        "EvalError",
        kernel.builtin_world().eval_error_proto.as_ptr() as *mut JsObject,
        crate::builtins::error::eval_error_constructor as *const (),
    );

    {
        let err_ctor_ptr = kernel.builtin_world().error_constructor.as_ptr() as *mut JsObject;
        let err_ctor = unsafe { &mut *err_ctor_ptr };
        // SAFETY: error_constructor is a NativeFn fn-item.
        err_ctor.set_native_fn(Some(unsafe {
            NativeFnPtr::from_raw(crate::builtins::error::error_constructor as *const ())
        }));
    }
}
