use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bindings::{bind_global_value, configure_native_constructor};

const STUBS: [(&str, *const (), u8); 8] = [
    ("Proxy", crate::builtins::stubs::proxy_stub as *const (), 2),
    ("BigInt", crate::builtins::stubs::bigint_stub as *const (), 1),
    ("WeakMap", crate::builtins::stubs::weakmap_stub as *const (), 0),
    ("WeakSet", crate::builtins::stubs::weakset_stub as *const (), 0),
    ("WeakRef", crate::builtins::stubs::weakref_stub as *const (), 1),
    ("FinalizationRegistry", crate::builtins::stubs::finalization_registry_stub as *const (), 1),
    ("SharedArrayBuffer", crate::builtins::stubs::shared_array_buffer_stub as *const (), 1),
    ("Atomics", crate::builtins::stubs::atomics_stub as *const (), 0),
];

pub fn bind_stubs(core: &Arc<KernelCore>, session: &mut KernelSession, global: &mut JsObject) {
    let builtin_world = Arc::get_mut(&mut session.builtin_world)
        .expect("BuiltinWorld must be uniquely owned during init_kernel_builtins");

    for (name, native_fn, arg_count) in STUBS {
        let mut stub = JsObject::new_empty(
            EMPTY_SHAPE_ID,
            JsValue::from_js_object(builtin_world.function_proto.as_ptr() as *mut JsObject),
        );
        stub.set_function(true);
        configure_native_constructor(&mut stub, native_fn, arg_count);

        let stub = P::new(stub);
        let stub_ptr = stub.as_ptr() as *mut JsObject;
        bind_global_value(core, global, name, JsValue::from_js_object(stub_ptr));
        builtin_world.stub_objects.push(stub);
    }
}
