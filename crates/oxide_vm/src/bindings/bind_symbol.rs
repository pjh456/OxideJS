use std::sync::Arc;

use oxide_kernel::bind_method;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use crate::bind_constructor_hash;

pub fn bind_symbol(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let ctor_ptr = kernel.builtin_world().symbol_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    let proto_ptr = kernel.builtin_world().symbol_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();

    bind_method!(
        kernel.builtin_world(),
        proto,
        sf,
        sh,
        "toString",
        crate::builtins::symbol::symbol_to_string,
        0
    );

    let sym_match_val = JsValue::from_js_object(
        kernel.builtin_world().sym_match.as_ptr() as *mut JsObject,
    );
    bind_well_known_symbol(kernel, ctor, sf, sh, "match", sym_match_val);

    let sym_replace_val = JsValue::from_js_object(
        kernel.builtin_world().sym_replace.as_ptr() as *mut JsObject,
    );
    bind_well_known_symbol(kernel, ctor, sf, sh, "replace", sym_replace_val);

    let sym_search_val = JsValue::from_js_object(
        kernel.builtin_world().sym_search.as_ptr() as *mut JsObject,
    );
    bind_well_known_symbol(kernel, ctor, sf, sh, "search", sym_search_val);

    let sym_split_val = JsValue::from_js_object(
        kernel.builtin_world().sym_split.as_ptr() as *mut JsObject,
    );
    bind_well_known_symbol(kernel, ctor, sf, sh, "split", sym_split_val);

    let sym_iterator_val = JsValue::from_js_object(
        kernel.builtin_world().sym_iterator.as_ptr() as *mut JsObject,
    );
    bind_well_known_symbol(kernel, ctor, sf, sh, "iterator", sym_iterator_val);

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
    _sf: &oxide_kernel::string_forge::StringForge,
    _sh: &oxide_kernel::shape_forge::ShapeForge,
    name: &str,
    val: JsValue,
) {
    let si = kernel.string_forge().intern(name).0;
    let shape_id = kernel.shape_forge().make_shape(ctor.shape_id(), si);
    ctor.set_shape_id(shape_id);
    ctor.ensure_hash_props().push(Box::new(val));
}
