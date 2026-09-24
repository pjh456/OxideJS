use std::sync::Arc;

use crate::bind_constructor;
use crate::bindings::{apply_binding_table, bind_accessor_getter, configure_native_constructor};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

/// 把 SharedArrayBuffer 构造器与原型访问器绑定到 global。
pub fn bind_shared_array_buffer(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let ctor_ptr = session.builtin_world().shared_array_buffer_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = session.builtin_world().shared_array_buffer_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(
        ctor,
        oxide_builtins::array_buffer::shared_array_buffer_constructor::<crate::vm::Vm> as *const (),
        1,
    );

    // grow/slice 方法：grow 原地增长（只增不缩）；slice 复制字节区间生成
    // 新 SAB。
    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[
            (
                "grow",
                oxide_builtins::array_buffer::shared_array_buffer_grow::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "slice",
                oxide_builtins::array_buffer::shared_array_buffer_slice::<crate::vm::Vm> as *const (),
                2,
            ),
        ],
    );

    // byteLength/maxByteLength 两枚访问器（set 恒 undefined）读载荷字节数 /
    // 存储态上限，品牌校验抛 TypeError（proto 自身访问即抛）。
    bind_accessor_getter(
        core,
        session,
        proto,
        "byteLength",
        oxide_builtins::array_buffer::shared_array_buffer_byte_length::<crate::vm::Vm> as *const (),
    );

    bind_accessor_getter(
        core,
        session,
        proto,
        "maxByteLength",
        oxide_builtins::array_buffer::shared_array_buffer_max_byte_length::<crate::vm::Vm> as *const (),
    );

    // growable 真值臂：读载荷存储态上限（0 定长 false / 非 0 growable true）。
    bind_accessor_getter(
        core,
        session,
        proto,
        "growable",
        oxide_builtins::array_buffer::shared_array_buffer_growable::<crate::vm::Vm> as *const (),
    );

    bind_constructor!(
        core,
        global,
        "SharedArrayBuffer",
        ctor_ptr,
        oxide_builtins::array_buffer::shared_array_buffer_constructor::<crate::vm::Vm>,
        1,
        hash: true
    );
}
