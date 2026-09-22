use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{
    apply_binding_table, bind_accessor_getter, bind_accessor_getter_key, configure_native_constructor,
};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};

/// 把 ArrayBuffer 构造器与原型方法绑定到 global。
pub fn bind_array_buffer(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().array_buffer_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().array_buffer_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(
        ctor,
        oxide_builtins::array_buffer::array_buffer_constructor::<crate::vm::Vm> as *const (),
        1,
    );

    apply_binding_table(
        session.builtin_world(),
        ctor,
        core,
        &[(
            "isView",
            oxide_builtins::array_buffer::array_buffer_is_view::<crate::vm::Vm> as *const (),
            1,
        )],
    );

    // ArrayBuffer[Symbol.species] 访问器：getter 返回 receiver，派生类沿静态原型链
    // 解析 @@species 得自身构造器（规范不给 class 默认 static @@species，类上无 own）。
    bind_accessor_getter_key(
        core,
        session,
        ctor,
        oxide_types::private_key::make_well_known_symbol_key(oxide_types::private_key::WELL_KNOWN_SYMBOL_SPECIES),
        "get [Symbol.species]",
        oxide_builtins::array::array_species_get::<crate::vm::Vm> as *const (),
    );

    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            (
                "resize",
                oxide_builtins::array_buffer::array_buffer_resize::<crate::vm::Vm> as *const (),
                1,
            ),
            ("slice", oxide_builtins::array_buffer::array_buffer_slice::<crate::vm::Vm> as *const (), 2),
            (
                "toString",
                oxide_builtins::array_buffer::array_buffer_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "markImmutable",
                oxide_builtins::array_buffer::array_buffer_mark_immutable::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "transfer",
                oxide_builtins::array_buffer::array_buffer_transfer::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "transferToFixedLength",
                oxide_builtins::array_buffer::array_buffer_transfer_to_fixed_length::<crate::vm::Vm> as *const (),
                0,
            ),
            (
                "transferToImmutable",
                oxide_builtins::array_buffer::array_buffer_transfer_to_immutable::<crate::vm::Vm> as *const (),
                0,
            ),
        ],
    );

    // byteLength 原型访问器（set 恒 undefined）：读载荷字节数，实例无 own 属性。
    bind_accessor_getter(
        core,
        session,
        proto,
        "byteLength",
        oxide_builtins::array_buffer::array_buffer_byte_length::<crate::vm::Vm> as *const (),
    );

    // resizable 原型访问器（set 恒 undefined）：读载荷 max_byte_length 状态位。
    bind_accessor_getter(
        core,
        session,
        proto,
        "resizable",
        oxide_builtins::array_buffer::array_buffer_resizable::<crate::vm::Vm> as *const (),
    );

    // maxByteLength 原型访问器（set 恒 undefined）：detached 回 +0，定长回
    // 当前字节数，resizable 回存储态上限 − 1。
    bind_accessor_getter(
        core,
        session,
        proto,
        "maxByteLength",
        oxide_builtins::array_buffer::array_buffer_max_byte_length::<crate::vm::Vm> as *const (),
    );

    // immutable 原型访问器（set 恒 undefined）：读载荷 immutable 标志。
    bind_accessor_getter(
        core,
        session,
        proto,
        "immutable",
        oxide_builtins::array_buffer::array_buffer_immutable::<crate::vm::Vm> as *const (),
    );

    // detached 原型访问器（set 恒 undefined）：读载荷 data 存活位。
    bind_accessor_getter(
        core,
        session,
        proto,
        "detached",
        oxide_builtins::array_buffer::array_buffer_detached::<crate::vm::Vm> as *const (),
    );

    bind_constructor!(
        core,
        global,
        "ArrayBuffer",
        ctor_ptr,
        oxide_builtins::array_buffer::array_buffer_constructor::<crate::vm::Vm>,
        1,
        hash: true
    );

    // 宏推入的 length 槽默认可写可枚举，补钉规范描述符
    // { [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: true }。
    let length_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));
}
