use std::sync::Arc;

use oxide_kernel::bind_methods;
use oxide_kernel::builtin::NumberMethods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

pub fn bind_number(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let number_methods = NumberMethods {
        is_nan: crate::builtins::number::number_is_nan as *const (),
        is_finite: crate::builtins::number::number_is_finite as *const (),
        parse_int: crate::builtins::number::number_parse_int as *const (),
        parse_float: crate::builtins::number::number_parse_float as *const (),
        to_string: crate::builtins::number::number_to_string as *const (),
        to_fixed: crate::builtins::number::number_to_fixed as *const (),
        is_integer: crate::builtins::number::number_is_integer as *const (),
        is_safe_integer: crate::builtins::number::number_is_safe_integer as *const (),
        to_exponential: crate::builtins::number::number_to_exponential as *const (),
        to_precision: crate::builtins::number::number_to_precision as *const (),
        value_of: crate::builtins::number::number_value_of as *const (),
    };
    kernel.builtin_world().bind_number_methods(
        &number_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_num = kernel.string_forge().intern("Number").0;
    let num_shape = kernel.shape_forge().make_shape(global.shape_id(), si_num);
    let num_val = JsValue::from_js_object(kernel.builtin_world().number_constructor.as_ptr() as *mut JsObject);
    global.set_shape_id(num_shape);
    global.ensure_hash_props().push(Box::new(num_val));
    global.bump_generation();

    {
        let num_ctor_ptr = kernel.builtin_world().number_constructor.as_ptr() as *mut JsObject;
        let num_ctor = unsafe { &mut *num_ctor_ptr };
        // SAFETY: number_constructor is a NativeFn fn-item.
        num_ctor.set_native_fn(Some(unsafe {
            NativeFnPtr::from_raw(crate::builtins::number::number_constructor as *const ())
        }));
        num_ctor.set_native_arg_count(1);

        for (name, value) in [
            ("EPSILON", JsValue::float(2.220446049250313e-16)),
            ("MAX_SAFE_INTEGER", JsValue::float(9007199254740991f64)),
            ("MIN_SAFE_INTEGER", JsValue::float(-9007199254740991f64)),
            ("MAX_VALUE", JsValue::float(1.7976931348623157e308)),
            ("MIN_VALUE", JsValue::float(5e-324)),
            ("NaN", JsValue::float(f64::NAN)),
            ("NEGATIVE_INFINITY", JsValue::float(f64::NEG_INFINITY)),
            ("POSITIVE_INFINITY", JsValue::float(f64::INFINITY)),
        ] {
            num_ctor.ensure_hash_props().push(Box::new(value));
            let prop_si = kernel.string_forge().intern(name).0;
            let next_shape = kernel.shape_forge().make_shape(num_ctor.shape_id(), prop_si);
            num_ctor.set_shape_id(next_shape);
        }
    }

    let pi_fn = crate::builtins::number::number_parse_int as *const ();
    let pf_fn = crate::builtins::number::number_parse_float as *const ();
    bind_methods!(
        kernel.builtin_world(),
        global,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        ("parseInt", pi_fn, 1),
        ("parseFloat", pf_fn, 1),
    );
}
