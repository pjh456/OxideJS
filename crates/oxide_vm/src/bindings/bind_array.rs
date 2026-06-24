use std::sync::Arc;

use oxide_kernel::builtin::ArrayMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

use crate::bind_constructor;

pub fn bind_array(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let _array_methods = ArrayMethods {
        is_array: crate::builtins::array::array_is_array as *const (),
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
        values: crate::builtins::array::array_values as *const (),
    };

    session.builtin_world().bind_array_methods(
        &_array_methods,
        core.perm_interner().as_ref(),
        core.shape_forge().as_ref(),
    );

    let ctor_ptr = session.builtin_world().array_constructor.as_ptr() as *mut JsObject;
    bind_constructor!(core, global, "Array", ctor_ptr, crate::builtins::array::array_constructor, 1, hash: true);
}
