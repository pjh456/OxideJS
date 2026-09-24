use std::sync::Arc;

use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_well_known_method, configure_native_constructor,
};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::private_key::{make_well_known_symbol_key, WELL_KNOWN_SYMBOL_TO_PRIMITIVE};
use oxide_types::value::JsValue;

use crate::bind_constructor;

/// 把 Symbol 构造器与原型方法绑定到 global（含 `Symbol.iterator` 等 well-known symbols）。
pub fn bind_symbol(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().symbol_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().symbol_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(ctor, oxide_builtins::symbol::symbol_constructor::<crate::vm::Vm> as *const (), 1);

    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[
            ("for", oxide_builtins::symbol::symbol_for::<crate::vm::Vm> as *const (), 1),
            ("keyFor", oxide_builtins::symbol::symbol_key_for::<crate::vm::Vm> as *const (), 1),
        ],
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            ("toString", oxide_builtins::symbol::symbol_to_string::<crate::vm::Vm> as *const (), 0),
            ("valueOf", oxide_builtins::symbol::symbol_value_of::<crate::vm::Vm> as *const (), 0),
        ],
    );

    // description 是访问器 getter（Symbol 原始值成员访问经 Symbol.prototype 链命中）。
    bind_accessor_getter(
        core,
        session,
        proto,
        "description",
        oxide_builtins::symbol::symbol_description_getter::<crate::vm::Vm> as *const (),
    );

    // [Symbol.toPrimitive] 方法：bind 默认描述符可写，规范为不可写，绑定后改 meta。
    bind_well_known_method(
        session.builtin_world(),
        core,
        proto,
        WELL_KNOWN_SYMBOL_TO_PRIMITIVE,
        "[Symbol.toPrimitive]",
        oxide_builtins::symbol::symbol_to_primitive::<crate::vm::Vm> as *const (),
        1,
    );
    let to_prim_key = make_well_known_symbol_key(WELL_KNOWN_SYMBOL_TO_PRIMITIVE);
    if let Some(pos) = core.shape_forge().lookup_position(proto.shape_id(), to_prim_key) {
        proto.set_data_meta(pos, PropAttributes::new(false, false, true));
        proto.bump_generation();
    }

    // well-known symbol 以符号原语绑定，下标与内建符号表（0..WELL_KNOWN_SYMBOL_COUNT）
    // 一一对应；名称表是 id/名映射的唯一来源，属性名去掉 `Symbol.` 前缀。
    for (id, full_name) in oxide_types::private_key::WELL_KNOWN_SYMBOL_NAMES.iter().enumerate() {
        let name = full_name.strip_prefix("Symbol.").unwrap_or(full_name);
        bind_well_known_symbol(core, ctor, name, JsValue::symbol(id as u32));
    }

    bind_constructor!(core, global, "Symbol", ctor_ptr, oxide_builtins::symbol::symbol_constructor::<crate::vm::Vm>, 0, hash: true);
}

fn bind_well_known_symbol(core: &Arc<KernelCore>, ctor: &mut JsObject, name: &str, val: JsValue) {
    let si = core.perm_interner().intern(name).0;
    let shape_id = core.shape_forge().make_shape(ctor.shape_id(), si);
    ctor.set_shape_id(shape_id);
    ctor.ensure_hash_props().push(val);
    // well-known symbol 属性按规范 { writable:false, enumerable:false, configurable:false }。
    let pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(pos, PropAttributes::new(false, false, false));
    ctor.bump_generation();
}
