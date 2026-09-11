use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bindings::{
    apply_binding_table, bind_global_value, bind_iterator_ctor_identity, bind_iterator_function_prototype,
    configure_native_constructor,
};

/// 绑定 `Iterator` 全局构造器（`prototype` = %IteratorPrototype%、`from` 方法）。
///
/// 迭代器原型方法（%IteratorPrototype% 的 `@@iterator` 与各集合原型 `next`）由
/// `rebind_dirty_builtins` 的 object 分支统一安装，本入口不重复绑定，避免
/// 保留原型经 dirty reset 时属性槽膨胀。
pub fn bind_iterator(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let function_proto = session.builtin_world().function_proto.as_ptr() as *mut JsObject;
    let mut iterator = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto)));
    iterator.set_function(true);
    configure_native_constructor(
        &mut iterator,
        oxide_builtins::iterator::iterator_constructor::<crate::vm::Vm> as *const (),
        0,
    );
    bind_iterator_ctor_identity(core, &mut iterator);
    bind_iterator_function_prototype(core, session, &mut iterator);

    apply_binding_table(
        session.builtin_world(),
        &mut iterator,
        core,
        &[("from", oxide_builtins::iterator::iterator_from::<crate::vm::Vm> as *const (), 1)],
    );

    let iterator_ptr = Box::into_raw(iterator);
    // 登记进 world 释放表：session 收尾统一释放构造器本体与属性区。
    session.builtin_world().track_leaked_object(iterator_ptr);
    bind_global_value(core, global, "Iterator", JsValue::from_js_object(iterator_ptr));
}
