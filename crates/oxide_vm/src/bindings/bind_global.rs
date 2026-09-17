use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

use crate::bindings::apply_binding_table;

/// 把普通全局函数（`isNaN`、`parseInt`、`decodeURIComponent` 等）绑定到 global。
pub fn bind_global(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    apply_binding_table(
        session.builtin_world(),
        global,
        core,
        &[
            ("escape", oxide_builtins::global::js_escape::<crate::vm::Vm> as *const (), 1),
            ("unescape", oxide_builtins::global::js_unescape::<crate::vm::Vm> as *const (), 1),
            ("encodeURI", oxide_builtins::global::encode_uri::<crate::vm::Vm> as *const (), 1),
            ("decodeURI", oxide_builtins::global::decode_uri::<crate::vm::Vm> as *const (), 1),
            (
                "encodeURIComponent",
                oxide_builtins::global::encode_uri_component::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "decodeURIComponent",
                oxide_builtins::global::decode_uri_component::<crate::vm::Vm> as *const (),
                1,
            ),
            // 模块求值 / 命名空间内部辅助（import 实现；非 JS 标准全局，以下划线开头避免撞名）。
            ("__moduleObject", oxide_builtins::module::module_object::<crate::vm::Vm> as *const (), 0),
            (
                "__modulePreRegister",
                oxide_builtins::module::module_pre_register::<crate::vm::Vm> as *const (),
                2,
            ),
            ("__moduleSet", oxide_builtins::module::module_set::<crate::vm::Vm> as *const (), 3),
            ("__moduleGet", oxide_builtins::module::module_get::<crate::vm::Vm> as *const (), 2),
            (
                "__moduleLinkGet",
                oxide_builtins::module::module_link_get::<crate::vm::Vm> as *const (),
                2,
            ),
            ("__moduleStar", oxide_builtins::module::module_star::<crate::vm::Vm> as *const (), 2),
            ("__moduleSeal", oxide_builtins::module::module_seal::<crate::vm::Vm> as *const (), 1),
            ("__moduleEval", oxide_builtins::module::module_eval::<crate::vm::Vm> as *const (), 1),
            ("__moduleData", oxide_builtins::module::module_data::<crate::vm::Vm> as *const (), 2),
            ("eval", oxide_builtins::eval::eval::<crate::vm::Vm> as *const (), 1),
        ],
    );
}
