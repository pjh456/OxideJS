use oxide_kernel::builtin::ObjectMethods;
use oxide_types::object::{JsObject, PropAttributes};

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, BindEnv};

/// 把 Object 构造器与原型方法绑定到 global。
pub fn bind_object(env: BindEnv<'_>, global: &mut JsObject) {
    let methods = ObjectMethods {
        keys: oxide_builtins::object::object_keys::<crate::vm::Vm> as *const (),
        create: oxide_builtins::object::object_create::<crate::vm::Vm> as *const (),
        assign: oxide_builtins::object::object_assign::<crate::vm::Vm> as *const (),
        is: oxide_builtins::object::object_is::<crate::vm::Vm> as *const (),
        define_property: oxide_builtins::object::object_define_property::<crate::vm::Vm> as *const (),
        get_own_property_descriptor: oxide_builtins::object::object_get_own_property_descriptor::<crate::vm::Vm>
            as *const (),
        get_own_property_descriptors: oxide_builtins::object::object_get_own_property_descriptors::<crate::vm::Vm>
            as *const (),
        freeze: oxide_builtins::object::object_freeze::<crate::vm::Vm> as *const (),
        seal: oxide_builtins::object::object_seal::<crate::vm::Vm> as *const (),
        prevent_extensions: oxide_builtins::object::object_prevent_extensions::<crate::vm::Vm> as *const (),
        is_frozen: oxide_builtins::object::object_is_frozen::<crate::vm::Vm> as *const (),
        is_sealed: oxide_builtins::object::object_is_sealed::<crate::vm::Vm> as *const (),
        is_extensible: oxide_builtins::object::object_is_extensible::<crate::vm::Vm> as *const (),
        get_own_property_names: oxide_builtins::object::object_get_own_property_names::<crate::vm::Vm> as *const (),
        get_own_property_symbols: oxide_builtins::object::object_get_own_property_symbols::<crate::vm::Vm> as *const (),
        define_properties: oxide_builtins::object::object_define_properties::<crate::vm::Vm> as *const (),
        from_entries: oxide_builtins::object::object_from_entries::<crate::vm::Vm> as *const (),
        get_prototype_of: oxide_builtins::object::object_get_prototype_of::<crate::vm::Vm> as *const (),
        set_prototype_of: oxide_builtins::object::object_set_prototype_of::<crate::vm::Vm> as *const (),
        has_own: oxide_builtins::object::object_has_own::<crate::vm::Vm> as *const (),
        entries: oxide_builtins::object::object_entries::<crate::vm::Vm> as *const (),
        values: oxide_builtins::object::object_values::<crate::vm::Vm> as *const (),
        has_own_property: oxide_builtins::object::object_proto_has_own_property::<crate::vm::Vm> as *const (),
        property_is_enumerable: oxide_builtins::object::object_proto_property_is_enumerable::<crate::vm::Vm>
            as *const (),
        group_by: oxide_builtins::object::object_group_by::<crate::vm::Vm> as *const (),
    };
    env.world().bind_object_methods(&methods, env.string_forge(), env.shape_forge());

    let object_proto_ptr = env.world().object_proto.as_ptr() as *mut JsObject;
    let object_proto = unsafe { &mut *object_proto_ptr };
    apply_binding_table(
        env,
        object_proto,
        &[
            (
                "toString",
                oxide_builtins::object::object_proto_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            ("valueOf", oxide_builtins::object::object_proto_value_of::<crate::vm::Vm> as *const (), 0),
            (
                "isPrototypeOf",
                oxide_builtins::object::object_proto_is_prototype_of::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "toLocaleString",
                oxide_builtins::object::object_proto_to_locale_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "__defineGetter__",
                oxide_builtins::object::object_proto_define_getter::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "__defineSetter__",
                oxide_builtins::object::object_proto_define_setter::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "__lookupGetter__",
                oxide_builtins::object::object_proto_lookup_getter::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "__lookupSetter__",
                oxide_builtins::object::object_proto_lookup_setter::<crate::vm::Vm> as *const (),
                1,
            ),
            ("toSource", oxide_builtins::to_source::to_source_object::<crate::vm::Vm> as *const (), 0),
        ],
    );
    // 内置原型方法不可枚举（否则会泄漏进 for-in）。
    for name in [
        "toString",
        "valueOf",
        "isPrototypeOf",
        "toLocaleString",
        "__defineGetter__",
        "__defineSetter__",
        "__lookupGetter__",
        "__lookupSetter__",
        "toSource",
    ] {
        let si = env.string_forge().intern(name).0;
        if let Some(pos) = env.shape_forge().lookup_position(object_proto.shape_id(), si) {
            object_proto.set_data_meta(pos, PropAttributes::new(true, false, true));
        }
    }

    let ctor_ptr = env.world().object_constructor.as_ptr() as *mut JsObject;
    bind_constructor!(
        env,
        global,
        "Object",
        ctor_ptr,
        oxide_builtins::object::object_constructor::<crate::vm::Vm>,
        1
    );
}
