use crate::bind_constructor;
use crate::bindings::{apply_binding_table, bind_accessor_getter, configure_native_constructor, BindEnv};
use oxide_types::object::{JsObject, PropAttributes};

/// 把 DataView 构造器与原型方法绑定到 global。
pub fn bind_data_view(env: BindEnv<'_>, global: &mut JsObject) {
    let ctor_ptr = env.world().data_view_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = env.world().data_view_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(
        ctor,
        oxide_builtins::data_view::data_view_constructor::<crate::vm::Vm> as *const (),
        1,
    );

    apply_binding_table(
        env,
        proto,
        &[
            ("getInt8", oxide_builtins::data_view::data_view_get_int8::<crate::vm::Vm> as *const (), 1),
            (
                "getUint8",
                oxide_builtins::data_view::data_view_get_uint8::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getInt16",
                oxide_builtins::data_view::data_view_get_int16::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getUint16",
                oxide_builtins::data_view::data_view_get_uint16::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getInt32",
                oxide_builtins::data_view::data_view_get_int32::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getUint32",
                oxide_builtins::data_view::data_view_get_uint32::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getFloat32",
                oxide_builtins::data_view::data_view_get_float32::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getFloat16",
                oxide_builtins::data_view::data_view_get_float16::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getFloat64",
                oxide_builtins::data_view::data_view_get_float64::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getBigInt64",
                oxide_builtins::data_view::data_view_get_big_int64::<crate::vm::Vm> as *const (),
                1,
            ),
            (
                "getBigUint64",
                oxide_builtins::data_view::data_view_get_big_uint64::<crate::vm::Vm> as *const (),
                1,
            ),
            ("setInt8", oxide_builtins::data_view::data_view_set_int8::<crate::vm::Vm> as *const (), 2),
            (
                "setUint8",
                oxide_builtins::data_view::data_view_set_uint8::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setInt16",
                oxide_builtins::data_view::data_view_set_int16::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setUint16",
                oxide_builtins::data_view::data_view_set_uint16::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setInt32",
                oxide_builtins::data_view::data_view_set_int32::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setUint32",
                oxide_builtins::data_view::data_view_set_uint32::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setFloat32",
                oxide_builtins::data_view::data_view_set_float32::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setFloat16",
                oxide_builtins::data_view::data_view_set_float16::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setFloat64",
                oxide_builtins::data_view::data_view_set_float64::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setBigInt64",
                oxide_builtins::data_view::data_view_set_big_int64::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "setBigUint64",
                oxide_builtins::data_view::data_view_set_big_uint64::<crate::vm::Vm> as *const (),
                2,
            ),
            (
                "toString",
                oxide_builtins::data_view::data_view_to_string::<crate::vm::Vm> as *const (),
                0,
            ),
        ],
    );

    bind_constructor!(env, global, "DataView", ctor_ptr, oxide_builtins::data_view::data_view_constructor::<crate::vm::Vm>, 1, hash: true);
    // length 属性描述符补 { writable:false, enumerable:false, configurable:true }
    // （bind_constructor! 宏的通用缺省 meta 不可写回，此处理仅覆盖 DataView）。
    {
        let length_si = env.string_forge().intern("length").0;
        if let Some(pos) = env.shape_forge().lookup_position(ctor.shape_id(), length_si) {
            ctor.set_data_meta(pos, PropAttributes::new(false, false, true));
        }
    }

    // buffer/byteOffset/byteLength 只读访问器（set 恒 undefined，getter 读视图状态盒）。
    bind_accessor_getter(
        env,
        proto,
        "buffer",
        oxide_builtins::data_view::data_view_buffer_getter::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        env,
        proto,
        "byteOffset",
        oxide_builtins::data_view::data_view_byte_offset_getter::<crate::vm::Vm> as *const (),
    );
    bind_accessor_getter(
        env,
        proto,
        "byteLength",
        oxide_builtins::data_view::data_view_byte_length_getter::<crate::vm::Vm> as *const (),
    );
}
