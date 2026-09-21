use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_well_known_method, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

use crate::bind_constructor;

/// 把 RegExp 构造器与原型方法绑定到 global。
pub fn bind_regexp(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().regexp_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().regexp_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::regexp::regexp_constructor::<crate::vm::Vm> as *const (), 2);

    // 静态方法 escape 装到构造器对象（wrapper 的 length/name 与槽属性由 bind_method 统一保证）。
    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[("escape", oxide_builtins::regexp::regexp_escape::<crate::vm::Vm> as *const (), 1)],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("exec", oxide_builtins::regexp::regexp_exec::<crate::vm::Vm> as *const (), 1),
            ("test", oxide_builtins::regexp::regexp_test::<crate::vm::Vm> as *const (), 1),
            ("toString", oxide_builtins::regexp::regexp_to_string::<crate::vm::Vm> as *const (), 0),
        ],
    );

    // Symbol.match 等 well-known symbol 方法按 Symbol 键安装，供 `re[Symbol.match]` 等读取。
    let world = session.builtin_world();
    bind_well_known_method(
        world,
        core,
        proto,
        1,
        "@@match",
        oxide_builtins::regexp::regexp_symbol_match::<crate::vm::Vm> as *const (),
        1,
    );
    bind_well_known_method(
        world,
        core,
        proto,
        2,
        "@@replace",
        oxide_builtins::regexp::regexp_symbol_replace::<crate::vm::Vm> as *const (),
        2,
    );
    bind_well_known_method(
        world,
        core,
        proto,
        3,
        "@@search",
        oxide_builtins::regexp::regexp_symbol_search::<crate::vm::Vm> as *const (),
        1,
    );
    bind_well_known_method(
        world,
        core,
        proto,
        4,
        "@@split",
        oxide_builtins::regexp::regexp_symbol_split::<crate::vm::Vm> as *const (),
        2,
    );
    bind_well_known_method(
        world,
        core,
        proto,
        7,
        "@@matchAll",
        oxide_builtins::regexp::regexp_symbol_match_all::<crate::vm::Vm> as *const (),
        1,
    );

    bind_constructor!(core, global, "RegExp", ctor_ptr, oxide_builtins::regexp::regexp_constructor::<crate::vm::Vm>, 2, hash: true);
}
