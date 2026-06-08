use std::sync::Arc;

use crate::bind_constructor;
use oxide_kernel::builtin::ObjectMethods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

pub fn bind_object(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let methods = ObjectMethods {
        keys: crate::builtins::object::object_keys as *const (),
        create: crate::builtins::object::object_create as *const (),
        assign: crate::builtins::object::object_assign as *const (),
        define_property: crate::builtins::object::object_define_property as *const (),
        get_own_property_descriptor: crate::builtins::object::object_get_own_property_descriptor
            as *const (),
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
    };
    kernel.builtin_world().bind_object_methods(
        &methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let ctor_ptr = kernel.builtin_world().object_constructor.as_ptr() as *mut JsObject;
    bind_constructor!(
        kernel,
        global,
        "Object",
        ctor_ptr,
        crate::builtins::object::object_constructor,
        1
    );
}
