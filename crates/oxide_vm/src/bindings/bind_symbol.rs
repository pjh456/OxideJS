use std::sync::Arc;

use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bind_constructor_hash;

pub fn bind_symbol(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().symbol_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().symbol_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };

    configure_native_constructor(
        ctor,
        crate::builtins::symbol::symbol_constructor as *const (),
        1,
    );

    apply_binding_table(
        kernel.builtin_world(),
        proto,
        kernel,
        &[(
            "toString",
            crate::builtins::symbol::symbol_to_string as *const (),
            0,
        )],
    );

    for (name, val) in [
        (
            "match",
            JsValue::from_js_object(kernel.builtin_world().sym_match.as_ptr() as *mut JsObject),
        ),
        (
            "replace",
            JsValue::from_js_object(kernel.builtin_world().sym_replace.as_ptr() as *mut JsObject),
        ),
        (
            "search",
            JsValue::from_js_object(kernel.builtin_world().sym_search.as_ptr() as *mut JsObject),
        ),
        (
            "split",
            JsValue::from_js_object(kernel.builtin_world().sym_split.as_ptr() as *mut JsObject),
        ),
        (
            "iterator",
            JsValue::from_js_object(kernel.builtin_world().sym_iterator.as_ptr() as *mut JsObject),
        ),
    ] {
        bind_well_known_symbol(kernel, ctor, name, val);
    }

    bind_constructor_hash!(
        kernel,
        global,
        "Symbol",
        ctor_ptr,
        crate::builtins::symbol::symbol_constructor,
        1
    );
}

fn bind_well_known_symbol(
    kernel: &Arc<OxideKernel>,
    ctor: &mut JsObject,
    name: &str,
    val: JsValue,
) {
    let si = kernel.string_forge().intern(name).0;
    let shape_id = kernel.shape_forge().make_shape(ctor.shape_id(), si);
    ctor.set_shape_id(shape_id);
    ctor.ensure_hash_props().push(Box::new(val));
    ctor.bump_generation();
}
