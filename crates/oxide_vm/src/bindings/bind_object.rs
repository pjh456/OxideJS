use std::sync::Arc;

use oxide_kernel::builtin::ObjectMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

use crate::bind_constructor;

pub fn bind_object(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let methods = ObjectMethods {
        keys: crate::builtins::object::object_keys as *const (),
        create: crate::builtins::object::object_create as *const (),
        assign: crate::builtins::object::object_assign as *const (),
        is: crate::builtins::object::object_is as *const (),
        define_property: crate::builtins::object::object_define_property as *const (),
        get_own_property_descriptor: crate::builtins::object::object_get_own_property_descriptor as *const (),
        freeze: crate::builtins::object::object_freeze as *const (),
        seal: crate::builtins::object::object_seal as *const (),
        prevent_extensions: crate::builtins::object::object_prevent_extensions as *const (),
        is_frozen: crate::builtins::object::object_is_frozen as *const (),
        is_sealed: crate::builtins::object::object_is_sealed as *const (),
        is_extensible: crate::builtins::object::object_is_extensible as *const (),
        get_own_property_names: crate::builtins::object::object_get_own_property_names as *const (),
        define_properties: crate::builtins::object::object_define_properties as *const (),
        from_entries: crate::builtins::object::object_from_entries as *const (),
        get_prototype_of: crate::builtins::object::object_get_prototype_of as *const (),
        has_own: crate::builtins::object::object_has_own as *const (),
        entries: crate::builtins::object::object_entries as *const (),
        values: crate::builtins::object::object_values as *const (),
        has_own_property: crate::builtins::object::object_proto_has_own_property as *const (),
        property_is_enumerable: crate::builtins::object::object_proto_property_is_enumerable as *const (),
    };
    session
        .builtin_world()
        .bind_object_methods(&methods, core.perm_interner().as_ref(), core.shape_forge().as_ref());

    let ctor_ptr = session.builtin_world().object_constructor.as_ptr() as *mut JsObject;
    bind_constructor!(core, global, "Object", ctor_ptr, crate::builtins::object::object_constructor, 1);
}
