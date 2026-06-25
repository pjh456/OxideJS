use std::sync::Arc;

use oxide_kernel::bind_methods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_math(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let math_ptr = session.builtin_world().math_object.as_ptr() as *mut JsObject;
    let math = unsafe { &mut *math_ptr };

    let bw = session.builtin_world();
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();

    bind_methods!(
        bw,
        math,
        sf,
        sh,
        ("abs", oxide_builtins::math::math_abs::<crate::vm::Vm>, 1),
        ("acos", oxide_builtins::math::math_acos::<crate::vm::Vm>, 1),
        ("asin", oxide_builtins::math::math_asin::<crate::vm::Vm>, 1),
        ("atan", oxide_builtins::math::math_atan::<crate::vm::Vm>, 1),
        ("atan2", oxide_builtins::math::math_atan2::<crate::vm::Vm>, 2),
        ("cbrt", oxide_builtins::math::math_cbrt::<crate::vm::Vm>, 1),
        ("ceil", oxide_builtins::math::math_ceil::<crate::vm::Vm>, 1),
        ("cos", oxide_builtins::math::math_cos::<crate::vm::Vm>, 1),
        ("cosh", oxide_builtins::math::math_cosh::<crate::vm::Vm>, 1),
        ("exp", oxide_builtins::math::math_exp::<crate::vm::Vm>, 1),
        ("floor", oxide_builtins::math::math_floor::<crate::vm::Vm>, 1),
        ("hypot", oxide_builtins::math::math_hypot::<crate::vm::Vm>, 2),
        ("imul", oxide_builtins::math::math_imul::<crate::vm::Vm>, 2),
        ("log", oxide_builtins::math::math_log::<crate::vm::Vm>, 1),
        ("log10", oxide_builtins::math::math_log10::<crate::vm::Vm>, 1),
        ("log2", oxide_builtins::math::math_log2::<crate::vm::Vm>, 1),
        ("max", oxide_builtins::math::math_max::<crate::vm::Vm>, 2),
        ("min", oxide_builtins::math::math_min::<crate::vm::Vm>, 2),
        ("pow", oxide_builtins::math::math_pow::<crate::vm::Vm>, 2),
        ("random", oxide_builtins::math::math_random::<crate::vm::Vm>, 0),
        ("round", oxide_builtins::math::math_round::<crate::vm::Vm>, 1),
        ("sign", oxide_builtins::math::math_sign::<crate::vm::Vm>, 1),
        ("sin", oxide_builtins::math::math_sin::<crate::vm::Vm>, 1),
        ("sinh", oxide_builtins::math::math_sinh::<crate::vm::Vm>, 1),
        ("sqrt", oxide_builtins::math::math_sqrt::<crate::vm::Vm>, 1),
        ("tan", oxide_builtins::math::math_tan::<crate::vm::Vm>, 1),
        ("tanh", oxide_builtins::math::math_tanh::<crate::vm::Vm>, 1),
        ("trunc", oxide_builtins::math::math_trunc::<crate::vm::Vm>, 1),
        ("acosh", oxide_builtins::math::math_acosh::<crate::vm::Vm>, 1),
        ("asinh", oxide_builtins::math::math_asinh::<crate::vm::Vm>, 1),
        ("atanh", oxide_builtins::math::math_atanh::<crate::vm::Vm>, 1),
        ("clz32", oxide_builtins::math::math_clz32::<crate::vm::Vm>, 1),
        ("expm1", oxide_builtins::math::math_expm1::<crate::vm::Vm>, 1),
        ("fround", oxide_builtins::math::math_fround::<crate::vm::Vm>, 1),
        ("log1p", oxide_builtins::math::math_log1p::<crate::vm::Vm>, 1),
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
        let si = core.perm_interner().as_ref().intern(name).0;
        let sh_c = core.shape_forge().as_ref().make_shape(math.shape_id(), si);
        math.set_shape_id(sh_c);
        math.ensure_hash_props().push(JsValue::float(val));
    }

    let si_m = core.perm_interner().intern("Math").0;
    let m_shape = core.shape_forge().make_shape(global.shape_id(), si_m);
    let m_val = JsValue::from_js_object(session.builtin_world().math_object.as_ptr() as *mut JsObject);
    global.set_shape_id(m_shape);
    global.ensure_hash_props().push(m_val);
    global.bump_generation();
}
