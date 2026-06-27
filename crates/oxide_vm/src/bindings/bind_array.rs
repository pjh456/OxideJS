use std::sync::Arc;

use oxide_kernel::builtin::ArrayMethods;
use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::{JsObject, PropAttributes};

use crate::bind_constructor;
use crate::bindings::apply_binding_table;

pub fn bind_array(core: &Arc<KernelCore>, session: &KernelSession, global: &mut JsObject) {
    let _array_methods = ArrayMethods {
        is_array: oxide_builtins::array::array_is_array::<crate::vm::Vm> as *const (),
        push: oxide_builtins::array::array_push::<crate::vm::Vm> as *const (),
        pop: oxide_builtins::array::array_pop::<crate::vm::Vm> as *const (),
        slice: oxide_builtins::array::array_slice::<crate::vm::Vm> as *const (),
        splice: oxide_builtins::array::array_splice::<crate::vm::Vm> as *const (),
        concat: oxide_builtins::array::array_concat::<crate::vm::Vm> as *const (),
        join: oxide_builtins::array::array_join::<crate::vm::Vm> as *const (),
        index_of: oxide_builtins::array::array_index_of::<crate::vm::Vm> as *const (),
        includes: oxide_builtins::array::array_includes::<crate::vm::Vm> as *const (),
        reverse: oxide_builtins::array::array_reverse::<crate::vm::Vm> as *const (),
        for_each: oxide_builtins::array::array_for_each::<crate::vm::Vm> as *const (),
        map: oxide_builtins::array::array_map::<crate::vm::Vm> as *const (),
        filter: oxide_builtins::array::array_filter::<crate::vm::Vm> as *const (),
        reduce: oxide_builtins::array::array_reduce::<crate::vm::Vm> as *const (),
        find: oxide_builtins::array::array_find::<crate::vm::Vm> as *const (),
        some: oxide_builtins::array::array_some::<crate::vm::Vm> as *const (),
        every: oxide_builtins::array::array_every::<crate::vm::Vm> as *const (),
        flat: oxide_builtins::array::array_flat::<crate::vm::Vm> as *const (),
        flat_map: oxide_builtins::array::array_flat_map::<crate::vm::Vm> as *const (),
        shift: oxide_builtins::array::array_shift::<crate::vm::Vm> as *const (),
        unshift: oxide_builtins::array::array_unshift::<crate::vm::Vm> as *const (),
        fill: oxide_builtins::array::array_fill::<crate::vm::Vm> as *const (),
        copy_within: oxide_builtins::array::array_copy_within::<crate::vm::Vm> as *const (),
        at: oxide_builtins::array::array_at::<crate::vm::Vm> as *const (),
        last_index_of: oxide_builtins::array::array_last_index_of::<crate::vm::Vm> as *const (),
        find_index: oxide_builtins::array::array_find_index::<crate::vm::Vm> as *const (),
        find_last: oxide_builtins::array::array_find_last::<crate::vm::Vm> as *const (),
        reduce_right: oxide_builtins::array::array_reduce_right::<crate::vm::Vm> as *const (),
        sort: oxide_builtins::array::array_sort::<crate::vm::Vm> as *const (),
        values: oxide_builtins::array::array_values::<crate::vm::Vm> as *const (),
    };

    session.builtin_world().bind_array_methods(
        &_array_methods,
        core.perm_interner().as_ref(),
        core.shape_forge().as_ref(),
    );

    let array_proto_ptr = session.builtin_world().array_proto.as_ptr() as *mut JsObject;
    let array_proto = unsafe { &mut *array_proto_ptr };
    apply_binding_table(
        session.builtin_world(),
        array_proto,
        core,
        &[("toString", oxide_builtins::array::array_to_string::<crate::vm::Vm> as *const (), 0)],
    );
    // Built-in prototype methods are non-enumerable (otherwise they leak into for-in).
    let si = core.perm_interner().intern("toString").0;
    if let Some(pos) = core.shape_forge().lookup_position(array_proto.shape_id(), si) {
        array_proto.set_data_meta(pos, PropAttributes::new(true, false, true));
    }

    let ctor_ptr = session.builtin_world().array_constructor.as_ptr() as *mut JsObject;
    bind_constructor!(core, global, "Array", ctor_ptr, oxide_builtins::array::array_constructor::<crate::vm::Vm>, 1, hash: true);
}
