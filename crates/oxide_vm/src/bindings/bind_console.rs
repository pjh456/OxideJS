use std::sync::Arc;

use crate::bindings::bind_global_value;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// 把 Console 单例对象及其方法（log/warn/error/info/debug/trace）绑定到 global。
pub fn bind_console(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let console_ptr = session.builtin_world().console_object.as_ptr() as *mut JsObject;
    let console = unsafe { &mut *console_ptr };

    let bw = session.builtin_world();
    let sf = core.perm_interner().as_ref();
    let sh = core.shape_forge().as_ref();

    oxide_kernel::bind_methods!(
        bw,
        console,
        sf,
        sh,
        ("log", oxide_builtins::console::log::<crate::vm::Vm>, 1),
        ("warn", oxide_builtins::console::warn::<crate::vm::Vm>, 1),
        ("error", oxide_builtins::console::error::<crate::vm::Vm>, 1),
        ("info", oxide_builtins::console::info::<crate::vm::Vm>, 1),
        ("debug", oxide_builtins::console::debug::<crate::vm::Vm>, 1),
        ("trace", oxide_builtins::console::trace::<crate::vm::Vm>, 1),
    );

    bind_global_value(
        core,
        global,
        "console",
        JsValue::from_js_object(session.builtin_world().console_object.as_ptr() as *mut JsObject),
    );
}
