use std::sync::Arc;

use crate::bindings::{apply_binding_table, bind_global_value};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_reflect(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let mut reflect = Box::new(JsObject::new_empty(
        EMPTY_SHAPE_ID,
        JsValue::from_js_object(session.builtin_world().object_proto.as_ptr() as *mut JsObject),
    ));

    apply_binding_table(
        session.builtin_world(),
        &mut reflect,
        core,
        &[
            ("apply", crate::builtins::reflect::reflect_apply as *const (), 3),
            ("construct", crate::builtins::reflect::reflect_construct as *const (), 2),
            ("defineProperty", crate::builtins::reflect::reflect_define_property as *const (), 3),
            ("deleteProperty", crate::builtins::reflect::reflect_delete_property as *const (), 2),
            ("get", crate::builtins::reflect::reflect_get as *const (), 2),
            (
                "getOwnPropertyDescriptor",
                crate::builtins::reflect::reflect_get_own_property_descriptor as *const (),
                2,
            ),
            ("getPrototypeOf", crate::builtins::reflect::reflect_get_prototype_of as *const (), 1),
            ("has", crate::builtins::reflect::reflect_has as *const (), 2),
            ("isExtensible", crate::builtins::reflect::reflect_is_extensible as *const (), 1),
            ("ownKeys", crate::builtins::reflect::reflect_own_keys as *const (), 1),
            ("preventExtensions", crate::builtins::reflect::reflect_prevent_extensions as *const (), 1),
            ("set", crate::builtins::reflect::reflect_set as *const (), 3),
            ("setPrototypeOf", crate::builtins::reflect::reflect_set_prototype_of as *const (), 2),
        ],
    );

    bind_global_value(core, global, "Reflect", JsValue::from_js_object(Box::into_raw(reflect)));
}
