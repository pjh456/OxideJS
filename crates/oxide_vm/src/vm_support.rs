#![allow(clippy::arc_with_non_send_sync)]

use std::collections::VecDeque;
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
        let mut vm = Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Arc::default(),
            immutables_cache: Vec::new(),
            active_immutables: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            frames: smallvec::SmallVec::new(),
            kernel_core: core,
            session,
            epoch: Epoch::new(),
            object_prototype: obj_proto,
            generator_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            generator_function_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            promise_constructor: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            promise_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            aggregate_error_constructor: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            aggregate_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            async_function_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            async_generator_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            async_generator_function_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            job_queue: VecDeque::new(),
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
            pending_completion: None,
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            inline_args_base: 0,
            inline_args_count: 0,
            accessor_frame_target_reg: None,
            inline_callee: None,
            inline_reg_pool: None,
            generator_suspended: None,
            delegated_iterator: None,
            generator_dispatch: false,
            generator_init_step: false,
            generator_body_started: false,
            async_context: None,
            async_suspended: false,
            async_dispatch: false,
            async_gen_context: None,
            async_gen_dispatch: false,
            async_gen_suspended: false,
            gc_state: GcState {
                session_epoch: bumpalo::Bump::new(),
                session_gc: crate::session_gc::SessionGc::new(),
                epoch_object_ptrs: Vec::new(),
                session_object_ptrs: Vec::new(),
                session_string_ptrs: Vec::new(),
                session_bigint_ptrs: std::cell::RefCell::new(Vec::new()),
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
            cell_stack: Vec::new(),
        };
        vm.init_generator_intrinsics();
        vm.init_promise_intrinsics();
        vm.init_async_intrinsics();
        vm.init_async_generator_intrinsics();
        // Promise 全局绑定发生在快照采集之后，重录快照避免首次 full_reset 误判脏。
        vm.session.record_snapshot();
        vm_info!("Vm created");
        vm
    }

    /// 复用共享 `KernelCore` 创建 VM（VM 池路径），共享 intern/shape/code 缓存。
    pub fn with_kernel_core(core: Arc<KernelCore>) -> Self {
        let mut session = KernelSession::new(&core);
        bindings::init_kernel_builtins(&core, &mut session);
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        let mut vm = Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Arc::default(),
            immutables_cache: Vec::new(),
            active_immutables: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            frames: smallvec::SmallVec::new(),
            kernel_core: core,
            session,
            epoch: Epoch::new(),
            object_prototype: obj_proto,
            generator_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            generator_function_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            promise_constructor: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            promise_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            aggregate_error_constructor: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            aggregate_error_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            async_function_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            async_generator_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            async_generator_function_proto: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            job_queue: VecDeque::new(),
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
            pending_completion: None,
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            inline_args_base: 0,
            inline_args_count: 0,
            accessor_frame_target_reg: None,
            inline_callee: None,
            inline_reg_pool: None,
            generator_suspended: None,
            delegated_iterator: None,
            generator_dispatch: false,
            generator_init_step: false,
            generator_body_started: false,
            async_context: None,
            async_suspended: false,
            async_dispatch: false,
            async_gen_context: None,
            async_gen_dispatch: false,
            async_gen_suspended: false,
            gc_state: GcState {
                session_epoch: bumpalo::Bump::new(),
                session_gc: crate::session_gc::SessionGc::new(),
                epoch_object_ptrs: Vec::new(),
                session_object_ptrs: Vec::new(),
                session_string_ptrs: Vec::new(),
                session_bigint_ptrs: std::cell::RefCell::new(Vec::new()),
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
            cell_stack: Vec::new(),
        };
        vm.init_generator_intrinsics();
        vm.init_promise_intrinsics();
        vm.init_async_intrinsics();
        vm.init_async_generator_intrinsics();
        // Promise 全局绑定发生在快照采集之后，重录快照避免首次 full_reset 误判脏。
        vm.session.record_snapshot();
        vm_info!("Vm created (pool)");
        vm
    }

    /// 初始化/重建生成器内建对象（`%GeneratorPrototype%` 与 `%GeneratorFunction.prototype%`）。
    ///
    /// 在 VM 创建与 `full_reset`（session 重建）后调用——原型继承自 session 的
    /// Object/Function 原型，session 重建后须重挂。
    pub(crate) fn init_generator_intrinsics(&mut self) {
        crate::generator::init_generator_intrinsics(self);
    }

    /// 初始化/重建异步函数内建对象（`%AsyncFunction.prototype%`）。
    ///
    /// 在 VM 创建与 `full_reset`（session 重建）后调用。
    pub(crate) fn init_async_intrinsics(&mut self) {
        crate::async_func::init_async_intrinsics(self);
    }

    /// 初始化/重建异步生成器内建对象（`%AsyncGeneratorPrototype%` 与
    /// `%AsyncGeneratorFunction.prototype%`）。
    ///
    /// 在 VM 创建与 `full_reset`（session 重建）后调用。
    pub(crate) fn init_async_generator_intrinsics(&mut self) {
        crate::async_generator::init_async_generator_intrinsics(self);
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
        self.object_prototype = P::clone(&self.session.builtin_world().object_proto);
        self.init_generator_intrinsics();
        self.init_promise_intrinsics();
        self.init_async_intrinsics();
        self.init_async_generator_intrinsics();
        // 快照须在 Promise 全局绑定之后采集：绑定会修改 global 世代。
        self.session.record_snapshot();
        self.clear_full_reset_state();
        vm_info!("full_reset completed");
    }

    /// 旧版全量重置：总是丢弃并重建整个 session 与内置对象（benchmark 专用）。
    #[doc(hidden)]
    pub fn full_reset_legacy_for_bench(&mut self) {
        self.session = KernelSession::new(&self.kernel_core);
        bindings::init_kernel_builtins(&self.kernel_core, &mut self.session);
        self.object_prototype = P::clone(&self.session.builtin_world().object_proto);
        self.init_generator_intrinsics();
        self.init_promise_intrinsics();
        self.init_async_intrinsics();
        self.init_async_generator_intrinsics();
        self.session.record_snapshot();
        self.clear_full_reset_state();
    }

    fn clear_full_reset_state(&mut self) {
        self.clear_execution_state();
        self.bytecode = Arc::default();
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
        self.free_session_bigint_heap_data();
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
        self.pending_completion = None;
        self.generator_suspended = None;
        self.delegated_iterator = None;
        self.generator_dispatch = false;
        self.generator_init_step = false;
        self.generator_body_started = false;
        self.async_context = None;
        self.async_suspended = false;
        self.async_dispatch = false;
        self.async_gen_context = None;
        self.async_gen_dispatch = false;
        self.async_gen_suspended = false;
        self.native_call_depth = 0;
        // inline 窗口缓冲池内容为已废弃快照，跨 run/reset 不保留。
        self.inline_reg_pool = None;
        // 微任务队列是执行期状态：跨 run 不保留。
        self.job_queue.clear();
    }

    /// 轻量重置：清空执行状态并回收 epoch 内存，但保留 session 字符串与 builtin。
    pub fn reset(&mut self) {
        self.clear_execution_state();
        self.maybe_collect_session_gc();
        self.bytecode = Arc::default();
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
        // 生成器函数对象：原型为 %GeneratorFunction.prototype%（constructor 链解析到
        // "GeneratorFunction"），且不像普通函数那样拥有 `prototype` 属性。
        let is_generator = self.sub_modules.get(sub_idx as usize).map(|m| m.is_generator).unwrap_or(false);
        let is_async = self.sub_modules.get(sub_idx as usize).map(|m| m.is_async).unwrap_or(false);
        // 异步生成器（`async function*`）函数对象：原型为 %AsyncGeneratorFunction.prototype%。
        let is_async_generator = is_generator && is_async;
        let proto_val = if is_async_generator {
            JsValue::from_js_object(self.async_generator_function_proto.as_ptr() as *mut JsObject)
        } else if is_generator {
            JsValue::from_js_object(self.generator_function_proto.as_ptr() as *mut JsObject)
        } else if is_async {
            JsValue::from_js_object(self.async_function_proto.as_ptr() as *mut JsObject)
        } else {
            JsValue::from_js_object(self.session.builtin_world().function_proto.as_ptr() as *mut JsObject)
        };
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
            // 原型对象自身的 [[Prototype]]：生成器为 %GeneratorPrototype%，普通函数为 Object.prototype。
            let proto_of_proto = if is_async_generator {
                JsValue::from_js_object(self.async_generator_proto.as_ptr() as *mut JsObject)
            } else if is_generator {
                JsValue::from_js_object(self.generator_proto.as_ptr() as *mut JsObject)
            } else {
                JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject)
            };
            let prototype_obj = self.epoch.alloc(JsObject::new_empty(EMPTY_SHAPE_ID, proto_of_proto));
            self.gc_state.track_epoch_object(prototype_obj);
            let prototype_val = JsValue::from_js_object(prototype_obj);

            if !is_generator {
                // 普通函数：prototype 对象带 constructor 指回函数。
                let constructor_si = self.kernel_core.perm_interner().intern("constructor").0;
                let constructor_shape = self.kernel_core.shape_forge().make_shape(EMPTY_SHAPE_ID, constructor_si);
                let prototype = unsafe { &mut *prototype_obj };
                prototype.set_shape_id(constructor_shape);
                let constructor_pos = prototype.push_prop(func_val);
                prototype.set_data_meta(constructor_pos, PropAttributes::new(true, false, true));
                prototype.bump_generation();
            }

            // 函数自身 `prototype` 属性：生成器 writable:true / enumerable:false / configurable:false。
            let prototype_si = self.kernel_core.perm_interner().intern("prototype").0;
            let func = unsafe { &mut *obj_ptr };
            let prototype_shape = self.kernel_core.shape_forge().make_shape(func.shape_id(), prototype_si);
            func.set_shape_id(prototype_shape);
            let prototype_pos = func.push_prop(prototype_val);
            if is_generator || is_async_generator {
                func.set_data_meta(prototype_pos, PropAttributes::new(true, false, false));
            }
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

    /// 分配一个 BigInt 值：把 `i128` 堆分配为 box 并返回携带指针的 `JsValue`。
    ///
    /// box 指针登记进 `gc_state.session_bigint_ptrs`，在 `full_reset` 统一释放。
    /// `&self` 使 `convert_immutables`（常量池 → JsValue）也能分配。
    pub fn new_bigint(&self, v: num_bigint::BigInt) -> JsValue {
        let ptr = Box::into_raw(Box::new(v));
        self.gc_state.session_bigint_ptrs.borrow_mut().push(ptr);
        JsValue::bigint(ptr)
    }

    /// 读取 BigInt 值；调用方须保证 `val.is_bigint()`。
    pub fn bigint_value(&self, val: JsValue) -> &num_bigint::BigInt {
        // SAFETY: bigint 指针由 new_bigint 经 Box::into_raw 产生，存活至 full_reset。
        unsafe { &*val.as_bigint_ptr() }
    }

    /// 释放全部 session 堆 BigInt box。仅在完全隔离重置（`full_reset`）时调用，
    /// 此时没有存活的 session 对象/寄存器会引用它们。
    fn free_session_bigint_heap_data(&mut self) {
        for ptr in self.gc_state.session_bigint_ptrs.borrow_mut().drain(..) {
            // SAFETY: 每个指针来自 new_bigint 的 Box::into_raw(Box::new(BigInt))，
            // 且只在这里恰好释放一次。
            unsafe {
                drop(Box::from_raw(ptr));
            }
        }
    }

    fn convert_constant(&self, constant: &Constant) -> JsValue {
        match constant {
            Constant::Number(v) => JsValue::float(*v),
            Constant::Int(v) => JsValue::int(*v),
            Constant::BigInt(v) => self.new_bigint(v.clone()),
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

        // Function 原型家族重建后，未重建家族（array/string/object/map 等）的方法
        // wrapper 是跨重置存活对象，其原型链必须仍能解析出 call/apply/bind：
        // 任何一条路径失效都说明 wrapper 原型指向了已释放的旧 Function 原型。
        assert_eq!(run_source(&mut vm, "Array.prototype.push.call([1], 2)"), JsValue::int(2));
        assert_eq!(
            run_source(&mut vm, "Array.prototype.push.apply([], [1, 2, 3])"),
            JsValue::int(3)
        );
        let replaced = run_source(&mut vm, "String.prototype.replace.call('a', 'a', 'b')");
        assert_eq!(vm.lookup_str(replaced).as_deref(), Some("b"));
        let has = run_source(&mut vm, "Object.prototype.hasOwnProperty.call({x: 1}, 'x')");
        assert_eq!(has, JsValue::bool(true));
        let fixed = run_source(&mut vm, "Number.prototype.toFixed.call(1.5, 1)");
        assert_eq!(vm.lookup_str(fixed).as_deref(), Some("1.5"));
        let in_map = run_source(&mut vm, "Map.prototype.has.call(new Map([[1, 2]]), 1)");
        assert_eq!(in_map, JsValue::bool(true));
        let mapped = run_source(&mut vm, "Array.prototype.map.call([1, 2], function(x){ return x + 1; }).join(',')");
        assert_eq!(vm.lookup_str(mapped).as_deref(), Some("2,3"));
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
    fn dynamic_function_basic_arity() {
        let mut vm = Vm::new();
        // 与等价静态函数返回值逐位一致（引擎函数调用统一产 Double 数值）。
        let expected = run_source(&mut vm, "function f(a,b){return a+b} f(3,4)");
        let result = run_source(&mut vm, "new Function('a','b','return a+b')(3,4)");
        assert_eq!(result, expected);
    }

    #[test]
    fn dynamic_function_called_without_new() {
        let mut vm = Vm::new();
        let expected = run_source(&mut vm, "function f(a,b){return a*b} f(6,7)");
        let result = run_source(&mut vm, "Function('a','b','return a*b')(6,7)");
        assert_eq!(result, expected);
    }

    #[test]
    fn dynamic_function_empty_body_returns_undefined() {
        let mut vm = Vm::new();
        assert!(run_source(&mut vm, "Function()()").is_undefined());
    }

    #[test]
    fn dynamic_function_syntax_error_throws_syntax_error() {
        let mut vm = Vm::new();
        let result = run_source(&mut vm, "try{new Function('return {{')}catch(e){e.name}");
        assert!(result.is_string());
        assert_eq!(vm.lookup_str(result).as_deref(), Some("SyntaxError"));
    }

    #[test]
    fn dynamic_function_nested_closure_renumbering() {
        let mut vm = Vm::new();
        // 匿名函数体声明嵌套函数 g，返回值是引用 g 的闭包：验证子树 flat_id 重编号
        // 与 CREATE_CLOSURE imm16 重写后嵌套调用仍指向正确的子模块。
        let expected = run_source(
            &mut vm,
            "function outer(){var g=function(n){return n*2}; return function(){return g(21)}} outer()()",
        );
        let result = run_source(
            &mut vm,
            "new Function('var g=function(n){return n*2}; return function(){return g(21)}')()()",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn dynamic_function_multiple_in_one_run() {
        let mut vm = Vm::new();
        // 同一 run 内连续创建多个动态函数：验证 base 偏移累计正确。
        let expected = run_source(
            &mut vm,
            "function f1(){return 1} function f2(){return 2} function f3(a){return a*3} f1()+f2()+f3(4)",
        );
        let result = run_source(
            &mut vm,
            "new Function('return 1')() + new Function('return 2')() + new Function('a','return a*3')(4)",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn dynamic_function_name_and_length() {
        let mut vm = Vm::new();
        let name = run_source(&mut vm, "var f=new Function('a','b','return a'); f.name");
        assert!(name.is_string());
        assert_eq!(vm.lookup_str(name).as_deref(), Some("anonymous"));
        let len = run_source(&mut vm, "var f=new Function('a','b','return a'); f.length");
        assert_eq!(len, JsValue::int(2));
    }

    #[test]
    fn dynamic_function_comma_split_params_count() {
        let mut vm = Vm::new();
        // 单个实参 "a,b,c" 拼接解析为 3 个形参，length 应为解析后的形参数。
        let result = run_source(&mut vm, "new Function('a,b,c','null').length");
        assert_eq!(result, JsValue::int(3));
    }

    #[test]
    fn dynamic_function_name_and_length_attributes() {
        let mut vm = Vm::new();
        // length/name 为不可写、不可枚举、可配置的数据属性。
        let attrs = run_source(
            &mut vm,
            "var d=Object.getOwnPropertyDescriptor(new Function('a','return a'),'length'); String(d.value)+d.writable+d.enumerable+d.configurable",
        );
        assert_eq!(vm.lookup_str(attrs).as_deref(), Some("1falsefalsetrue"));
        let name_attrs = run_source(
            &mut vm,
            "var d=Object.getOwnPropertyDescriptor(Function(),'name'); String(d.writable)+d.enumerable+d.configurable",
        );
        assert_eq!(vm.lookup_str(name_attrs).as_deref(), Some("falsefalsetrue"));
    }

    #[test]
    fn dynamic_function_rethrows_to_string_exception() {
        let mut vm = Vm::new();
        // 形参 ToString 回调抛出的原始值须原样传播，而非包成 TypeError。
        let result = run_source(&mut vm, "try{new Function({toString:function(){throw 7}})}catch(e){e}");
        assert_eq!(result, JsValue::int(7));
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

    #[test]
    fn bigint_literal_arithmetic_and_comparison() {
        let mut vm = Vm::new();
        assert_eq!(run_source(&mut vm, "100n + 23n"), run_source(&mut vm, "123n"));
        assert_eq!(run_source(&mut vm, "100n - 30n"), run_source(&mut vm, "70n"));
        assert_eq!(run_source(&mut vm, "7n * 6n"), run_source(&mut vm, "42n"));
        assert_eq!(run_source(&mut vm, "10n / 4n"), run_source(&mut vm, "2n"));
        assert_eq!(run_source(&mut vm, "10n % 3n"), run_source(&mut vm, "1n"));
        assert_eq!(run_source(&mut vm, "-7n"), run_source(&mut vm, "0n - 7n"));
        assert_eq!(run_source(&mut vm, "123n == 123n"), JsValue::bool(true));
        assert_eq!(run_source(&mut vm, "123n === 123n"), JsValue::bool(true));
        assert_eq!(run_source(&mut vm, "5n < 3n"), JsValue::bool(false));
        assert_eq!(run_source(&mut vm, "5n > 3n"), JsValue::bool(true));
        assert_eq!(run_source(&mut vm, "1n === 1"), JsValue::bool(false));
        let typeof_result = run_source(&mut vm, "typeof 123n");
        assert!(typeof_result.is_string());
        assert_eq!(vm.lookup_str(typeof_result).as_deref(), Some("bigint"));
    }

    #[test]
    fn bigint_constructor_and_string() {
        let mut vm = Vm::new();
        assert_eq!(run_source(&mut vm, "BigInt(42)"), run_source(&mut vm, "42n"));
        assert_eq!(run_source(&mut vm, "BigInt('123')"), run_source(&mut vm, "123n"));
        assert_eq!(run_source(&mut vm, "BigInt('0x10')"), run_source(&mut vm, "16n"));
        let s = run_source(&mut vm, "String(123n)");
        assert!(s.is_string());
        assert_eq!(vm.lookup_str(s).as_deref(), Some("123"));
        let ts = run_source(&mut vm, "(123n).toString()");
        assert!(ts.is_string());
        assert_eq!(vm.lookup_str(ts).as_deref(), Some("123"));
        assert_eq!(run_source(&mut vm, "Number(5n)"), JsValue::int(5));
    }

    #[test]
    fn bigint_mixed_type_throws() {
        let mut vm = Vm::new();
        let te = run_source(&mut vm, "try { 1n + 1 } catch(e) { e.name }");
        assert_eq!(vm.lookup_str(te).as_deref(), Some("TypeError"));
        let re = run_source(&mut vm, "try { 1n / 0n } catch(e) { e.name }");
        assert_eq!(vm.lookup_str(re).as_deref(), Some("RangeError"));
        let ne = run_source(&mut vm, "try { new BigInt(1) } catch(e) { e.name }");
        assert_eq!(vm.lookup_str(ne).as_deref(), Some("TypeError"));
    }

    #[test]
    fn bigint_survives_reset_and_gc() {
        let mut vm = Vm::new();
        let result = run_source(&mut vm, "100n + 23n");
        assert!(result.is_bigint());
        assert_eq!(vm.bigint_value(result), &num_bigint::BigInt::from(123));
        vm.reset();
        // reset 保留 session 字符串/bigint box：值仍可读。
        assert_eq!(vm.bigint_value(result), &num_bigint::BigInt::from(123));
    }

    #[test]
    fn bigint_wrapped_and_number_comparison() {
        let mut vm = Vm::new();
        // 包装对象 coerce 后双 BigInt 运算。
        assert_eq!(run_source(&mut vm, "Object(2n) / 2n"), run_source(&mut vm, "1n"));
        assert_eq!(run_source(&mut vm, "Object(2n) * 3n"), run_source(&mut vm, "6n"));
        assert_eq!(run_source(&mut vm, "2n + Object(3n)"), run_source(&mut vm, "5n"));
        // BigInt 与 Number 精确关系比较（超出 f64 精度仍精确）。
        assert_eq!(run_source(&mut vm, "9007199254740993n > 9007199254740992"), JsValue::bool(true));
        assert_eq!(run_source(&mut vm, "9007199254740993n < 9007199254740994"), JsValue::bool(true));
        assert_eq!(run_source(&mut vm, "2n < 3"), JsValue::bool(true));
        assert_eq!(run_source(&mut vm, "3n >= 3"), JsValue::bool(true));
        // NaN 关系比较为 false。
        assert_eq!(run_source(&mut vm, "0n < NaN"), JsValue::bool(false));
        // 混合算术抛 TypeError。
        let te = run_source(&mut vm, "try { Object(1n) - 1 } catch(e) { e.name }");
        assert_eq!(vm.lookup_str(te).as_deref(), Some("TypeError"));
    }
}
