use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_number(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().number_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().number_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::number::number_constructor::<crate::vm::Vm> as *const (), 1);

    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[
            ("isNaN", oxide_builtins::number::number_is_nan::<crate::vm::Vm> as *const (), 1),
            ("isFinite", oxide_builtins::number::number_is_finite::<crate::vm::Vm> as *const (), 1),
            ("isInteger", oxide_builtins::number::number_is_integer::<crate::vm::Vm> as *const (), 1),
            (
                "isSafeInteger",
                oxide_builtins::number::number_is_safe_integer::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("toString", oxide_builtins::number::number_to_string::<crate::vm::Vm> as *const (), 0),
            ("toFixed", oxide_builtins::number::number_to_fixed::<crate::vm::Vm> as *const (), 0),
            (
                "toExponential",
                oxide_builtins::number::number_to_exponential::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toPrecision",
                oxide_builtins::number::number_to_precision::<crate::vm::Vm> as *const (),
                0,
            ),
            ("valueOf", oxide_builtins::number::number_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );

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
        ctor.ensure_hash_props().push(value);
        let prop_si = core.perm_interner().intern(name).0;
        let next_shape = core.shape_forge().make_shape(ctor.shape_id(), prop_si);
        ctor.set_shape_id(next_shape);
    }

    bind_constructor!(core, global, "Number", ctor_ptr, oxide_builtins::number::number_constructor::<crate::vm::Vm>, 1, hash: true);

    apply_binding_table(
        session.builtin_world(),
        global,
        core,
        &[
            ("parseInt", oxide_builtins::number::number_parse_int::<crate::vm::Vm> as *const (), 1),
            ("parseFloat", oxide_builtins::number::number_parse_float::<crate::vm::Vm> as *const (), 1),
        ],
    );
}
