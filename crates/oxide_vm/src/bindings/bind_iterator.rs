use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bindings::{apply_binding_table, bind_global_value, configure_native_constructor};

pub fn bind_iterator(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let function_proto = session.builtin_world().function_proto.as_ptr() as *mut JsObject;
    let mut iterator = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(function_proto)));
    iterator.set_function(true);
    configure_native_constructor(&mut iterator, crate::builtins::iterator::iterator_constructor as *const (), 0);

    apply_binding_table(
        session.builtin_world(),
        &mut iterator,
        core,
        &[("from", crate::builtins::iterator::iterator_from as *const (), 1)],
    );

    bind_global_value(core, global, "Iterator", JsValue::from_js_object(Box::into_raw(iterator)));
}
