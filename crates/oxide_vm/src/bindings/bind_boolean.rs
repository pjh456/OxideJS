use crate::bind_constructor;
use crate::bindings::{apply_binding_table, configure_native_constructor, BindEnv};
use oxide_types::object::JsObject;

/// 把 Boolean 构造器与原型方法绑定到 global。
pub fn bind_boolean(env: BindEnv<'_>, global: &mut JsObject) {
    let ctor_ptr = env.world().boolean_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = env.world().boolean_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::boolean::boolean_constructor::<crate::vm::Vm> as *const (), 1);

    apply_binding_table(
        env,
        proto,
        &[
            (
                "valueOf",
                oxide_builtins::boolean::boolean_prototype_value_of::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "toString",
                oxide_builtins::boolean::boolean_prototype_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            ("toSource", oxide_builtins::to_source::to_source_boolean::<crate::vm::Vm> as *const (), 0),
        ],
    );

    bind_constructor!(env, global, "Boolean", ctor_ptr, oxide_builtins::boolean::boolean_constructor::<crate::vm::Vm>, 1, hash: true);
}
