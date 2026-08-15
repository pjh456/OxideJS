use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bindings::{
    apply_binding_table, bind_global_value, bind_iterator_function_prototype, bind_iterator_proto_next,
    configure_native_constructor,
};

/// 绑定迭代器基础设施：`%IteratorPrototype%` 与 `%ArrayIteratorPrototype%` 等。
pub fn bind_iterator(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let function_proto = session.builtin_world().function_proto.as_ptr() as *mut JsObject;
    let mut iterator = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto)));
    iterator.set_function(true);
    configure_native_constructor(
        &mut iterator,
        oxide_builtins::iterator::iterator_constructor::<crate::vm::Vm> as *const (),
        0,
    );
    bind_iterator_function_prototype(core, session, &mut iterator);

    apply_binding_table(
        session.builtin_world(),
        &mut iterator,
        core,
        &[("from", oxide_builtins::iterator::iterator_from::<crate::vm::Vm> as *const (), 1)],
    );

    bind_global_value(core, global, "Iterator", JsValue::from_js_object(Box::into_raw(iterator)));

    // %IteratorPrototype% 自身可迭代：@@iterator 返回 this，经原型链被所有
    // 集合迭代器继承（it[Symbol.iterator]() === it 恒等）。
    let iter_proto_ptr = session.builtin_world().iterator_proto.as_ptr() as *mut JsObject;
    let iter_proto = unsafe { &mut *iter_proto_ptr };
    crate::bindings::bind_well_known_method(
        session.builtin_world(),
        core,
        iter_proto,
        0,
        "iterator",
        oxide_builtins::iterator::iterator_symbol_iterator::<crate::vm::Vm> as *const (),
        0,
    );

    // 集合迭代器原型各自安装 next（%ArrayIteratorPrototype% 服务 Array/TA 两族；
    // Map/Set 共用按 `__mode__` 分发的实现；%RegExpStringIteratorPrototype% 供 matchAll）。
    let world = session.builtin_world();
    bind_iterator_proto_next(
        core,
        session,
        world.array_iterator_proto.as_ptr() as *mut JsObject,
        oxide_builtins::array::array_iterator_next::<crate::vm::Vm> as *const (),
    );
    bind_iterator_proto_next(
        core,
        session,
        world.map_iterator_proto.as_ptr() as *mut JsObject,
        oxide_builtins::iterator::map_set_iterator_next::<crate::vm::Vm> as *const (),
    );
    bind_iterator_proto_next(
        core,
        session,
        world.set_iterator_proto.as_ptr() as *mut JsObject,
        oxide_builtins::iterator::map_set_iterator_next::<crate::vm::Vm> as *const (),
    );
    bind_iterator_proto_next(
        core,
        session,
        world.regexp_string_iterator_proto.as_ptr() as *mut JsObject,
        oxide_builtins::string::string_match_all_next::<crate::vm::Vm> as *const (),
    );
}
