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

        num_ctor
            .ensure_hash_props()
            .push(Box::new(JsValue::float(2.220446049250313e-16)));
        let eps_si = kernel.string_forge().intern("EPSILON").0;
        let _shape1 = kernel.shape_forge().make_shape(num_ctor.shape_id(), eps_si);
        num_ctor.set_shape_id(_shape1);

        num_ctor
            .ensure_hash_props()
            .push(Box::new(JsValue::float(9007199254740991f64)));
        let max_safe_si = kernel.string_forge().intern("MAX_SAFE_INTEGER").0;
        let _shape2 = kernel
            .shape_forge()
            .make_shape(num_ctor.shape_id(), max_safe_si);
        num_ctor.set_shape_id(_shape2);

        num_ctor
            .ensure_hash_props()
            .push(Box::new(JsValue::float(-9007199254740991f64)));
        let min_safe_si = kernel.string_forge().intern("MIN_SAFE_INTEGER").0;
        let _shape3 = kernel
            .shape_forge()
            .make_shape(num_ctor.shape_id(), min_safe_si);
        num_ctor.set_shape_id(_shape3);

        num_ctor
            .ensure_hash_props()
            .push(Box::new(JsValue::float(1.7976931348623157e308)));
        let max_val_si = kernel.string_forge().intern("MAX_VALUE").0;
        let _shape4 = kernel
            .shape_forge()
            .make_shape(num_ctor.shape_id(), max_val_si);
        num_ctor.set_shape_id(_shape4);

        num_ctor
            .ensure_hash_props()
            .push(Box::new(JsValue::float(5e-324)));
        let min_val_si = kernel.string_forge().intern("MIN_VALUE").0;
        let _shape5 = kernel
            .shape_forge()
            .make_shape(num_ctor.shape_id(), min_val_si);
        num_ctor.set_shape_id(_shape5);

        num_ctor
            .ensure_hash_props()
            .push(Box::new(JsValue::float(f64::NAN)));
        let nan_si = kernel.string_forge().intern("NaN").0;
        let _shape6 = kernel.shape_forge().make_shape(num_ctor.shape_id(), nan_si);
        num_ctor.set_shape_id(_shape6);

        num_ctor
            .ensure_hash_props()
            .push(Box::new(JsValue::float(f64::NEG_INFINITY)));
        let neg_inf_si = kernel.string_forge().intern("NEGATIVE_INFINITY").0;
        let _shape7 = kernel
            .shape_forge()
            .make_shape(num_ctor.shape_id(), neg_inf_si);
        num_ctor.set_shape_id(_shape7);

        num_ctor
            .ensure_hash_props()
            .push(Box::new(JsValue::float(f64::INFINITY)));
        let pos_inf_si = kernel.string_forge().intern("POSITIVE_INFINITY").0;
        let _shape8 = kernel
            .shape_forge()
            .make_shape(num_ctor.shape_id(), pos_inf_si);
        num_ctor.set_shape_id(_shape8);
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
