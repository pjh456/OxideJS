use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 Number 构造器与原型方法绑定到 global（含 NaN/POSITIVE_INFINITY 等常量）。
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
            ("parseInt", oxide_builtins::number::number_parse_int::<crate::vm::Vm> as *const (), 1),
            ("parseFloat", oxide_builtins::number::number_parse_float::<crate::vm::Vm> as *const (), 1),
        ],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("toString", oxide_builtins::number::number_to_string::<crate::vm::Vm> as *const (), 1),
            ("toFixed", oxide_builtins::number::number_to_fixed::<crate::vm::Vm> as *const (), 1),
            (
                "toExponential",
                oxide_builtins::number::number_to_exponential::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "toPrecision",
                oxide_builtins::number::number_to_precision::<crate::vm::Vm> as *const (),
                1,
            ),
            // toLocaleString 为 Number.prototype 的 own 属性（规范 21.7.3.25）：
            // 保留参数不计 length（length 0），locale 参数忽略、恒十进制。
            (
                "toLocaleString",
                oxide_builtins::number::number_to_locale_string::<crate::vm::Vm> as *const (),
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
        let pos = ctor.ensure_hash_props().len() - 1;
        let prop_si = core.perm_interner().intern(name).0;
        let next_shape = core.shape_forge().make_shape(ctor.shape_id(), prop_si);
        ctor.set_shape_id(next_shape);
        ctor.set_data_meta(pos, PropAttributes::new(false, false, false));
    }

    bind_constructor!(core, global, "Number", ctor_ptr, oxide_builtins::number::number_constructor::<crate::vm::Vm>, 1, hash: true);

    apply_binding_table(
        session.builtin_world(),
        global,
        core,
        &[
            ("parseInt", oxide_builtins::number::number_parse_int::<crate::vm::Vm> as *const (), 1),
            ("parseFloat", oxide_builtins::number::number_parse_float::<crate::vm::Vm> as *const (), 1),
            ("isNaN", oxide_builtins::global::global_is_nan::<crate::vm::Vm> as *const (), 1),
            ("isFinite", oxide_builtins::global::global_is_finite::<crate::vm::Vm> as *const (), 1),
        ],
    );

    // 规范（21.7.3 / 20.1.3 注）：Number.parseInt/parseFloat 与全局同名函数
    // 是同一函数对象（SameValue）。ctor 侧绑定先执行、产生独立 wrapper，
    // 此处把 global 侧 wrapper 值原位写回 ctor 同名槽，描述符形态不变。
    for name in ["parseInt", "parseFloat"] {
        let si = core.perm_interner().intern(name).0;
        if let Some(global_pos) = core.shape_forge().lookup_position(global.shape_id(), si) {
            let val = global.get_prop_at(global_pos);
            if let Some(ctor_pos) = core.shape_forge().lookup_position(ctor.shape_id(), si) {
                ctor.set_prop_at(ctor_pos, val);
            }
        }
    }
}
