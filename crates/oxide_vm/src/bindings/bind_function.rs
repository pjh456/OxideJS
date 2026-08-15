use std::sync::Arc;

use oxide_kernel::builtin::FunctionMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use super::bind_global_value;
use super::configure_native_constructor;

/// 把 Function 构造器与原型方法绑定到 global。
pub fn bind_function(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
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
    bind_global_value(core, global, "Function", JsValue::from_js_object(function_ctor));
}
