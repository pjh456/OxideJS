use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use std::time::Instant;

use oxide_types::object::{JsObject, PropMetaEntry};
use oxide_types::value::JsValue;

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
}

impl SessionGc {
    pub fn new() -> Self {
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
        }
    }

    #[inline]
    pub(crate) fn is_session_ptr(&self, vm: &Vm, obj_ptr: *mut JsObject) -> bool {
        vm.is_session_ptr(obj_ptr)
    }

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

    fn object_edges(&self, obj: &JsObject) -> Vec<JsValue> {
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
        edges
    }

    pub(crate) fn mark(&mut self, vm: &Vm, roots: &[JsValue]) {
        let mut queue = VecDeque::<*mut JsObject>::new();

        for root in roots {
            if !root.is_object() {
                continue;
            }
            let ptr = root.as_js_object_ptr();
            if self.is_session_ptr(vm, ptr) {
                queue.push_back(ptr);
                continue;
            }
            if !ptr.is_null() {
                // SAFETY: object roots are produced by VM-owned fields and builtin objects.
                for edge in self.object_edges(unsafe { &*ptr }) {
                    if edge.is_object() {
                        let edge_ptr = edge.as_js_object_ptr();
                        if self.is_session_ptr(vm, edge_ptr) {
                            queue.push_back(edge_ptr);
                        }
                    }
                }
            }
        }

        while let Some(ptr) = queue.pop_front() {
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
                let edges = self.object_edges(obj);
                for edge in edges {
                    if !edge.is_object() {
                        continue;
                    }
                    let child_ptr = edge.as_js_object_ptr();
                    if self.is_session_ptr(vm, child_ptr) {
                        queue.push_back(child_ptr);
                    }
                }
            }
        }
    }

    fn drop_session_object_heap_data(obj_ptr: *mut JsObject) -> u64 {
        if obj_ptr.is_null() {
            return 0;
        }
        // SAFETY: `obj_ptr` is verified before calling this helper and points to a session object
        // owned by the VM session arena. We only reconstruct Boxes that were allocated in
        // `JsObject::ensure_hash_props`/`ensure_prop_meta` and then drop them once here.
        unsafe {
            let obj = &mut *obj_ptr;
            debug_assert!(obj.is_session_epoch());
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

            freed_bytes
        }
    }

    fn drop_dead_session_object(obj_ptr: *mut JsObject) -> u64 {
        Self::drop_session_object_heap_data(obj_ptr) + size_of::<JsObject>() as u64
    }

    pub(crate) fn sweep(&mut self, vm: &mut Vm) -> u64 {
        let old_ptrs = std::mem::take(&mut vm.session_object_ptrs);
        let mut forwarding: HashMap<*mut JsObject, *mut JsObject> = HashMap::new();
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
                let clone = unsafe { &*old_ptr }.clone_for_session_epoch();
                let new_ptr = new_arena.alloc(clone) as *mut JsObject;
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
        }

        rewrite_vm_roots(vm, &forwarding);

        vm.session_object_ptrs = forwarding.values().copied().collect();
        vm.session_epoch = new_arena;
        vm.session_bytes_allocated = vm.session_object_ptrs.len() * size_of::<JsObject>();

        self.clear_all_marks(vm);

        let total_ptrs = survivors + dead;
        if total_ptrs > 0 {
            if dead == 0 {
                eprintln!("[GC] sweep phase -> no objects collected ({} live, {} dead)", survivors, dead);
            } else {
                eprintln!("[GC] sweep phase: {total_ptrs} scanned, {survivors} live, {dead} dead, {freed_bytes} bytes");
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

    pub(crate) fn should_collect(&self, vm: &Vm) -> bool {
        !vm.session_object_ptrs.is_empty()
            && vm.session_bytes_allocated >= vm.kernel_core().config().session_gc_threshold
    }

    pub(crate) fn collect(&mut self, vm: &mut Vm) {
        let start = Instant::now();

        self.mark(vm, &vm.gc_roots());
        let freed_bytes = self.sweep(vm);

        let elapsed = start.elapsed();
        self.total_collections += 1;
        self.last_collection_duration_us = elapsed.as_micros() as u64;
        self.max_collection_duration_us = self.max_collection_duration_us.max(self.last_collection_duration_us);
        self.min_collection_duration_us = self.min_collection_duration_us.min(self.last_collection_duration_us);

        if freed_bytes > 0 || self.total_collections % 100 == 0 {
            eprintln!("{}", self.stats_summary());
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
        Self::new()
    }
}

fn rewrite_forwarded_value(value: JsValue, forwarding: &HashMap<*mut JsObject, *mut JsObject>) -> JsValue {
    if !value.is_object() {
        return value;
    }
    forwarding
        .get(&value.as_js_object_ptr())
        .map(|&ptr| JsValue::from_js_object(ptr))
        .unwrap_or(value)
}

fn rewrite_vm_roots(vm: &mut Vm, forwarding: &HashMap<*mut JsObject, *mut JsObject>) {
    for value in &mut vm.regs {
        *value = rewrite_forwarded_value(*value, forwarding);
    }
    for frame in &mut vm.frames {
        for value in frame.saved_regs.iter_mut() {
            *value = rewrite_forwarded_value(*value, forwarding);
        }
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
            caller_reg_limit: 0,
            saved_regs: vec![JsValue::from_js_object(frame_session)].into_boxed_slice(),
            saved_this: JsValue::from_js_object(this_session),
            saved_new_target: JsValue::from_js_object(child_session),
            callee: JsValue::from_js_object(child_session),
            construct_result_reg: None,
            constructed_this: Some(JsValue::from_js_object(child_session)),
            is_derived_constructor: false,
            continuation: FrameContinuation::None,
        });

        vm.regs[1] = JsValue::from_js_object(child_session);
        vm.exception_value = Some(JsValue::from_js_object(root_session));
        vm.pending_exception = Some(JsValue::from_js_object(child_session));
        vm.for_of_iters.push(JsValue::from_js_object(child_session));
        vm.last_for_of_result = JsValue::from_js_object(root_session);
        vm.constants.push(JsValue::from_js_object(frame_session));
        vm.sub_module_constants = vec![vec![JsValue::from_js_object(child_session)]];

        let roots = vm.gc_roots();
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
        let roots = vm.gc_roots();
        let mut gc = std::mem::replace(&mut vm.session_gc, SessionGc::new());
        gc.mark(&vm, &roots);
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
        let roots = vm.gc_roots();
        let mut gc = std::mem::replace(&mut vm.session_gc, SessionGc::new());
        gc.mark(&vm, &roots);
        let _ = gc.sweep(&mut vm);
        vm.session_gc = gc;

        assert_eq!(vm.session_object_ptrs.len(), 2);
        assert!(!vm.session_object_ptrs.contains(&root_session));
        assert!(!vm.session_object_ptrs.contains(&dead_session));
        assert!(!vm.session_object_ptrs.iter().any(|ptr| unsafe { (*(*ptr)).is_gc_marked() }));
    }

    fn vm_with_low_threshold() -> Vm {
        let mut cfg = KernelConfig::minimal();
        cfg.set_session_gc_threshold(1);
        let core = KernelCore::new(cfg);
        Vm::with_kernel_core(core)
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
        let key = vm.kernel_core.string_forge().intern("gcRoot").0;
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
}
