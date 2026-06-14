use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

use crate::bindings::apply_binding_table;

pub fn bind_global(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    apply_binding_table(
        session.builtin_world(),
        global,
        core,
        &[
            ("escape", crate::builtins::global::js_escape as *const (), 1),
            ("unescape", crate::builtins::global::js_unescape as *const (), 1),
        ],
    );
}
