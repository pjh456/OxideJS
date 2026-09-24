use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::P;
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use crate::bindings::{bind_global_value, configure_native_constructor};

/// 未实现内置的 stub 表：（全局名、native 函数项指针、形参个数、
/// 原型 @@toStringTag、是否安装原型对象）。
///
/// `tag` 为 `None` 的条目（Proxy）原型对象不设 @@toStringTag；四弱族各取族名。
/// 规范 25.1 的 %Proxy% 无 prototype 属性（`has_prototype` = false），
/// 四弱族各装带 constructor 回指的原型对象。
const STUBS: [(&str, *const (), u8, Option<&str>, bool); 5] = [
    ("Proxy", oxide_builtins::stubs::proxy_stub::<crate::vm::Vm> as *const (), 2, None, false),
    (
        "WeakMap",
        oxide_builtins::stubs::weakmap_stub::<crate::vm::Vm> as *const (),
        0,
        Some("WeakMap"),
        true,
    ),
    (
        "WeakSet",
        oxide_builtins::stubs::weakset_stub::<crate::vm::Vm> as *const (),
        0,
        Some("WeakSet"),
        true,
    ),
    (
        "WeakRef",
        oxide_builtins::stubs::weakref_stub::<crate::vm::Vm> as *const (),
        1,
        Some("WeakRef"),
        true,
    ),
    (
        "FinalizationRegistry",
        oxide_builtins::stubs::finalization_registry_stub::<crate::vm::Vm> as *const (),
        1,
        Some("FinalizationRegistry"),
        true,
    ),
];

/// 在 `obj` 上按键 `si` 安装数据属性，描述符为 `attributes`：
/// 开 shape 槽，值写入命名属性区并置描述符元数据。
fn set_data_property(core: &Arc<KernelCore>, obj: &mut JsObject, si: u32, value: JsValue, attributes: PropAttributes) {
    let shape = core.shape_forge().make_shape(obj.shape_id(), si);
    obj.set_shape_id(shape);
    let pos = obj.push_prop(value);
    obj.set_data_meta(pos, attributes);
    obj.bump_generation();
}

/// 把未实现内置（Proxy/WeakMap/WeakSet/WeakRef/FinalizationRegistry）的 stub 构造器
/// 绑定到 global，并登记到 `stub_objects` 供快照跟踪。
///
/// # 副作用
/// 每个 stub 按规范补装构造器面属性：`length`、`name`（五族全装）；四弱族
/// 另装 `prototype`（新建 plain 对象，原型链落 Object.prototype，其上带指回
/// stub 的 `constructor` 与 @@toStringTag）——规范 25.1 的 %Proxy% 无
/// prototype 属性，不装；描述符均不可枚举，`prototype` 另不可写、不可配置。
/// 原型对象登记进 world 释放登记表，与 stub 对象同生死，session 收尾统一释放。
pub fn bind_stubs(core: &Arc<KernelCore>, session: &mut KernelSession, global: &mut JsObject) {
    let builtin_world = Arc::get_mut(&mut session.builtin_world)
        .expect("BuiltinWorld must be uniquely owned during init_kernel_builtins");
    let object_proto_ptr = builtin_world.object_proto.as_ptr() as *mut JsObject;

    for (name, native_fn, arg_count, tag, has_prototype) in STUBS {
        let mut stub = JsObject::new_empty(
            EMPTY_SHAPE_ID,
            JsValue::from_js_object(builtin_world.function_proto.as_ptr() as *mut JsObject),
        );
        stub.set_function(true);
        configure_native_constructor(&mut stub, native_fn, arg_count);

        let stub = P::new(stub);
        let stub_ptr = stub.as_ptr() as *mut JsObject;

        // 构造器面属性：length / name 五族全装；prototype 仅四弱族（%Proxy% 无）。
        // SAFETY: stub 尚未发布到 global，局部 Arc 为唯一持有者，可变访问无读者冲突。
        let stub_obj = unsafe { &mut *stub_ptr };
        if has_prototype {
            // 原型对象：constructor 回指 + @@toStringTag（四弱族）；属性装完再
            // 登记释放登记表，登记点世代即洁净基线。
            let mut proto = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto_ptr));
            let si_constructor = core.perm_interner().intern("constructor").0;
            set_data_property(
                core,
                &mut proto,
                si_constructor,
                JsValue::from_js_object(stub_ptr),
                PropAttributes::new(true, false, true),
            );
            if let Some(tag_name) = tag {
                let tag_key = oxide_types::private_key::make_well_known_symbol_key(
                    oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG,
                );
                let tag_val =
                    JsValue::perm_string(core.perm_interner().string_ptr(core.perm_interner().intern(tag_name).0));
                set_data_property(core, &mut proto, tag_key, tag_val, PropAttributes::new(false, false, true));
            }
            let proto_ptr = Box::into_raw(Box::new(proto));
            builtin_world.track_leaked_object(proto_ptr);

            let si_prototype = core.perm_interner().intern("prototype").0;
            set_data_property(
                core,
                stub_obj,
                si_prototype,
                JsValue::from_js_object(proto_ptr),
                PropAttributes::new(false, false, false),
            );
        }
        let si_length = core.perm_interner().intern("length").0;
        set_data_property(
            core,
            stub_obj,
            si_length,
            JsValue::int(arg_count as i32),
            PropAttributes::new(false, false, true),
        );
        let si_name = core.perm_interner().intern("name").0;
        let name_val = JsValue::perm_string(core.perm_interner().string_ptr(core.perm_interner().intern(name).0));
        set_data_property(core, stub_obj, si_name, name_val, PropAttributes::new(false, false, true));

        bind_global_value(core, global, name, JsValue::from_js_object(stub_ptr));
        builtin_world.stub_objects.push(stub);
    }
}
