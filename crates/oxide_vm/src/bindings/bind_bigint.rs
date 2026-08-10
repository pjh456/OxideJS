use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_global_value, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
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

    apply_binding_table(
        world,
        proto,
        core,
        &[
            ("toString", oxide_builtins::bigint::bigint_to_string::<crate::vm::Vm> as *const (), 0),
            ("valueOf", oxide_builtins::bigint::bigint_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );

    bind_global_value(core, global, "BigInt", JsValue::from_js_object(ctor_ptr));
}
