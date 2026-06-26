use std::sync::Arc;

use oxide_kernel::builtin::ObjectMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

use crate::bind_constructor;

pub fn bind_object(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let methods = ObjectMethods {
        keys: oxide_builtins::object::object_keys::<crate::vm::Vm> as *const (),
        create: oxide_builtins::object::object_create::<crate::vm::Vm> as *const (),
        assign: oxide_builtins::object::object_assign::<crate::vm::Vm> as *const (),
        is: oxide_builtins::object::object_is::<crate::vm::Vm> as *const (),
        define_property: oxide_builtins::object::object_define_property::<crate::vm::Vm> as *const (),
        get_own_property_descriptor: oxide_builtins::object::object_get_own_property_descriptor::<crate::vm::Vm>
            as *const (),
        freeze: oxide_builtins::object::object_freeze::<crate::vm::Vm> as *const (),
        seal: oxide_builtins::object::object_seal::<crate::vm::Vm> as *const (),
        prevent_extensions: oxide_builtins::object::object_prevent_extensions::<crate::vm::Vm> as *const (),
        is_frozen: oxide_builtins::object::object_is_frozen::<crate::vm::Vm> as *const (),
        is_sealed: oxide_builtins::object::object_is_sealed::<crate::vm::Vm> as *const (),
        is_extensible: oxide_builtins::object::object_is_extensible::<crate::vm::Vm> as *const (),
        get_own_property_names: oxide_builtins::object::object_get_own_property_names::<crate::vm::Vm> as *const (),
        define_properties: oxide_builtins::object::object_define_properties::<crate::vm::Vm> as *const (),
        from_entries: oxide_builtins::object::object_from_entries::<crate::vm::Vm> as *const (),
        get_prototype_of: oxide_builtins::object::object_get_prototype_of::<crate::vm::Vm> as *const (),
        has_own: oxide_builtins::object::object_has_own::<crate::vm::Vm> as *const (),
        entries: oxide_builtins::object::object_entries::<crate::vm::Vm> as *const (),
        values: oxide_builtins::object::object_values::<crate::vm::Vm> as *const (),
        has_own_property: oxide_builtins::object::object_proto_has_own_property::<crate::vm::Vm> as *const (),
        property_is_enumerable: oxide_builtins::object::object_proto_property_is_enumerable::<crate::vm::Vm>
            as *const (),
    };
    session
        .builtin_world()
        .bind_object_methods(&methods, core.perm_interner().as_ref(), core.shape_forge().as_ref());

    let ctor_ptr = session.builtin_world().object_constructor.as_ptr() as *mut JsObject;
    bind_constructor!(
        core,
        global,
        "Object",
        ctor_ptr,
        oxide_builtins::object::object_constructor::<crate::vm::Vm>,
        1
    );
}
