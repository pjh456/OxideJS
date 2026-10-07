use std::sync::Arc;

use oxide_kernel::builtin::{FnWrapperKey, FunctionMethods};
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use super::bind_global_value;
use super::configure_native_constructor;

/// 把 Function 构造器与原型方法绑定到 global。
///
/// `realm_id` 是目标所属 realm 的编号：`@@hasInstance` 符号键按 (realm 编号,
/// 局部下标) 编码，realm 编号为 0 时与旧编码逐字节一致。
pub fn bind_function(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject, realm_id: u32) {
    let function_methods = FunctionMethods {
        call: oxide_builtins::function::function_call::<crate::vm::Vm> as *const (),
        apply: oxide_builtins::function::function_apply::<crate::vm::Vm> as *const (),
        bind: oxide_builtins::function::function_bind::<crate::vm::Vm> as *const (),
        to_string: oxide_builtins::function::function_to_string::<crate::vm::Vm> as *const (),
        has_instance: oxide_builtins::function::function_symbol_has_instance::<crate::vm::Vm> as *const (),
    };
    session.builtin_world().bind_function_methods(
        &function_methods,
        core.perm_interner().as_ref(),
        core.shape_forge().as_ref(),
        realm_id,
    );

    let function_ctor = session.builtin_world().function_constructor.as_ptr() as *mut JsObject;
    configure_native_constructor(
        unsafe { &mut *function_ctor },
        oxide_builtins::function::function_constructor::<crate::vm::Vm> as *const (),
        1,
    );
    // 设置 Function.length = 1（configure_native_constructor 不写 length 属性），
    // 属性为不可写不可枚举可配置。
    let length_si = core.perm_interner().intern("length").0;
    let length_shape = core.shape_forge().make_shape(unsafe { &*function_ctor }.shape_id(), length_si);
    let ctor = unsafe { &mut *function_ctor };
    ctor.set_shape_id(length_shape);
    ctor.ensure_hash_props().push(JsValue::int(1));
    let length_pos = ctor.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    ctor.set_data_meta(length_pos, PropAttributes::new(false, false, true));

    // Function.prototype 自身属性：length = 0、name = ""（不可写不可枚举可配置），
    // 自身属性名序 length 在 name 之前。
    let proto_ptr = session.builtin_world().function_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    let proto_length_shape = core.shape_forge().make_shape(proto.shape_id(), length_si);
    proto.set_shape_id(proto_length_shape);
    proto.ensure_hash_props().push(JsValue::int(0));
    let proto_length_pos = proto.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    proto.set_data_meta(proto_length_pos, PropAttributes::new(false, false, true));
    let name_si = core.perm_interner().intern("name").0;
    let name_shape = core.shape_forge().make_shape(proto.shape_id(), name_si);
    proto.set_shape_id(name_shape);
    proto
        .ensure_hash_props()
        .push(JsValue::perm_string(core.perm_interner().string_ptr(core.perm_interner().intern("").0)));
    let name_pos = proto.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
    proto.set_data_meta(name_pos, PropAttributes::new(false, false, true));

    // caller/arguments 受限访问器：两属性 get/set 共用同一 %ThrowTypeError%
    // 函数对象，任何访问抛 TypeError（AddRestrictedFunctionProperties 语义）。
    bind_function_proto_restricted(core, session, proto);

    bind_global_value(core, global, "Function", JsValue::from_js_object(function_ctor));
}

/// 在 Function.prototype 上绑定 caller/arguments 受限访问器：两属性的
/// get/set 共用同一 %ThrowTypeError% 函数对象（匿名、length=0、冻结、
/// 不可扩展），描述符 { enumerable:false, configurable:true }。
///
/// # 步骤
/// 1. 取共享 %ThrowTypeError% 对象：选择性重建复用（同键旧对象迁移），
///    未命中时新建（length 先于 name、描述符三标志全 false）。
/// 2. 对象指针写入 world Cell（运行期创建 arguments 对象时读取，跨形态恒等）。
/// 3. 为 caller/arguments 各开 shape 槽位并写入访问器 meta。
///
/// # 副作用
/// - 为 caller/arguments 各开 shape 槽位并写入访问器 meta；thrower 函数对象
///   登记进 world 释放表（与 `bind_accessor_getter` 的 getter 同一生命周期）。
fn bind_function_proto_restricted(core: &Arc<KernelCore>, session: &KernelSession, proto: &mut JsObject) {
    let shape_forge = core.shape_forge().as_ref();
    let string_forge = core.perm_interner().as_ref();
    let world = session.builtin_world();
    let family = world.wrapper_family_of(proto as *const JsObject);
    let si_name = string_forge.intern("name").0;
    let si_length = string_forge.intern("length").0;
    let si_caller = string_forge.intern("caller").0;
    let si_arguments = string_forge.intern("arguments").0;
    let si_label = string_forge.intern("ThrowTypeError").0;

    // 取共享 %ThrowTypeError% 对象，供两属性 get/set 与 unmapped arguments.callee
    // 访问器共用（选择性重建复用：同键旧对象迁移，不新建，跨形态恒等）。
    // SAFETY: thrower 函数项指针转为 *const ()。
    let thrower_fn_ptr =
        unsafe { NativeFnPtr::from_raw(oxide_builtins::function::throw_type_error::<crate::vm::Vm> as *const ()) };
    let reuse_key = FnWrapperKey::new(family, si_label, si_caller, si_name);
    let thrower_ptr = match world.find_fn_wrapper(reuse_key, thrower_fn_ptr, 0) {
        Some(ptr) => ptr,
        None => {
            let fn_proto_val = JsValue::from_js_object(world.function_proto.as_ptr() as *mut JsObject);
            let mut thrower = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
            thrower.set_function(true);
            thrower.set_native_fn(Some(thrower_fn_ptr));
            thrower.set_native_arg_count(0);
            // length 先于 name（ownNames 序 ["length","name"]），描述符
            // { writable:false, enumerable:false, configurable:false }。
            let length_shape = shape_forge.make_shape(thrower.shape_id(), si_length);
            thrower.set_shape_id(length_shape);
            thrower.ensure_hash_props().push(JsValue::int(0));
            let length_pos = thrower.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
            thrower.set_data_meta(length_pos, PropAttributes::new(false, false, false));
            let name_shape = shape_forge.make_shape(thrower.shape_id(), si_name);
            thrower.set_shape_id(name_shape);
            thrower
                .ensure_hash_props()
                .push(JsValue::perm_string(string_forge.string_ptr(string_forge.intern("").0)));
            let name_pos = thrower.hash_props_vec().map_or(0, |v| v.len() as u32).saturating_sub(1);
            thrower.set_data_meta(name_pos, PropAttributes::new(false, false, false));
            thrower.set_extensible(false);
            thrower.set_frozen(true);
            let ptr = Box::into_raw(thrower);
            world.track_fn_wrapper(ptr, reuse_key);
            ptr
        }
    };
    // 指针写入 world Cell：运行期创建 arguments 对象时读取，保证跨形态恒等。
    world.throw_type_error.set(thrower_ptr);
    let thrower_val = JsValue::from_js_object(thrower_ptr);
    let attrs = PropAttributes::new(false, false, true);
    for key in [si_caller, si_arguments] {
        let new_shape = shape_forge.make_shape(proto.shape_id(), key);
        proto.set_shape_id(new_shape);
        let pos = proto.push_prop(JsValue::undefined());
        proto.set_accessor_meta(pos, thrower_val, thrower_val, attrs);
        proto.bump_generation();
    }
}
