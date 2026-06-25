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
            ("apply", oxide_builtins::reflect::reflect_apply::<crate::vm::Vm> as *const (), 3),
            ("construct", oxide_builtins::reflect::reflect_construct::<crate::vm::Vm> as *const (), 2),
            (
                "defineProperty",
                oxide_builtins::reflect::reflect_define_property::<crate::vm::Vm> as *const (),
                3,
            ),
            (
                "deleteProperty",
                oxide_builtins::reflect::reflect_delete_property::<crate::vm::Vm> as *const (),
                2,
            ),
            ("get", oxide_builtins::reflect::reflect_get::<crate::vm::Vm> as *const (), 2),
            (
                "getOwnPropertyDescriptor",
                oxide_builtins::reflect::reflect_get_own_property_descriptor::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "getPrototypeOf",
                oxide_builtins::reflect::reflect_get_prototype_of::<crate::vm::Vm> as *const (),
                1,
            ),
            ("has", oxide_builtins::reflect::reflect_has::<crate::vm::Vm> as *const (), 2),
            (
                "isExtensible",
                oxide_builtins::reflect::reflect_is_extensible::<crate::vm::Vm> as *const (),
                1,
            ),
            ("ownKeys", oxide_builtins::reflect::reflect_own_keys::<crate::vm::Vm> as *const (), 1),
            (
                "preventExtensions",
                oxide_builtins::reflect::reflect_prevent_extensions::<crate::vm::Vm> as *const (),
                1,
            ),
            ("set", oxide_builtins::reflect::reflect_set::<crate::vm::Vm> as *const (), 3),
            (
                "setPrototypeOf",
                oxide_builtins::reflect::reflect_set_prototype_of::<crate::vm::Vm> as *const (),
                2,
            ),
        ],
    );

    bind_global_value(core, global, "Reflect", JsValue::from_js_object(Box::into_raw(reflect)));
}
