use std::sync::Arc;

use oxide_kernel::bind_methods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bindings::bind_global_value;

/// 把 Atomics 全局纯对象及其 10 个原子方法绑定到 global。
///
/// Atomics 非函数对象：原型链落 Object.prototype（`wire_builtin_world_links`），
/// `@@toStringTag` 由 `install_to_string_tags` 统一补装；此处只装方法与全局槽。
pub fn bind_atomics(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let atomics_ptr = session.builtin_world().atomics_object.as_ptr() as *mut JsObject;
    let atomics = unsafe { &mut *atomics_ptr };

    let bw = session.builtin_world();
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();

    bind_methods!(
        bw,
        atomics,
        sf,
        sh,
        ("load", oxide_builtins::atomics::atomics_load::<crate::vm::Vm>, 2),
        ("store", oxide_builtins::atomics::atomics_store::<crate::vm::Vm>, 3),
        ("exchange", oxide_builtins::atomics::atomics_exchange::<crate::vm::Vm>, 3),
        ("add", oxide_builtins::atomics::atomics_add::<crate::vm::Vm>, 3),
        ("sub", oxide_builtins::atomics::atomics_sub::<crate::vm::Vm>, 3),
        ("and", oxide_builtins::atomics::atomics_and::<crate::vm::Vm>, 3),
        ("or", oxide_builtins::atomics::atomics_or::<crate::vm::Vm>, 3),
        ("xor", oxide_builtins::atomics::atomics_xor::<crate::vm::Vm>, 3),
        ("compareExchange", oxide_builtins::atomics::atomics_compare_exchange::<crate::vm::Vm>, 4),
        ("isLockFree", oxide_builtins::atomics::atomics_is_lock_free::<crate::vm::Vm>, 1),
        ("wait", oxide_builtins::atomics::atomics_wait::<crate::vm::Vm>, 4),
        ("notify", oxide_builtins::atomics::atomics_notify::<crate::vm::Vm>, 3),
        ("waitAsync", oxide_builtins::atomics::atomics_wait_async::<crate::vm::Vm>, 4),
        ("pause", oxide_builtins::atomics::atomics_pause::<crate::vm::Vm>, 0),
    );

    let a_val = JsValue::from_js_object(atomics_ptr);
    bind_global_value(core, global, "Atomics", a_val);
}
