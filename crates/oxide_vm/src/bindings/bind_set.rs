use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_method_alias, bind_well_known_method_alias,
    configure_native_constructor,
};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

/// 把 Set 构造器与原型方法绑定到 global。
pub fn bind_set(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().set_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().set_proto.as_ptr() as *mut JsObject;

    configure_native_constructor(ctor, oxide_builtins::set::set_constructor::<crate::vm::Vm> as *const (), 1);
    let proto = unsafe { &mut *proto_ptr };

    // size 是访问器 getter，不进 apply_binding_table。
    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("add", oxide_builtins::set::set_add::<crate::vm::Vm> as *const (), 1),
            ("has", oxide_builtins::set::set_has::<crate::vm::Vm> as *const (), 1),
            ("delete", oxide_builtins::set::set_delete::<crate::vm::Vm> as *const (), 1),
            ("clear", oxide_builtins::set::set_clear::<crate::vm::Vm> as *const (), 0),
            ("forEach", oxide_builtins::set::set_for_each::<crate::vm::Vm> as *const (), 1),
            ("entries", oxide_builtins::set::set_entries::<crate::vm::Vm> as *const (), 0),
            ("values", oxide_builtins::set::set_values::<crate::vm::Vm> as *const (), 0),
            ("union", oxide_builtins::set::set_union::<crate::vm::Vm> as *const (), 1),
            ("intersection", oxide_builtins::set::set_intersection::<crate::vm::Vm> as *const (), 1),
            ("difference", oxide_builtins::set::set_difference::<crate::vm::Vm> as *const (), 1),
            (
                "symmetricDifference",
                oxide_builtins::set::set_symmetric_difference::<crate::vm::Vm> as *const (),
                1,
            ),
            ("isSubsetOf", oxide_builtins::set::set_is_subset_of::<crate::vm::Vm> as *const (), 1),
            ("isSupersetOf", oxide_builtins::set::set_is_superset_of::<crate::vm::Vm> as *const (), 1),
            (
                "isDisjointFrom",
                oxide_builtins::set::set_is_disjoint_from::<crate::vm::Vm> as *const (),
                1,
            ),
        ],
    );

    // keys 与 @@iterator（Symbol.iterator）是 values 的同一函数对象。
    bind_method_alias(core, proto, "values", "keys");
    bind_well_known_method_alias(core, proto, "values", 0);
    bind_accessor_getter(core, session, proto, "size", oxide_builtins::set::set_size::<crate::vm::Vm> as *const ());

    bind_constructor!(core, global, "Set", ctor_ptr, oxide_builtins::set::set_constructor::<crate::vm::Vm>, 1, hash: true);
}
