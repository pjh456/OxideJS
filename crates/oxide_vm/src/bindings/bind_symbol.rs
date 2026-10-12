use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_well_known_method, configure_native_constructor, BindEnv,
};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{encode_symbol_key, WELL_KNOWN_SYMBOL_TO_PRIMITIVE};
use oxide_types::value::JsValue;

use crate::bind_constructor;

/// 把 Symbol 构造器与原型方法绑定到 global（含 `Symbol.iterator` 等 well-known symbols）。
///
/// `realm_id` 是目标所属 realm 的编号：well-known 符号键按 (realm 编号, 局部
/// 下标) 编码，realm 编号为 0 时与旧编码逐字节一致。
pub fn bind_symbol(env: BindEnv<'_>, global: &mut JsObject) {
    let ctor_ptr = env.world().symbol_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = env.world().symbol_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::symbol::symbol_constructor::<crate::vm::Vm> as *const (), 1);

    apply_binding_table(
        env,
        ctor,
        &[
            ("for", oxide_builtins::symbol::symbol_for::<crate::vm::Vm> as *const (), 1),
            ("keyFor", oxide_builtins::symbol::symbol_key_for::<crate::vm::Vm> as *const (), 1),
        ],
    );

    apply_binding_table(
        env,
        proto,
        &[
            ("toString", oxide_builtins::symbol::symbol_to_string::<crate::vm::Vm> as *const (), 0),
            ("valueOf", oxide_builtins::symbol::symbol_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );

    // description 是访问器 getter（Symbol 原始值成员访问经 Symbol.prototype 链命中）。
    bind_accessor_getter(
        env,
        proto,
        "description",
        oxide_builtins::symbol::symbol_description_getter::<crate::vm::Vm> as *const (),
    );

    // [Symbol.toPrimitive] 方法：bind 默认描述符可写，规范为不可写，绑定后改 meta。
    bind_well_known_method(
        env,
        proto,
        WELL_KNOWN_SYMBOL_TO_PRIMITIVE,
        "[Symbol.toPrimitive]",
        oxide_builtins::symbol::symbol_to_primitive::<crate::vm::Vm> as *const (),
        1,
    );
    let to_prim_key = encode_symbol_key(env.realm_id, WELL_KNOWN_SYMBOL_TO_PRIMITIVE);
    if let Some(pos) = env.shape_forge().lookup_position(proto.shape_id(), to_prim_key) {
        proto.set_data_meta(pos, PropAttributes::new(false, false, true));
        proto.bump_generation();
    }

    // well-known symbol 以符号原语绑定，下标与内建符号表（0..WELL_KNOWN_SYMBOL_COUNT）
    // 一一对应；名称表是 id/名映射的唯一来源，属性名去掉 `Symbol.` 前缀。
    // 原语按 (realm 编号, 局部下标) 编码，与键空间 `encode_symbol_key` 同口径，
    // 非零 realm 下与键物化值保持同一符号。
    for (id, full_name) in oxide_types::private_key::WELL_KNOWN_SYMBOL_NAMES.iter().enumerate() {
        let name = full_name.strip_prefix("Symbol.").unwrap_or(full_name);
        bind_well_known_symbol(env, ctor, name, JsValue::symbol_realm(env.realm_id, id as u32));
    }

    bind_constructor!(env, global, "Symbol", ctor_ptr, oxide_builtins::symbol::symbol_constructor::<crate::vm::Vm>, 0, hash: true);
}

fn bind_well_known_symbol(env: BindEnv<'_>, ctor: &mut JsObject, name: &str, val: JsValue) {
    let si = env.string_forge().intern(name).0;
    let shape_id = env.shape_forge().make_shape(ctor.shape_id(), si);
    ctor.set_shape_id(shape_id);
    ctor.ensure_hash_props().push(val);
    // well-known symbol 属性按规范 { writable:false, enumerable:false, configurable:false }。
    let pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(pos, PropAttributes::new(false, false, false));
    ctor.bump_generation();
}
