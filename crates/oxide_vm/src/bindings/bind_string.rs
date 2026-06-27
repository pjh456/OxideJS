use std::sync::Arc;

use crate::bindings::{apply_binding_table, configure_native_constructor};
use oxide_kernel::builtin::StringMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_string(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let string_methods = StringMethods {
        from_char_code: oxide_builtins::string::string_from_char_code::<crate::vm::Vm> as *const (),
        index_of: oxide_builtins::string::string_index_of::<crate::vm::Vm> as *const (),
        includes: oxide_builtins::string::string_includes::<crate::vm::Vm> as *const (),
        char_at: oxide_builtins::string::string_char_at::<crate::vm::Vm> as *const (),
        char_code_at: oxide_builtins::string::string_char_code_at::<crate::vm::Vm> as *const (),
        concat: oxide_builtins::string::string_concat::<crate::vm::Vm> as *const (),
        slice: oxide_builtins::string::string_slice::<crate::vm::Vm> as *const (),
        substring: oxide_builtins::string::string_substring::<crate::vm::Vm> as *const (),
        to_upper_case: oxide_builtins::string::string_to_upper_case::<crate::vm::Vm> as *const (),
        to_lower_case: oxide_builtins::string::string_to_lower_case::<crate::vm::Vm> as *const (),
        trim: oxide_builtins::string::string_trim::<crate::vm::Vm> as *const (),
        repeat: oxide_builtins::string::string_repeat::<crate::vm::Vm> as *const (),
        pad_start: oxide_builtins::string::string_pad_start::<crate::vm::Vm> as *const (),
        pad_end: oxide_builtins::string::string_pad_end::<crate::vm::Vm> as *const (),
        starts_with: oxide_builtins::string::string_starts_with::<crate::vm::Vm> as *const (),
        ends_with: oxide_builtins::string::string_ends_with::<crate::vm::Vm> as *const (),
        split: oxide_builtins::string::string_split::<crate::vm::Vm> as *const (),
        replace: oxide_builtins::string::string_replace::<crate::vm::Vm> as *const (),
        match_fn: oxide_builtins::string::string_match_fn::<crate::vm::Vm> as *const (),
        search: oxide_builtins::string::string_search::<crate::vm::Vm> as *const (),
        trim_start: oxide_builtins::string::string_trim_start::<crate::vm::Vm> as *const (),
        trim_end: oxide_builtins::string::string_trim_end::<crate::vm::Vm> as *const (),
        code_point_at: oxide_builtins::string::string_code_point_at::<crate::vm::Vm> as *const (),
        normalize: oxide_builtins::string::string_normalize::<crate::vm::Vm> as *const (),
        match_all: oxide_builtins::string::string_match_all::<crate::vm::Vm> as *const (),
        replace_all: oxide_builtins::string::string_replace_all::<crate::vm::Vm> as *const (),
        value_of: oxide_builtins::string::string_value_of::<crate::vm::Vm> as *const (),
        substr: oxide_builtins::string::string_substr::<crate::vm::Vm> as *const (),
        at: oxide_builtins::string::string_at::<crate::vm::Vm> as *const (),
        last_index_of: oxide_builtins::string::string_last_index_of::<crate::vm::Vm> as *const (),
    };
    session.builtin_world().bind_string_methods(
        &string_methods,
        core.perm_interner().as_ref(),
        core.shape_forge().as_ref(),
    );

    let ctor_ptr = session.builtin_world().string_constructor.as_ptr() as *mut JsObject;
    let ctor = unsafe { &mut *ctor_ptr };
    configure_native_constructor(ctor, oxide_builtins::string::string_constructor::<crate::vm::Vm> as *const (), 1);

    let proto_ptr = session.builtin_world().string_proto.as_ptr() as *mut JsObject;
    let proto = unsafe { &mut *proto_ptr };
    apply_binding_table(
        session.builtin_world(),
        proto,
        core,
        &[("toString", oxide_builtins::string::string_to_string::<crate::vm::Vm> as *const (), 0)],
    );

    let si_str = core.perm_interner().intern("String").0;
    let str_shape = core.shape_forge().make_shape(global.shape_id(), si_str);
    let str_val = JsValue::from_js_object(session.builtin_world().string_constructor.as_ptr() as *mut JsObject);
    global.set_shape_id(str_shape);
    global.ensure_hash_props().push(str_val);
    global.bump_generation();
}
