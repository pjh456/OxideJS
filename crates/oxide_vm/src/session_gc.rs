use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::time::Instant;

use oxide_kernel::vm_debug;
use oxide_types::object::{JsObject, JsString, PropMetaEntry};
use oxide_types::value::JsValue;
use rustc_hash::FxBuildHasher;

use crate::builtins::{array_buffer, data_view, map, regexp, set, typed_array};
use crate::vm::Vm;

pub struct SessionGc {
    pub total_collections: u64,
    pub total_bytes_freed: u64,
    pub total_objects_scanned: u64,
    pub total_objects_live: u64,
    pub total_objects_dead: u64,
    pub last_collection_objects_scanned: u64,
    pub last_collection_objects_live: u64,
    pub last_collection_objects_dead: u64,
    pub last_collection_bytes_freed: u64,
    pub last_collection_duration_us: u64,
    pub max_collection_duration_us: u64,
    pub min_collection_duration_us: u64,
    pub(crate) mark_stack: Vec<*mut JsObject>,
    pub(crate) live_strings: HashSet<*mut JsString, FxBuildHasher>,
}

impl SessionGc {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionGc {
    pub(crate) fn clear_all_marks(&mut self, vm: &mut Vm) {
        for &ptr in &vm.session_object_ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptrs in session_object_ptrs come only from `session_epoch.alloc` in
            // `promote_object_inner`, and are valid while the arena is alive.
            unsafe { (*ptr).set_gc_mark(false) };
        }
    }

    fn object_edges(obj: &JsObject) -> Vec<JsValue> {
        let mut edges = Vec::new();
        if let Some(props) = obj.hash_props_vec() {
            edges.extend(props.iter().copied().filter(|val| val.is_object()));
        }
        if let Some(meta) = obj.prop_meta_vec() {
            for entry in meta.iter().flatten() {
                if entry.get.is_object() {
                    edges.push(entry.get);
                }
                if entry.set.is_object() {
                    edges.push(entry.set);
                }
            }
        }
        if obj.proto().is_object() {
            edges.push(obj.proto());
        }
        if obj.captured_this().is_object() {
            edges.push(obj.captured_this());
        }
        if obj.home_object().is_object() {
            edges.push(obj.home_object());
        }
        if obj.is_map() {
            edges.extend(map::map_native_edges(obj));
        }
        if obj.is_set() {
            edges.extend(set::set_native_edges(obj));
        }
        if obj.is_typed_array_obj() {
            edges.extend(typed_array::typed_array_native_edges(obj));
        }
        if obj.is_data_view_obj() {
            edges.extend(data_view::data_view_native_edges(obj));
        }
        edges
    }

    /// Record session-string values held directly by `obj` into `live`. JsStrings hold no GC
    /// references, so "reaching" a string IS marking it — there is no string DFS stack. Permanent
    /// strings are also recorded here harmlessly; the sweep only iterates `session_string_ptrs`, so
    /// non-session pointers in `live` are simply never consulted.
    fn record_object_string_edges(live: &mut HashSet<*mut JsString, FxBuildHasher>, obj: &JsObject) {
        if let Some(props) = obj.hash_props_vec() {
            for value in props.iter() {
                if value.is_string() {
                    live.insert(value.as_string_ptr_mut());
                }
            }
        }
        if obj.captured_this().is_string() {
            live.insert(obj.captured_this().as_string_ptr_mut());
        }
        if obj.home_object().is_string() {
            live.insert(obj.home_object().as_string_ptr_mut());
        }
        if obj.is_map() {
            for value in map::map_native_edges(obj) {
                if value.is_string() {
                    live.insert(value.as_string_ptr_mut());
                }
            }
        }
        if obj.is_set() {
            for value in set::set_native_edges(obj) {
                if value.is_string() {
                    live.insert(value.as_string_ptr_mut());
                }
            }
        }
    }

    pub(crate) fn mark(&mut self, vm: &Vm) {
        let mut seeds = Vec::new();
        let mut string_seeds: Vec<*mut JsString> = Vec::new();
        vm.for_each_root(|root| {
            if root.is_object() {
                seeds.push(root.as_js_object_ptr());
            } else if root.is_string() {
                string_seeds.push(root.as_string_ptr_mut());
            }
        });

        let Self {
            mark_stack: stack,
            live_strings,
            ..
        } = self;
        stack.clear();
        live_strings.clear();
        for ptr in string_seeds {
            if !ptr.is_null() {
                live_strings.insert(ptr);
            }
        }

        for ptr in seeds {
            if ptr.is_null() {
                continue;
            }
            if vm.is_session_ptr(ptr) {
                stack.push(ptr);
                continue;
            }
            // SAFETY: object roots are produced by VM-owned fields and builtin objects.
            let obj = unsafe { &*ptr };
            Self::record_object_string_edges(live_strings, obj);
            for edge in Self::object_edges(obj) {
                if edge.is_object() {
                    let edge_ptr = edge.as_js_object_ptr();
                    if vm.is_session_ptr(edge_ptr) {
                        stack.push(edge_ptr);
                    }
                }
            }
        }

        while let Some(ptr) = stack.pop() {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr was discovered from a root/session edge and session root checks require
            // this to be a valid session object pointer.
            unsafe {
                let obj = &mut *ptr;
                if obj.is_gc_marked() {
                    continue;
                }
                obj.set_gc_mark(true);
                Self::record_object_string_edges(live_strings, obj);
                let edges = Self::object_edges(obj);
                for edge in edges {
                    if !edge.is_object() {
                        continue;
                    }
                    let child_ptr = edge.as_js_object_ptr();
                    if vm.is_session_ptr(child_ptr) {
                        stack.push(child_ptr);
                    }
                }
            }
        }
    }

    pub(crate) fn drop_object_heap_data(obj_ptr: *mut JsObject, require_session: bool) -> u64 {
        if obj_ptr.is_null() {
            return 0;
        }
        // SAFETY: `obj_ptr` is verified before calling this helper and points to a session object
        // owned by the VM session arena. We only reconstruct Boxes that were allocated in
        // `JsObject::ensure_hash_props`/`ensure_prop_meta` and then drop them once here.
        unsafe {
            let obj = &mut *obj_ptr;
            if require_session {
                debug_assert!(obj.is_session_epoch());
            }
            let mut freed_bytes = 0u64;

            let hash_ptr = obj.hash_props_raw() as *mut Vec<JsValue>;
            if !hash_ptr.is_null() {
                let vec = Box::from_raw(hash_ptr);
                freed_bytes += size_of::<Vec<JsValue>>() as u64 + (vec.capacity() * size_of::<JsValue>()) as u64;
                std::mem::drop(vec);
            }

            let meta_ptr = obj.prop_meta_raw() as *mut Vec<Option<PropMetaEntry>>;
            if !meta_ptr.is_null() {
                let vec = Box::from_raw(meta_ptr);
                freed_bytes += size_of::<Vec<Option<PropMetaEntry>>>() as u64
                    + (vec.capacity() * size_of::<Option<PropMetaEntry>>()) as u64;
                std::mem::drop(vec);
            }

            freed_bytes += map::drop_map_native(obj);
            freed_bytes += set::drop_set_native(obj);
            freed_bytes += array_buffer::drop_array_buffer_native(obj);
            freed_bytes += regexp::drop_regexp_native(obj);
            freed_bytes += typed_array::drop_typed_array_native(obj);
            freed_bytes += data_view::drop_data_view_native(obj);

            freed_bytes
        }
    }

    fn drop_session_object_heap_data(obj_ptr: *mut JsObject) -> u64 {
        Self::drop_object_heap_data(obj_ptr, true)
    }

    fn drop_dead_session_object(obj_ptr: *mut JsObject) -> u64 {
        Self::drop_session_object_heap_data(obj_ptr) + size_of::<JsObject>() as u64
    }

    /// Drop a dead session `JsString` (allocated by `Vm::new_string` via `Box::into_raw`),
    /// returning the bytes freed. Mirrors `Vm::free_session_string_heap_data`'s drop, but applied
    /// selectively to a single dead pointer.
    ///
    /// SAFETY: `ptr` must be a non-null pointer produced by `Box::into_raw(Box::new(JsString))` in
    /// `Vm::new_string`, still present in `session_string_ptrs`, and dropped exactly once.
    unsafe fn drop_dead_session_string(ptr: *mut JsString) -> u64 {
        let bytes = (size_of::<JsString>() + (*ptr).len()) as u64;
        drop(Box::from_raw(ptr));
        bytes
    }

    pub(crate) fn sweep(&mut self, vm: &mut Vm) -> u64 {
        let old_ptrs = std::mem::take(&mut vm.session_object_ptrs);
        let mut forwarding = std::mem::take(&mut vm.forwarding);
        let new_arena = bumpalo::Bump::new();
        let mut survivors = 0u64;
        let mut dead = 0u64;
        let mut freed_bytes = 0u64;

        for old_ptr in old_ptrs {
            if old_ptr.is_null() {
                continue;
            }
            // SAFETY: old_ptr comes from session_arena promotions and still points into the old
            // session arena while sweep runs.
            let is_live = unsafe { (*old_ptr).is_gc_marked() };
            if is_live {
                survivors += 1;
                let old_ref = unsafe { &*old_ptr };
                let clone = old_ref.clone_for_session_epoch();
                let new_ptr = new_arena.alloc(clone) as *mut JsObject;
                let new_ref = unsafe { &mut *new_ptr };
                if old_ref.is_map() {
                    map::clone_map_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_set() {
                    set::clone_set_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_array_buffer_obj() {
                    array_buffer::clone_array_buffer_native(old_ref, new_ref);
                } else if old_ref.is_typed_array_obj() {
                    typed_array::clone_typed_array_native_with_rewrite(old_ref, new_ref, |value| value);
                } else if old_ref.is_data_view_obj() {
                    data_view::clone_data_view_native_with_rewrite(old_ref, new_ref, |value| value);
                }
                forwarding.insert(old_ptr, new_ptr);
                freed_bytes += Self::drop_session_object_heap_data(old_ptr);
            } else {
                dead += 1;
                freed_bytes += Self::drop_dead_session_object(old_ptr);
            }
        }

        for &dst in forwarding.values() {
            // SAFETY: all pointers in forwarding are newly allocated and initialized objects.
            let obj = unsafe { &mut *dst };
            obj.rewrite_object_values(|value| {
                if value.is_object() {
                    let ptr = value.as_js_object_ptr();
                    if let Some(&fwd) = forwarding.get(&ptr) {
                        return JsValue::from_js_object(fwd);
                    }
                }
                value
            });
            if obj.is_map() {
                map::rewrite_map_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_set() {
                set::rewrite_set_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_typed_array_obj() {
                typed_array::rewrite_typed_array_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            } else if obj.is_data_view_obj() {
                data_view::rewrite_data_view_native(obj, |value| rewrite_forwarded_value(value, &forwarding));
            }
        }

        rewrite_vm_roots(vm, &forwarding);

        vm.session_object_ptrs = forwarding.values().copied().collect();
        forwarding.clear();
        vm.forwarding = forwarding;
        vm.session_epoch = new_arena;
        vm.session_bytes_allocated = vm.session_object_ptrs.len() * size_of::<JsObject>();

        self.clear_all_marks(vm);

        let total_ptrs = survivors + dead;
        if total_ptrs > 0 {
            if dead == 0 {
                vm_debug!("[GC] sweep phase -> no objects collected ({} live, {} dead)", survivors, dead);
            } else {
                vm_debug!(
                    "[GC] sweep phase: {} scanned, {} live, {} dead, {} bytes",
                    total_ptrs,
                    survivors,
                    dead,
                    freed_bytes
                );
            }
        }

        self.total_bytes_freed += freed_bytes;
        self.total_objects_scanned += total_ptrs;
        self.total_objects_live += survivors;
        self.total_objects_dead += dead;
        self.last_collection_objects_scanned = total_ptrs;
        self.last_collection_objects_live = survivors;
        self.last_collection_objects_dead = dead;
        self.last_collection_bytes_freed = freed_bytes;
        freed_bytes
    }

    /// Sweep session `JsString`s: keep the pointers recorded live during `mark()`, drop the rest
    /// via `Box::from_raw`. Live strings are NOT moved — Box addresses are stable — so no forwarding
    /// map and no root-pointer rewriting are needed. Run after the object sweep (which resets
    /// `session_bytes_allocated` to object-only), re-adding surviving string bytes. Returns the
    /// bytes freed.
    pub(crate) fn sweep_session_strings(&mut self, vm: &mut Vm) -> u64 {
        let old = std::mem::take(&mut vm.session_string_ptrs);
        let mut freed = 0u64;
        let mut live_bytes = 0usize;
        let mut live = Vec::with_capacity(old.len());
        for ptr in old {
            if ptr.is_null() {
                continue;
            }
            if self.live_strings.contains(&ptr) {
                // Survivor — address unchanged, no rewrite needed.
                // SAFETY: ptr is a live session-string box still owned by the VM.
                live_bytes += unsafe { size_of::<JsString>() + (*ptr).len() };
                live.push(ptr);
            } else {
                // SAFETY: ptr is in session_string_ptrs but not reachable; drop exactly once.
                freed += unsafe { Self::drop_dead_session_string(ptr) };
            }
        }
        vm.session_string_ptrs = live;
        vm.session_bytes_allocated = vm.session_bytes_allocated.saturating_add(live_bytes);

        self.total_bytes_freed = self.total_bytes_freed.saturating_add(freed);
        self.last_collection_bytes_freed = self.last_collection_bytes_freed.saturating_add(freed);
        freed
    }

    pub(crate) fn should_collect(&self, vm: &Vm) -> bool {
        (!vm.session_object_ptrs.is_empty() || !vm.session_string_ptrs.is_empty())
            && vm.session_bytes_allocated >= vm.kernel_core().config().session_gc_threshold
    }

    pub(crate) fn collect(&mut self, vm: &mut Vm) {
        let start = Instant::now();

        self.mark(vm);
        let mut freed_bytes = self.sweep(vm);
        freed_bytes += self.sweep_session_strings(vm);

        let elapsed = start.elapsed();
        self.total_collections += 1;
        self.last_collection_duration_us = elapsed.as_micros() as u64;
        self.max_collection_duration_us = self.max_collection_duration_us.max(self.last_collection_duration_us);
        self.min_collection_duration_us = self.min_collection_duration_us.min(self.last_collection_duration_us);

        if freed_bytes > 0 || self.total_collections % 100 == 0 {
            vm_debug!("{}", self.stats_summary());
        }
    }

    pub(crate) fn maybe_collect(&mut self, vm: &mut Vm) {
        if self.should_collect(vm) {
            self.collect(vm);
        }
    }

    pub(crate) fn stats_summary(&self) -> String {
        format!(
            "[GC] collection #{}: {} scanned, {} live, {} dead, {} freed, {}μs",
            self.total_collections,
            self.last_collection_objects_scanned,
            self.last_collection_objects_live,
            self.last_collection_objects_dead,
            self.last_collection_bytes_freed,
            self.last_collection_duration_us
        )
    }
}

impl Default for SessionGc {
    fn default() -> Self {
        Self {
            total_collections: 0,
            total_bytes_freed: 0,
            total_objects_scanned: 0,
            total_objects_live: 0,
            total_objects_dead: 0,
            last_collection_objects_scanned: 0,
            last_collection_objects_live: 0,
            last_collection_objects_dead: 0,
            last_collection_bytes_freed: 0,
            last_collection_duration_us: 0,
            max_collection_duration_us: 0,
            min_collection_duration_us: u64::MAX,
            mark_stack: Vec::new(),
            live_strings: HashSet::default(),
        }
    }
}

fn rewrite_forwarded_value(
    value: JsValue, forwarding: &HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>,
) -> JsValue {
    if !value.is_object() {
        return value;
    }
    forwarding
        .get(&value.as_js_object_ptr())
        .map(|&ptr| JsValue::from_js_object(ptr))
        .unwrap_or(value)
}

fn rewrite_vm_roots(vm: &mut Vm, forwarding: &HashMap<*mut JsObject, *mut JsObject, FxBuildHasher>) {
    for value in &mut vm.regs {
        *value = rewrite_forwarded_value(*value, forwarding);
    }
    for value in &mut vm.save_stack {
        *value = rewrite_forwarded_value(*value, forwarding);
    }
    for frame in &mut vm.frames {
        frame.saved_this = rewrite_forwarded_value(frame.saved_this, forwarding);
        frame.saved_new_target = rewrite_forwarded_value(frame.saved_new_target, forwarding);
        frame.callee = rewrite_forwarded_value(frame.callee, forwarding);
        frame.constructed_this = frame.constructed_this.map(|value| rewrite_forwarded_value(value, forwarding));
    }
    vm.exception_value = vm.exception_value.map(|value| rewrite_forwarded_value(value, forwarding));
    vm.pending_exception = vm.pending_exception.map(|value| rewrite_forwarded_value(value, forwarding));
    for value in &mut vm.for_of_iters {
        *value = rewrite_forwarded_value(*value, forwarding);
    }
    vm.last_for_of_result = rewrite_forwarded_value(vm.last_for_of_result, forwarding);
    for value in &mut vm.constants {
        *value = rewrite_forwarded_value(*value, forwarding);
    }
    for values in &mut vm.sub_module_constants {
        for value in values {
            *value = rewrite_forwarded_value(*value, forwarding);
        }
    }
    for values in &mut vm.saved_constants_stack {
        for value in values {
            *value = rewrite_forwarded_value(*value, forwarding);
        }
    }
    for iter in &mut vm.for_in_iters {
        if iter.is_null() {
            continue;
        }
        // SAFETY: for_in_iters stores live iterator pointers owned by the current VM epoch.
        unsafe {
            for value in (*(*iter)).keys.iter_mut() {
                *value = rewrite_forwarded_value(*value, forwarding);
            }
        }
    }
    let global_ptr = vm.session.global_object().as_ptr() as *mut JsObject;
    if !global_ptr.is_null() {
        // SAFETY: KernelSession owns global_object for the VM lifetime.
        unsafe {
            (*global_ptr).rewrite_object_values(|value| rewrite_forwarded_value(value, forwarding));
        }
    }
}

#[cfg(test)]
mod tests {
    use oxide_kernel::kernel::{KernelConfig, KernelCore};
    use oxide_types::object::JsObject;

    use super::*;
    use crate::builtins::{array_buffer, data_view, map, set, typed_array};
    use crate::native::NativeResult;
    use crate::vm::{CallFrame, FrameContinuation};

    fn plain_object(vm: &mut Vm) -> *mut JsObject {
        let proto_ptr = vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        vm.epoch.alloc(JsObject::new_empty(
            oxide_kernel::shape_forge::EMPTY_SHAPE_ID,
            JsValue::from_js_object(proto_ptr),
        ))
    }

    fn has_ptr(roots: &[JsValue], ptr: *mut JsObject) -> bool {
        roots
            .iter()
            .any(|value| value.is_object() && std::ptr::eq(value.as_js_object_ptr(), ptr))
    }

    #[test]
    fn gc_roots_contains_registers_frames_and_root_roots() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let frame_obj = plain_object(&mut vm);
        let saved_this = plain_object(&mut vm);
        let child = plain_object(&mut vm);

        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(child));
        }

        let root_session = vm.promote_object(root);
        let frame_session = vm.promote_object(frame_obj);
        let this_session = vm.promote_object(saved_this);
        let child_session = vm.promote_object(child);
        vm.regs[0] = JsValue::from_js_object(root_session);

        vm.frames.push(CallFrame {
            return_addr: 0,
            function_name: 0,
            caller_reg_limit: 1,
            saved_reg_offset: 0,
            saved_this: JsValue::from_js_object(this_session),
            saved_new_target: JsValue::from_js_object(child_session),
            callee: JsValue::from_js_object(child_session),
            construct_result_reg: None,
            constructed_this: Some(JsValue::from_js_object(child_session)),
            is_derived_constructor: false,
            continuation: FrameContinuation::None,
        });
        vm.save_stack.push(JsValue::from_js_object(frame_session));

        vm.regs[1] = JsValue::from_js_object(child_session);
        vm.exception_value = Some(JsValue::from_js_object(root_session));
        vm.pending_exception = Some(JsValue::from_js_object(child_session));
        vm.for_of_iters.push(JsValue::from_js_object(child_session));
        vm.last_for_of_result = JsValue::from_js_object(root_session);
        vm.constants.push(JsValue::from_js_object(frame_session));
        vm.sub_module_constants = vec![vec![JsValue::from_js_object(child_session)]];

        let mut roots = Vec::new();
        vm.for_each_root(|v| roots.push(v));
        assert!(has_ptr(&roots, root_session));
        assert!(has_ptr(&roots, frame_session));
        assert!(has_ptr(&roots, this_session));
        assert!(has_ptr(&roots, child_session));
        assert!(has_ptr(&roots, vm.session.global_object().as_ptr() as *mut JsObject));
        assert!(roots.len() >= 1);
        assert!(roots.contains(&JsValue::from_js_object(root_session)));
        assert_eq!(vm.exception_value, Some(JsValue::from_js_object(root_session)));
    }

    #[test]
    fn mark_phase_reaches_cycles_and_unreachable_are_unmarked() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let reachable = plain_object(&mut vm);
        let unreachable = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(reachable));
            (*reachable).set_prop_at(0, JsValue::from_js_object(root));
        }

        let root_session = vm.promote_object(root);
        let reachable_session = unsafe { (*root_session).get_prop_at(0).as_js_object_ptr() };
        let unreachable_session = vm.promote_object(unreachable);

        vm.regs[0] = JsValue::from_js_object(root_session);
        let mut gc = std::mem::take(&mut vm.session_gc);
        gc.mark(&vm);
        vm.session_gc = gc;

        assert!(unsafe { (*root_session).is_gc_marked() });
        assert!(unsafe { (*reachable_session).is_gc_marked() });
        assert!(!unsafe { (*unreachable_session).is_gc_marked() });
        assert_ne!(unreachable_session, root_session);
    }

    #[test]
    fn sweep_preserves_cycle_and_collects_unreachable() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let child = plain_object(&mut vm);
        let dead = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(child));
            (*child).set_prop_at(0, JsValue::from_js_object(root));
        }

        let root_session = vm.promote_object(root);
        vm.regs[0] = JsValue::from_js_object(root_session);
        let dead_session = vm.promote_object(dead);
        let mut gc = std::mem::take(&mut vm.session_gc);
        gc.mark(&vm);
        let _ = gc.sweep(&mut vm);
        vm.session_gc = gc;

        assert_eq!(vm.session_object_ptrs.len(), 2);
        assert!(!vm.session_object_ptrs.contains(&root_session));
        assert!(!vm.session_object_ptrs.contains(&dead_session));
        assert!(!vm.session_object_ptrs.iter().any(|ptr| unsafe { (*(*ptr)).is_gc_marked() }));
    }

    #[test]
    fn forwarding_is_cleared_after_sweep() {
        let mut vm = Vm::new();
        let root = plain_object(&mut vm);
        let child = plain_object(&mut vm);
        unsafe {
            (*root).set_prop_at(0, JsValue::from_js_object(child));
        }
        let root_session = vm.promote_object(root);
        vm.regs[0] = JsValue::from_js_object(root_session);
        let _ = vm.promote_object(child);
        let mut gc = std::mem::take(&mut vm.session_gc);
        gc.mark(&vm);
        let _ = gc.sweep(&mut vm);
        vm.session_gc = gc;

        // The reused forwarding map MUST be cleared after sweep, otherwise
        // promote would observe stale old->new entries pointing into the freed arena.
        assert!(vm.forwarding.is_empty());
    }

    fn vm_with_low_threshold() -> Vm {
        let mut cfg = KernelConfig::minimal();
        cfg.set_session_gc_threshold(1);
        let core = KernelCore::new(cfg);
        Vm::with_kernel_core(core)
    }

    fn native_ok(result: NativeResult) -> JsValue {
        match result {
            NativeResult::Ok(value) => value,
            NativeResult::Err(err) => panic!("native error: {err}"),
            NativeResult::TailCall { .. } => panic!("unexpected native bytecode call"),
        }
    }

    #[test]
    fn reset_maybe_collect_collects_after_threshold() {
        let mut vm = vm_with_low_threshold();
        let obj = plain_object(&mut vm);
        vm.promote_object(obj);
        assert!(!vm.session_object_ptrs.is_empty());
        let tracked_before = vm.session_object_ptrs.len();

        vm.regs[0] = JsValue::undefined();
        vm.regs[1] = JsValue::undefined();
        vm.reset();

        assert!(vm.session_object_ptrs.len() <= tracked_before);
        assert_eq!(vm.session_object_ptrs.len(), 0);
        assert_eq!(vm.session_bytes_allocated, 0);
    }

    #[test]
    fn gc_stats_summary_includes_collection() {
        let mut vm = vm_with_low_threshold();
        let obj = plain_object(&mut vm);
        vm.promote_object(obj);
        vm.regs[0] = JsValue::undefined();
        vm.maybe_collect_session_gc();
        let summary = vm.session_gc.stats_summary();
        assert!(summary.contains("[GC] collection"));
    }

    #[test]
    fn moving_sweep_rewrites_global_root_edges() {
        let mut vm = vm_with_low_threshold();
        let obj = plain_object(&mut vm);
        unsafe {
            (*obj).set_prop_at(0, JsValue::int(42));
        }
        let old_ptr = vm.promote_object(obj);
        let key = vm.kernel_core.perm_interner().intern("gcRoot").0;
        let global_ptr = vm.session.global_object().as_ptr() as *mut JsObject;
        unsafe {
            let global = &mut *global_ptr;
            vm.set_or_create_prop_value(global, key, JsValue::from_js_object(old_ptr));
        }

        vm.maybe_collect_session_gc();

        let global = vm.session.global_object();
        let pos = vm
            .kernel_core
            .shape_forge()
            .lookup_position(global.shape_id(), key)
            .expect("global slot");
        let new_value = global.get_prop_at(pos);
        assert!(new_value.is_object());
        assert!(!std::ptr::eq(new_value.as_js_object_ptr(), old_ptr));
        assert_eq!(unsafe { (*new_value.as_js_object_ptr()).get_prop_at(0) }, JsValue::int(42));
    }

    #[test]
    fn map_native_storage_is_not_a_normal_object_edge() {
        let mut vm = Vm::new();
        let map_value = native_ok(map::map_constructor(&mut vm, &[]));
        let map_obj = unsafe { &*map_value.as_js_object_ptr() };
        let native_ptr = map_obj.native_data() as *mut JsObject;

        assert!(map_obj.hash_props_vec().is_none());
        assert!(!map_obj.native_data().is_null());
        assert!(!SessionGc::object_edges(map_obj)
            .iter()
            .any(|edge| std::ptr::eq(edge.as_js_object_ptr(), native_ptr)));
    }

    #[test]
    fn session_gc_traces_map_object_key_and_value() {
        let mut vm = vm_with_low_threshold();
        let map_value = native_ok(map::map_constructor(&mut vm, &[]));
        let key = JsValue::from_js_object(plain_object(&mut vm));
        let value = JsValue::from_js_object(plain_object(&mut vm));
        vm.regs[0] = map_value;
        vm.regs[1] = key;
        vm.regs[2] = value;
        native_ok(map::map_set(&mut vm, &[0, 1, 2]));

        let map_session = vm.promote_object(map_value.as_js_object_ptr());
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(map_session);
        let mut gc = std::mem::take(&mut vm.session_gc);
        gc.collect(&mut vm);
        vm.session_gc = gc;

        let live_map = unsafe { &*vm.regs[0].as_js_object_ptr() };
        let edges = map::map_native_edges(live_map);
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|value| vm.is_session_ptr(value.as_js_object_ptr())));
        assert_eq!(vm.session_object_ptrs.len(), 3);
    }

    #[test]
    fn session_gc_traces_set_object_key() {
        let mut vm = vm_with_low_threshold();
        let set_value = native_ok(set::set_constructor(&mut vm, &[]));
        let key = JsValue::from_js_object(plain_object(&mut vm));
        vm.regs[0] = set_value;
        vm.regs[1] = key;
        native_ok(set::set_add(&mut vm, &[0, 1]));

        let set_session = vm.promote_object(set_value.as_js_object_ptr());
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(set_session);
        let mut gc = std::mem::take(&mut vm.session_gc);
        gc.collect(&mut vm);
        vm.session_gc = gc;

        let live_set = unsafe { &*vm.regs[0].as_js_object_ptr() };
        let edges = set::set_native_edges(live_set);
        assert_eq!(edges.len(), 1);
        assert!(vm.is_session_ptr(edges[0].as_js_object_ptr()));
        assert_eq!(vm.session_object_ptrs.len(), 2);
    }

    #[test]
    fn session_gc_keeps_shared_array_buffer_alive_through_view_native_edges() {
        let mut vm = vm_with_low_threshold();
        let root = plain_object(&mut vm);

        vm.regs[1] = JsValue::int(8);
        let buffer = native_ok(array_buffer::array_buffer_constructor(&mut vm, &[0, 1]));

        vm.regs[1] = buffer;
        let typed = native_ok(typed_array::int32array_constructor(&mut vm, &[0, 1]));

        vm.regs[1] = buffer;
        let view = native_ok(data_view::data_view_constructor(&mut vm, &[0, 1]));

        vm.regs[0] = view;
        vm.regs[1] = JsValue::int(0);
        vm.regs[2] = JsValue::int(42);
        vm.regs[3] = JsValue::bool(true);
        native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2, 3]));

        unsafe {
            (*root).set_prop_at(0, typed);
            (*root).set_prop_at(1, view);
        }

        let root_session = vm.promote_object(root);
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(root_session);
        let mut gc = std::mem::take(&mut vm.session_gc);
        gc.collect(&mut vm);
        vm.session_gc = gc;

        let live_root = unsafe { &*vm.regs[0].as_js_object_ptr() };
        let live_typed = live_root.get_prop_at(0);
        let live_view = live_root.get_prop_at(1);
        let typed_obj = unsafe { &*live_typed.as_js_object_ptr() };
        let view_obj = unsafe { &*live_view.as_js_object_ptr() };
        let typed_edges = typed_array::typed_array_native_edges(typed_obj);
        let view_edges = data_view::data_view_native_edges(view_obj);

        assert_eq!(typed_edges.len(), 1);
        assert_eq!(view_edges.len(), 1);
        assert!(vm.is_session_ptr(typed_edges[0].as_js_object_ptr()));
        assert!(vm.is_session_ptr(view_edges[0].as_js_object_ptr()));
        assert!(std::ptr::eq(typed_edges[0].as_js_object_ptr(), view_edges[0].as_js_object_ptr()));

        vm.regs[0] = live_typed;
        vm.regs[1] = JsValue::int(0);
        assert_eq!(native_ok(typed_array::typed_array_at(&mut vm, &[0, 1])), JsValue::int(42));

        vm.regs[0] = live_view;
        vm.regs[1] = JsValue::int(4);
        vm.regs[2] = JsValue::int(7);
        vm.regs[3] = JsValue::bool(true);
        native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2, 3]));

        vm.regs[0] = live_typed;
        vm.regs[1] = JsValue::int(1);
        assert_eq!(native_ok(typed_array::typed_array_at(&mut vm, &[0, 1])), JsValue::int(7));
    }

    #[test]
    fn session_gc_rewrites_buffer_retained_only_by_data_view_native_edge() {
        let mut vm = vm_with_low_threshold();
        vm.regs[1] = JsValue::int(8);
        let buffer = native_ok(array_buffer::array_buffer_constructor(&mut vm, &[0, 1]));
        vm.regs[1] = buffer;
        let view = native_ok(data_view::data_view_constructor(&mut vm, &[0, 1]));

        vm.regs[0] = view;
        vm.regs[1] = JsValue::int(0);
        vm.regs[2] = JsValue::int(9);
        native_ok(data_view::data_view_set_int32(&mut vm, &[0, 1, 2]));

        let view_session = vm.promote_object(view.as_js_object_ptr());
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(view_session);
        let mut gc = std::mem::take(&mut vm.session_gc);
        gc.collect(&mut vm);
        vm.session_gc = gc;

        let live_view = vm.regs[0];
        let live_view_obj = unsafe { &*live_view.as_js_object_ptr() };
        let edges = data_view::data_view_native_edges(live_view_obj);
        assert_eq!(edges.len(), 1);
        assert!(vm.is_session_ptr(edges[0].as_js_object_ptr()));
        assert_eq!(vm.session_object_ptrs.len(), 2);

        vm.regs[0] = live_view;
        vm.regs[1] = JsValue::int(0);
        assert_eq!(native_ok(data_view::data_view_get_int32(&mut vm, &[0, 1])), JsValue::int(9));
    }

    fn collect(vm: &mut Vm) {
        let mut gc = std::mem::take(&mut vm.session_gc);
        gc.collect(vm);
        vm.session_gc = gc;
    }

    #[test]
    fn session_string_collected_when_dead() {
        let mut vm = Vm::new();
        let dead = vm.new_string("dead-string");
        let dead_ptr = dead.as_string_ptr_mut();
        assert!(vm.session_string_ptrs.contains(&dead_ptr));

        // No root references `dead` (it lives only on the Rust stack as a value wrapper).
        collect(&mut vm);

        assert!(!vm.session_string_ptrs.contains(&dead_ptr));
    }

    #[test]
    fn session_string_survives_when_in_register() {
        let mut vm = Vm::new();
        let live = vm.new_string("live-in-reg");
        let live_ptr = live.as_string_ptr_mut();
        vm.regs[0] = live;

        collect(&mut vm);

        assert!(vm.session_string_ptrs.contains(&live_ptr));
        // Live strings are never moved — the register still points at the same box.
        assert_eq!(vm.regs[0].as_string_ptr_mut(), live_ptr);
        assert_eq!(unsafe { (*live_ptr).as_str() }, "live-in-reg");
    }

    #[test]
    fn session_string_survives_via_object_property() {
        let mut vm = Vm::new();
        let obj = plain_object(&mut vm);
        let s = vm.new_string("prop-string");
        let s_ptr = s.as_string_ptr_mut();
        unsafe {
            (*obj).set_prop_at(0, s);
        }
        let obj_session = vm.promote_object(obj);

        // The string is reachable ONLY through the live object's property, not via any register.
        vm.regs.fill(JsValue::undefined());
        vm.regs[0] = JsValue::from_js_object(obj_session);

        collect(&mut vm);

        assert!(vm.session_string_ptrs.contains(&s_ptr));
        assert_eq!(unsafe { (*s_ptr).as_str() }, "prop-string");
    }

    #[test]
    fn permanent_string_untouched_by_sweep() {
        let mut vm = Vm::new();
        let perm = vm.perm_string("perm");
        let perm_ptr = perm.as_string_ptr_mut();
        // Permanent strings live in PermInterner, never in the session set.
        assert!(!vm.session_string_ptrs.contains(&perm_ptr));
        vm.regs[0] = perm;

        collect(&mut vm);

        // Never dropped by the session sweep (still readable, still absent from session set).
        assert!(!vm.session_string_ptrs.contains(&perm_ptr));
        assert_eq!(unsafe { (*perm_ptr).as_str() }, "perm");
    }

    #[test]
    fn string_sweep_byte_accounting() {
        let mut vm = Vm::new();
        let dead = vm.new_string("0123456789");
        let _ = dead.as_string_ptr_mut();
        let expected = (size_of::<JsString>() + "0123456789".len()) as u64;

        let mut gc = std::mem::take(&mut vm.session_gc);
        let before = gc.total_bytes_freed;
        gc.collect(&mut vm);
        let after = gc.total_bytes_freed;
        vm.session_gc = gc;

        assert!(after >= before + expected);
    }
}
