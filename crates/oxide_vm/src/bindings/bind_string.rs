use std::sync::Arc;

use oxide_kernel::builtin::StringMethods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub fn bind_string(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let string_methods = StringMethods {
        index_of: crate::builtins::string::string_index_of as *const (),
        includes: crate::builtins::string::string_includes as *const (),
        char_at: crate::builtins::string::string_char_at as *const (),
        char_code_at: crate::builtins::string::string_char_code_at as *const (),
        concat: crate::builtins::string::string_concat as *const (),
        slice: crate::builtins::string::string_slice as *const (),
        substring: crate::builtins::string::string_substring as *const (),
        to_upper_case: crate::builtins::string::string_to_upper_case as *const (),
        to_lower_case: crate::builtins::string::string_to_lower_case as *const (),
        trim: crate::builtins::string::string_trim as *const (),
        repeat: crate::builtins::string::string_repeat as *const (),
        pad_start: crate::builtins::string::string_pad_start as *const (),
        pad_end: crate::builtins::string::string_pad_end as *const (),
        starts_with: crate::builtins::string::string_starts_with as *const (),
        ends_with: crate::builtins::string::string_ends_with as *const (),
        split: crate::builtins::string::string_split as *const (),
        replace: crate::builtins::string::string_replace as *const (),
        match_fn: crate::builtins::string::string_match_fn as *const (),
        search: crate::builtins::string::string_search as *const (),
        trim_start: crate::builtins::string::string_trim_start as *const (),
        trim_end: crate::builtins::string::string_trim_end as *const (),
        code_point_at: crate::builtins::string::string_code_point_at as *const (),
        normalize: crate::builtins::string::string_normalize as *const (),
        match_all: crate::builtins::string::string_match_all as *const (),
        replace_all: crate::builtins::string::string_replace_all as *const (),
    };
    kernel.builtin_world().bind_string_methods(
        &string_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_str = kernel.string_forge().intern("String").0;
    let str_shape = kernel.shape_forge().make_shape(global.shape_id(), si_str);
    let str_val = JsValue::from_js_object(
        kernel.builtin_world().string_constructor.as_ptr() as *mut JsObject
    );
    global.set_shape_id(str_shape);
    global.ensure_hash_props().push(Box::new(str_val));
    global.bump_generation();
}
