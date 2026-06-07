use std::sync::Arc;

use oxide_kernel::bind_method;
use oxide_kernel::builtin::NumberMethods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_number(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let number_methods = NumberMethods {
        is_nan: crate::builtins::number::number_is_nan as *const (),
        is_finite: crate::builtins::number::number_is_finite as *const (),
        parse_int: crate::builtins::number::number_parse_int as *const (),
        parse_float: crate::builtins::number::number_parse_float as *const (),
        to_string: crate::builtins::number::number_to_string as *const (),
        to_fixed: crate::builtins::number::number_to_fixed as *const (),
    };
    kernel.builtin_world().bind_number_methods(
        &number_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_num = kernel.string_forge().intern("Number").0;
    let num_shape = kernel.shape_forge().make_shape(global.shape_id(), si_num);
    let num_val = JsValue::from_js_object(
        kernel.builtin_world().number_constructor.as_ptr() as *mut JsObject
    );
    global.set_shape_id(num_shape);
    global.ensure_hash_props().push(Box::new(num_val));
    global.bump_generation();

    {
        let num_ctor_ptr = kernel.builtin_world().number_constructor.as_ptr() as *mut JsObject;
        let num_ctor = unsafe { &mut *num_ctor_ptr };
        num_ctor.set_native_fn(Some(
            crate::builtins::number::number_constructor as *const (),
        ));
        num_ctor.set_native_arg_count(1);
    }

    let pi_fn = crate::builtins::number::number_parse_int as *const ();
    let pf_fn = crate::builtins::number::number_parse_float as *const ();
    bind_method!(
        kernel.builtin_world(),
        global,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "parseInt",
        pi_fn,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        global,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "parseFloat",
        pf_fn,
        1
    );
}
