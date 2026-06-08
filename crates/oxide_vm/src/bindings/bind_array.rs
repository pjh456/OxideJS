use std::sync::Arc;

use crate::bind_constructor_hash;
use oxide_kernel::builtin::ArrayMethods;
use oxide_kernel::kernel::OxideKernel;
use oxide_types::object::JsObject;

pub fn bind_array(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let _array_methods = ArrayMethods {
        push: crate::builtins::array::array_push as *const (),
        pop: crate::builtins::array::array_pop as *const (),
        slice: crate::builtins::array::array_slice as *const (),
        splice: crate::builtins::array::array_splice as *const (),
        concat: crate::builtins::array::array_concat as *const (),
        join: crate::builtins::array::array_join as *const (),
        index_of: crate::builtins::array::array_index_of as *const (),
        includes: crate::builtins::array::array_includes as *const (),
        reverse: crate::builtins::array::array_reverse as *const (),
        for_each: crate::builtins::array::array_for_each as *const (),
        map: crate::builtins::array::array_map as *const (),
        filter: crate::builtins::array::array_filter as *const (),
        reduce: crate::builtins::array::array_reduce as *const (),
        find: crate::builtins::array::array_find as *const (),
        some: crate::builtins::array::array_some as *const (),
        every: crate::builtins::array::array_every as *const (),
        flat: crate::builtins::array::array_flat as *const (),
        flat_map: crate::builtins::array::array_flat_map as *const (),
        shift: crate::builtins::array::array_shift as *const (),
        unshift: crate::builtins::array::array_unshift as *const (),
        fill: crate::builtins::array::array_fill as *const (),
        copy_within: crate::builtins::array::array_copy_within as *const (),
        at: crate::builtins::array::array_at as *const (),
        last_index_of: crate::builtins::array::array_last_index_of as *const (),
        find_index: crate::builtins::array::array_find_index as *const (),
        find_last: crate::builtins::array::array_find_last as *const (),
        reduce_right: crate::builtins::array::array_reduce_right as *const (),
        sort: crate::builtins::array::array_sort as *const (),
    };

    kernel.builtin_world().bind_array_methods(
        &_array_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let ctor_ptr = kernel.builtin_world().array_constructor.as_ptr() as *mut JsObject;
    bind_constructor_hash!(
        kernel,
        global,
        "Array",
        ctor_ptr,
        crate::builtins::array::array_constructor,
        1
    );
}
