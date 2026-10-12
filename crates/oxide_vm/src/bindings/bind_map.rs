use crate::bind_constructor;
use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_accessor_getter_key, bind_well_known_method_alias,
    configure_native_constructor, BindEnv,
};
use oxide_types::object::JsObject;

/// 把 Map 构造器与原型方法绑定到 global。
///
/// `realm_id` 是目标所属 realm 的编号：well-known 符号键按 (realm 编号, 局部
/// 下标) 编码，realm 编号为 0 时与旧编码逐字节一致。
pub fn bind_map(env: BindEnv<'_>, global: &mut JsObject) {
    let ctor_ptr = env.world().map_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = env.world().map_proto.as_ptr() as *mut JsObject;

    configure_native_constructor(ctor, oxide_builtins::map::map_constructor::<crate::vm::Vm> as *const (), 1);
    let proto = unsafe { &mut *proto_ptr };

    // size 是访问器 getter，不进 apply_binding_table。
    apply_binding_table(
        env,
        proto,
        &[
            ("set", oxide_builtins::map::map_set::<crate::vm::Vm> as *const (), 2),
            ("get", oxide_builtins::map::map_get::<crate::vm::Vm> as *const (), 1),
            ("getOrInsert", oxide_builtins::map::map_get_or_insert::<crate::vm::Vm> as *const (), 2),
            (
                "getOrInsertComputed",
                oxide_builtins::map::map_get_or_insert_computed::<crate::vm::Vm> as *const (),
                2,
            ),
            ("has", oxide_builtins::map::map_has::<crate::vm::Vm> as *const (), 1),
            ("delete", oxide_builtins::map::map_delete::<crate::vm::Vm> as *const (), 1),
            ("clear", oxide_builtins::map::map_clear::<crate::vm::Vm> as *const (), 0),
            ("forEach", oxide_builtins::map::map_for_each::<crate::vm::Vm> as *const (), 1),
            ("entries", oxide_builtins::map::map_entries::<crate::vm::Vm> as *const (), 0),
            ("values", oxide_builtins::map::map_values::<crate::vm::Vm> as *const (), 0),
            ("keys", oxide_builtins::map::map_keys::<crate::vm::Vm> as *const (), 0),
        ],
    );

    // Map 的 @@iterator（Symbol.iterator）是 entries 的同一函数对象。
    bind_well_known_method_alias(env.core, proto, "entries", 0, env.realm_id);
    bind_accessor_getter(env, proto, "size", oxide_builtins::map::map_size::<crate::vm::Vm> as *const ());

    // Map[Symbol.species] 访问器：getter 返回 receiver，派生类沿静态原型链解析
    // @@species 得自身构造器（规范不给 class 默认 static @@species，类上无 own）。
    bind_accessor_getter_key(
        env,
        ctor,
        oxide_types::private_key::encode_symbol_key(env.realm_id, oxide_types::private_key::WELL_KNOWN_SYMBOL_SPECIES),
        "get [Symbol.species]",
        oxide_builtins::array::array_species_get::<crate::vm::Vm> as *const (),
    );

    // Map.groupBy 静态方法。
    apply_binding_table(
        env,
        ctor,
        &[("groupBy", oxide_builtins::map::map_group_by::<crate::vm::Vm> as *const (), 2)],
    );

    bind_constructor!(env, global, "Map", ctor_ptr, oxide_builtins::map::map_constructor::<crate::vm::Vm>, 0, hash: true);
}
