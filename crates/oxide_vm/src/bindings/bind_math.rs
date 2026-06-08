use std::sync::Arc;

use oxide_kernel::bind_methods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_math(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let math_ptr = kernel.builtin_world().math_object.as_ptr() as *mut JsObject;
    let math = unsafe { &mut *math_ptr };

    let bw = kernel.builtin_world();
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();

    bind_methods!(
        bw,
        math,
        sf,
        sh,
        ("abs", crate::builtins::math::math_abs, 1),
        ("acos", crate::builtins::math::math_acos, 1),
        ("asin", crate::builtins::math::math_asin, 1),
        ("atan", crate::builtins::math::math_atan, 1),
        ("atan2", crate::builtins::math::math_atan2, 2),
        ("cbrt", crate::builtins::math::math_cbrt, 1),
        ("ceil", crate::builtins::math::math_ceil, 1),
        ("cos", crate::builtins::math::math_cos, 1),
        ("cosh", crate::builtins::math::math_cosh, 1),
        ("exp", crate::builtins::math::math_exp, 1),
        ("floor", crate::builtins::math::math_floor, 1),
        ("hypot", crate::builtins::math::math_hypot, 2),
        ("imul", crate::builtins::math::math_imul, 2),
        ("log", crate::builtins::math::math_log, 1),
        ("log10", crate::builtins::math::math_log10, 1),
        ("log2", crate::builtins::math::math_log2, 1),
        ("max", crate::builtins::math::math_max, 2),
        ("min", crate::builtins::math::math_min, 2),
        ("pow", crate::builtins::math::math_pow, 2),
        ("random", crate::builtins::math::math_random, 0),
        ("round", crate::builtins::math::math_round, 1),
        ("sign", crate::builtins::math::math_sign, 1),
        ("sin", crate::builtins::math::math_sin, 1),
        ("sinh", crate::builtins::math::math_sinh, 1),
        ("sqrt", crate::builtins::math::math_sqrt, 1),
        ("tan", crate::builtins::math::math_tan, 1),
        ("tanh", crate::builtins::math::math_tanh, 1),
        ("trunc", crate::builtins::math::math_trunc, 1),
        ("acosh", crate::builtins::math::math_acosh, 1),
        ("asinh", crate::builtins::math::math_asinh, 1),
        ("atanh", crate::builtins::math::math_atanh, 1),
        ("clz32", crate::builtins::math::math_clz32, 1),
        ("expm1", crate::builtins::math::math_expm1, 1),
        ("fround", crate::builtins::math::math_fround, 1),
        ("log1p", crate::builtins::math::math_log1p, 1),
    );

    for (name, val) in [
        ("PI", std::f64::consts::PI),
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        let si = kernel.string_forge().as_ref().intern(name).0;
        let sh_c = kernel.shape_forge().as_ref().make_shape(math.shape_id(), si);
        math.set_shape_id(sh_c);
        math.ensure_hash_props().push(Box::new(JsValue::float(val)));
    }

    let si_m = kernel.string_forge().intern("Math").0;
    let m_shape = kernel.shape_forge().make_shape(global.shape_id(), si_m);
    let m_val = JsValue::from_js_object(kernel.builtin_world().math_object.as_ptr() as *mut JsObject);
    global.set_shape_id(m_shape);
    global.ensure_hash_props().push(Box::new(m_val));
    global.bump_generation();
}
