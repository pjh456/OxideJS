use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_global_value, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

/// 把 BigInt 构造器与原型方法绑定到 global。
///
/// 构造器/原型对象由 `BuiltinWorld` 创建（含 name/prototype/constructor 互指），
/// 这里只配置 native 实现并挂方法。`BigInt(value)` 当函数调用时返回 BigInt 值；
/// `new BigInt()` 抛 TypeError（BigInt 不可 new）。
pub fn bind_bigint(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let world = session.builtin_world();
    let ctor_ptr = world.bigint_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = world.bigint_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::bigint::bigint_constructor::<crate::vm::Vm> as *const (), 1);
    // BigInt.length = 1，属性 { [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: true }。
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(ctor.shape_id(), length_si);
    ctor.set_shape_id(length_shape);
    ctor.ensure_hash_props().push(JsValue::int(1));
    let length_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    // 静态方法：asIntN / asUintN（length 均为 2）。
    apply_binding_table(
        world,
        ctor,
        core,
        &[
            ("asIntN", oxide_builtins::bigint::bigint_as_int_n::<crate::vm::Vm> as *const (), 2),
            ("asUintN", oxide_builtins::bigint::bigint_as_uint_n::<crate::vm::Vm> as *const (), 2),
        ],
    );

    apply_binding_table(
        world,
        proto,
        core,
        &[
            ("toString", oxide_builtins::bigint::bigint_to_string::<crate::vm::Vm> as *const (), 0),
            ("toLocaleString", oxide_builtins::bigint::bigint_to_locale_string::<crate::vm::Vm> as *const (), 0),
            ("valueOf", oxide_builtins::bigint::bigint_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );

    bind_global_value(core, global, "BigInt", JsValue::from_js_object(ctor_ptr));
    // 全局 BigInt 属性描述符：{ writable: true, enumerable: false, configurable: true }。
    let bigint_si = core.perm_interner().intern("BigInt").0;
    if let Some(pos) = core.shape_forge().lookup_position(global.shape_id(), bigint_si) {
        global.set_data_meta(pos, PropAttributes::new(true, false, true));
    }
}
