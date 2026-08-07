#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_bytecode::module::Constant;

use crate::bindings;
use crate::vm::Vm;
use crate::vm_info;
use crate::vm_state::{GcState, IterState, ProfilingState, SymbolState};
use oxide_kernel::kernel::{KernelConfig, KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{JsObject, JsString, PropAttributes};
use oxide_types::value::JsValue;

impl Vm {
    /// 以最小配置创建独立 VM：新建 `KernelCore` + `KernelSession` 并初始化内置对象。
    pub fn new() -> Self {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        bindings::init_kernel_builtins(&core, &mut session);
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        let vm = Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            immutables_cache: Vec::new(),
            active_immutables: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            frames: smallvec::SmallVec::new(),
            kernel_core: core,
            session,
            epoch: Epoch::new(),
            object_prototype: obj_proto,
            math_rng_state: 0,
            sub_modules: Arc::new(Vec::new()),
            saved_bytecode_stack: Vec::new(),
            saved_immutables_stack: Vec::new(),
            save_stack: Vec::new(),
            spill_stack: Vec::new(),
            try_stack: Vec::new(),
            exception_value: None,
            last_uncaught_value: None,
            pending_exception: None,
            pending_error_kind: None,
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            accessor_frame_target_reg: None,
            inline_callee: None,
            gc_state: GcState {
                session_epoch: bumpalo::Bump::new(),
                session_gc: crate::session_gc::SessionGc::new(),
                epoch_object_ptrs: Vec::new(),
                session_object_ptrs: Vec::new(),
                session_string_ptrs: Vec::new(),
                session_bytes_allocated: 0,
                forwarding: std::collections::HashMap::with_hasher(rustc_hash::FxBuildHasher),
            },
            symbols: SymbolState {
                symbol_counter: 0,
                symbol_descriptions: Vec::new(),
                symbol_registry: std::collections::HashMap::new(),
            },
            iters: IterState {
                for_in_iters: Vec::new(),
                for_of_iters: Vec::new(),
                last_for_of_result: JsValue::undefined(),
            },
            profiling: ProfilingState {
                ic_hits: std::cell::Cell::new(0),
                ic_misses: std::cell::Cell::new(0),
                instruction_count: 0,
            },
            string_buf: String::new(),
            cell_stack: Vec::new(),
        };
        vm_info!("Vm created");
        vm
    }

    /// 复用共享 `KernelCore` 创建 VM（VM 池路径），共享 intern/shape/code 缓存。
    pub fn with_kernel_core(core: Arc<KernelCore>) -> Self {
        let mut session = KernelSession::new(&core);
        bindings::init_kernel_builtins(&core, &mut session);
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        let vm = Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            immutables_cache: Vec::new(),
            active_immutables: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            frames: smallvec::SmallVec::new(),
            kernel_core: core,
            session,
            epoch: Epoch::new(),
            object_prototype: obj_proto,
            math_rng_state: 0,
            sub_modules: Arc::new(Vec::new()),
            saved_bytecode_stack: Vec::new(),
            saved_immutables_stack: Vec::new(),
            save_stack: Vec::new(),
            spill_stack: Vec::new(),
            try_stack: Vec::new(),
            exception_value: None,
            last_uncaught_value: None,
            pending_exception: None,
            pending_error_kind: None,
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            accessor_frame_target_reg: None,
            inline_callee: None,
            gc_state: GcState {
                session_epoch: bumpalo::Bump::new(),
                session_gc: crate::session_gc::SessionGc::new(),
                epoch_object_ptrs: Vec::new(),
                session_object_ptrs: Vec::new(),
                session_string_ptrs: Vec::new(),
                session_bytes_allocated: 0,
                forwarding: std::collections::HashMap::with_hasher(rustc_hash::FxBuildHasher),
            },
            symbols: SymbolState {
                symbol_counter: 0,
                symbol_descriptions: Vec::new(),
                symbol_registry: std::collections::HashMap::new(),
            },
            iters: IterState {
                for_in_iters: Vec::new(),
                for_of_iters: Vec::new(),
                last_for_of_result: JsValue::undefined(),
            },
            profiling: ProfilingState {
                ic_hits: std::cell::Cell::new(0),
                ic_misses: std::cell::Cell::new(0),
                instruction_count: 0,
            },
            string_buf: String::new(),
            cell_stack: Vec::new(),
        };
        vm_info!("Vm created (pool)");
        vm
    }

    /// 全量隔离重置：仅重建被污染的内置对象与 global，并清空所有执行状态与内存。
    ///
    /// 用于在多次 JS 执行之间达到完全隔离：session 内未被污染的 builtin 保留原指针。
    pub fn full_reset(&mut self) {
        let dirty = self.session.selective_reset(&self.kernel_core);
        if dirty.any_builtin_dirty() {
            bindings::rebind_dirty_builtins(&self.kernel_core, &mut self.session, Some(&dirty));
        }
        if dirty.global {
            let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
            let global = unsafe { &mut *global_ptr };
            bindings::bind_global_builtin_slots(&self.kernel_core, &self.session, global);
        }
        self.session.record_snapshot();
        self.object_prototype = P::clone(&self.session.builtin_world().object_proto);
        self.clear_full_reset_state();
        vm_info!("full_reset completed");
    }

    /// 旧版全量重置：总是丢弃并重建整个 session 与内置对象（benchmark 专用）。
    #[doc(hidden)]
    pub fn full_reset_legacy_for_bench(&mut self) {
        self.session = KernelSession::new(&self.kernel_core);
        bindings::init_kernel_builtins(&self.kernel_core, &mut self.session);
        self.object_prototype = P::clone(&self.session.builtin_world().object_proto);
        self.clear_full_reset_state();
    }

    fn clear_full_reset_state(&mut self) {
        self.clear_execution_state();
        self.bytecode.clear();
        self.immutables_cache.clear();
        self.active_immutables = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        self.free_epoch_object_heap_data();
        self.epoch.reset();
        self.gc_state.epoch_object_ptrs.clear();
        self.gc_state.session_epoch.reset();
        self.gc_state.session_object_ptrs.clear();
        self.gc_state.session_bytes_allocated = 0;
        self.gc_state.session_gc = crate::session_gc::SessionGc::new();
        self.free_session_string_heap_data();
        self.symbols.reset();
        self.root_reg_limit = 0;
        self.active_reg_limit = 0;
    }

    fn free_epoch_object_heap_data(&mut self) {
        let mut freed = 0u64;
        for ptr in self.gc_state.epoch_object_ptrs.drain(..) {
            freed += crate::session_gc::SessionGc::drop_object_heap_data(ptr, false);
        }
        if freed > 0 {
            self.gc_state.session_gc.total_bytes_freed =
                self.gc_state.session_gc.total_bytes_freed.saturating_add(freed);
            self.gc_state.session_gc.last_collection_bytes_freed = freed;
        }
    }

    pub(crate) fn clear_execution_state(&mut self) {
        // 重置契约：
        // - 清空寄存器文件、pc、帧/迭代器栈、保存的执行栈、try 处理器、
        //   待处理异常与 native 调用深度。
        // - 保留 kernel 共享状态不变。
        // - `reset()` 额外清空 bytecode/constants 并重置 epoch 归属。
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.frames.clear();
        self.iters.reset();
        self.saved_bytecode_stack.clear();
        self.saved_immutables_stack.clear();
        self.save_stack.clear();
        self.spill_stack.clear();
        self.cell_stack.clear();
        self.try_stack.clear();
        self.exception_value = None;
        self.pending_exception = None;
        self.pending_error_kind = None;
        self.native_call_depth = 0;
    }

    /// 轻量重置：清空执行状态并回收 epoch 内存，但保留 session 字符串与 builtin。
    pub fn reset(&mut self) {
        self.clear_execution_state();
        self.maybe_collect_session_gc();
        self.bytecode.clear();
        self.immutables_cache.clear();
        self.active_immutables = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        self.free_epoch_object_heap_data();
        self.epoch.reset();
        self.gc_state.epoch_object_ptrs.clear();
        self.root_reg_limit = 0;
        self.active_reg_limit = 0;
    }

    /// 分配一个可被 session GC 回收的字符串 `JsValue`（session-heap 字符串）。
    pub fn new_string(&mut self, s: &str) -> JsValue {
        self.new_string_owned(s.to_string())
    }

    /// 同 `new_string`，但以 move 接收 `String`，避免一次克隆。
    pub fn new_string_owned(&mut self, s: String) -> JsValue {
        let len = s.len();
        let ptr = Box::into_raw(Box::new(JsString::new(s)));
        self.gc_state.session_string_ptrs.push(ptr);
        self.gc_state.session_bytes_allocated += std::mem::size_of::<JsString>() + len;
        JsValue::string(ptr)
    }

    /// 把字符串 intern 为永久 key id（属性名/方法名），进程生命周期内稳定。
    pub fn intern_key(&self, s: &str) -> u32 {
        self.kernel_core.perm_interner().intern(s).0
    }

    /// 把编译期字符串字面量 intern 为永久、进程生命周期内、跨 session 共享的
    /// `JsString` 值。源码字面量与 RegExp 的 source/flags 会反复出现（模板化/
    /// 重复代码）且不可变，经 `PermInterner` 共享可恢复跨 session 字符串复用——
    /// 不 intern 瞬态计算值（它们走 session 堆 `new_string`，保持可回收）。
    pub fn perm_string(&self, s: &str) -> JsValue {
        let id = self.kernel_core.perm_interner().intern(s).0;
        JsValue::perm_string(self.kernel_core.perm_interner().string_ptr(id))
    }

    /// 释放全部 session 堆 `JsString`。仅在完全隔离重置
    /// （`full_reset` / `clear_full_reset_state`）时调用，此时没有存活的 session 对象
    /// 会引用它们。较轻量的 `reset()` 刻意保留它们，与 session 对象跨 eval 存活一致。
    fn free_session_string_heap_data(&mut self) {
        for ptr in self.gc_state.session_string_ptrs.drain(..) {
            // SAFETY: 每个指针来自 new_string 中的 Box::into_raw(Box::new(JsString))，
            // 且只在这里恰好释放一次。
            unsafe {
                drop(Box::from_raw(ptr));
            }
        }
    }

    /// 为 BytecodeFunc 常量创建函数 JsObject。
    /// 当 `is_arrow` 为 true 时，捕获当前 `this`（regs[254]），供调用时词法 this 绑定。
    pub(crate) fn create_function_object(
        &mut self, sub_idx: u32, is_arrow: bool, is_class_constructor: bool, is_derived_constructor: bool,
        needs_home_object: bool,
    ) -> JsValue {
        let func_proto_ptr = self.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let proto_val = JsValue::from_js_object(func_proto_ptr);
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
        obj.set_function(true);
        obj.set_sub_module_index(sub_idx);
        obj.set_class_constructor(is_class_constructor);
        obj.set_derived_constructor(is_derived_constructor);
        let _ = needs_home_object;
        if is_arrow {
            obj.set_arrow(true);
            obj.set_captured_this(self.regs[254]);
        }
        let obj_ptr = self.alloc_object(obj);
        let func_val = JsValue::object(obj_ptr as *mut u8);

        if !is_arrow {
            let object_proto_ptr = self.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
            let prototype_obj = self
                .epoch
                .alloc(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto_ptr)));
            self.gc_state.track_epoch_object(prototype_obj);
            let prototype_val = JsValue::from_js_object(prototype_obj);

            let constructor_si = self.kernel_core.perm_interner().intern("constructor").0;
            let constructor_shape = self.kernel_core.shape_forge().make_shape(EMPTY_SHAPE_ID, constructor_si);
            let prototype = unsafe { &mut *prototype_obj };
            prototype.set_shape_id(constructor_shape);
            let constructor_pos = prototype.push_prop(func_val);
            prototype.set_data_meta(constructor_pos, PropAttributes::new(true, false, true));
            prototype.bump_generation();

            let prototype_si = self.kernel_core.perm_interner().intern("prototype").0;
            let func = unsafe { &mut *obj_ptr };
            let prototype_shape = self.kernel_core.shape_forge().make_shape(func.shape_id(), prototype_si);
            func.set_shape_id(prototype_shape);
            func.ensure_hash_props().push(prototype_val);
            func.bump_generation();
        }

        func_val
    }

    pub(crate) fn error_text(&self, val: JsValue) -> String {
        if let Some(s) = self.lookup_str(val) {
            return s;
        }
        if val.is_object() {
            let obj = unsafe { &*val.as_js_object_ptr() };
            let name_si = self.kernel_core.perm_interner().intern("name").0;
            let message_si = self.kernel_core.perm_interner().intern("message").0;
            let name = self
                .resolve_property(obj, name_si)
                .and_then(|v| self.lookup_str(v))
                .unwrap_or_else(|| "Error".to_string());
            let message = self
                .resolve_property(obj, message_si)
                .and_then(|v| self.lookup_str(v))
                .unwrap_or_default();
            return crate::vm::format_error_message(&name, &message);
        }
        format!("{val}")
    }

    fn convert_constant(&self, constant: &Constant) -> JsValue {
        match constant {
            Constant::Number(v) => JsValue::float(*v),
            Constant::Int(v) => JsValue::int(*v),
            Constant::String(s) => self.perm_string(s),
            Constant::Boolean(b) => JsValue::bool(*b),
            Constant::Null => JsValue::null(),
            Constant::Undefined => JsValue::undefined(),
        }
    }

    /// 把模块的不可变常量池转换为 `JsValue`。不可能失败——CreateClosure/CreateRegExp
    /// 已把函数与正则移出常量池，池中只剩标量 + 永久 intern 字符串值。`&self`
    /// 使其可运行于 `OnceLock::get_or_init` 内部。
    pub(crate) fn convert_immutables(&self, constants: &[Constant]) -> Vec<JsValue> {
        constants.iter().map(|c| self.convert_constant(c)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global_prop(vm: &Vm, name: &str) -> JsValue {
        global_prop_opt(vm, name).expect("global slot should exist")
    }

    fn global_prop_opt(vm: &Vm, name: &str) -> Option<JsValue> {
        let global = vm.session.global_object();
        let si = vm.kernel_core.perm_interner().intern(name).0;
        vm.kernel_core
            .shape_forge()
            .lookup_position(global.shape_id(), si)
            .map(|pos| global.get_prop_at(pos))
    }

    fn run_source(vm: &mut Vm, source: &str) -> JsValue {
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, source).expect("parse failed");
        let module = oxide_compiler::compiler::Compiler::new()
            .compile(&program)
            .expect("compile failed");
        vm.run(&module).expect("vm run failed")
    }

    #[test]
    fn full_reset_clean_keeps_session_objects() {
        let mut vm = Vm::new();
        let world_ptr = Arc::as_ptr(&vm.session.builtin_world);
        let global_ptr = vm.session.global_object.as_ptr();
        let object_proto_ptr = vm.session.builtin_world().object_proto.as_ptr();

        vm.full_reset();

        assert!(std::ptr::eq(world_ptr, Arc::as_ptr(&vm.session.builtin_world)));
        assert!(std::ptr::eq(global_ptr, vm.session.global_object.as_ptr()));
        assert!(std::ptr::eq(object_proto_ptr, vm.session.builtin_world().object_proto.as_ptr()));
        assert!(!vm.session.is_dirty_since_snapshot());
    }

    #[test]
    fn full_reset_global_dirty_rebuilds_global_and_restores_slots() {
        let mut vm = Vm::new();
        let world_ptr = Arc::as_ptr(&vm.session.builtin_world);
        let global_ptr = vm.session.global_object.as_ptr();
        let global = unsafe { &mut *(vm.session.global_object.as_ptr() as *mut JsObject) };
        bindings::bind_global_value(&vm.kernel_core, global, "userGlobal", JsValue::int(99));
        unsafe { &mut *(vm.session.global_object.as_ptr() as *mut JsObject) }.bump_generation();

        vm.full_reset();

        assert!(std::ptr::eq(world_ptr, Arc::as_ptr(&vm.session.builtin_world)));
        assert!(!std::ptr::eq(global_ptr, vm.session.global_object.as_ptr()));
        assert!(global_prop_opt(&vm, "userGlobal").is_none());
        assert!(std::ptr::eq(
            global_prop(&vm, "Array").as_js_object_ptr(),
            vm.session.builtin_world().array_constructor.as_ptr() as *mut JsObject
        ));
        assert!(std::ptr::eq(
            global_prop(&vm, "globalThis").as_js_object_ptr(),
            vm.session.global_object.as_ptr() as *mut JsObject
        ));
        assert!(!vm.session.is_dirty_since_snapshot());
    }

    #[test]
    fn full_reset_dirty_builtin_rebinds_global_slot() {
        let mut vm = Vm::new();
        let old_object_proto = vm.session.builtin_world().object_proto.as_ptr();
        let old_array_proto = vm.session.builtin_world().array_proto.as_ptr();
        unsafe { &mut *(old_array_proto as *mut JsObject) }.bump_generation();

        vm.full_reset();

        assert!(std::ptr::eq(old_object_proto, vm.session.builtin_world().object_proto.as_ptr()));
        assert!(!std::ptr::eq(old_array_proto, vm.session.builtin_world().array_proto.as_ptr()));
        assert!(std::ptr::eq(
            global_prop(&vm, "Array").as_js_object_ptr(),
            vm.session.builtin_world().array_constructor.as_ptr() as *mut JsObject
        ));
        let constructor_si = vm.kernel_core.perm_interner().intern("constructor").0;
        let array_proto = &*vm.session.builtin_world().array_proto;
        let constructor = vm
            .resolve_property(array_proto, constructor_si)
            .expect("Array.prototype.constructor");
        assert!(std::ptr::eq(
            constructor.as_js_object_ptr(),
            vm.session.builtin_world().array_constructor.as_ptr() as *mut JsObject
        ));
        assert!(!vm.session.is_dirty_since_snapshot());
    }

    #[test]
    fn full_reset_dirty_function_keeps_call_working() {
        let mut vm = Vm::new();
        let function_proto = vm.session.builtin_world().function_proto.as_ptr();
        unsafe { &mut *(function_proto as *mut JsObject) }.bump_generation();

        vm.full_reset();

        let result = run_source(&mut vm, "Array.prototype.push.call([1], 2)");
        assert_eq!(result, JsValue::int(2));
        assert!(!vm.session.is_dirty_since_snapshot());
    }

    #[test]
    fn session_epoch_survives_reset() {
        let mut vm = Vm::new();
        let session_ptr = vm.gc_state.session_epoch.alloc(123i32) as *mut i32;

        vm.reset();

        assert!(unsafe { *session_ptr } == 123);
    }

    #[test]
    fn immutables_cache_filled_once_per_module() {
        let mut vm = Vm::new();
        // `f` 递归（同一子模块进入 4 次），其不可变常量经 OnceLock 只转换一次。
        let result = run_source(&mut vm, "function f(n){ if(n<=0){ return 'done'; } return f(n-1); } f(3)");
        assert!(result.is_string());
        assert_eq!(vm.lookup_str(result).as_deref(), Some("done"));
        // 缓存 = 顶层模块 + 1 个子模块（f）；子模块槽由这些调用初始化。
        assert_eq!(vm.immutables_cache.len(), 2);
        // 子模块常量改走 temp_immutables（避免缓存下标冲突）
    }

    #[test]
    fn session_epoch_reset_is_only_in_full_reset_state_clear() {
        let src = include_str!("vm_support.rs");
        let production = src.split("#[cfg(test)]").next().expect("production source");
        assert_eq!(production.matches("self.gc_state.session_epoch.reset()").count(), 1);
        assert!(production.contains("fn clear_full_reset_state(&mut self)"));
        assert!(production.contains("self.gc_state.session_epoch.reset();"));
    }

    #[test]
    fn full_reset_refreshes_object_prototype_after_object_dirty() {
        let mut vm = Vm::new();
        let old_object_proto = vm.session.builtin_world().object_proto.as_ptr();
        unsafe { &mut *(old_object_proto as *mut JsObject) }.bump_generation();

        vm.full_reset();

        assert!(!std::ptr::eq(old_object_proto, vm.session.builtin_world().object_proto.as_ptr()));
        assert!(std::ptr::eq(
            vm.object_prototype.as_ptr(),
            vm.session.builtin_world().object_proto.as_ptr()
        ));
        assert!(!vm.session.is_dirty_since_snapshot());
    }
}
