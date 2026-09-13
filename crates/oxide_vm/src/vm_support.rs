#![allow(clippy::arc_with_non_send_sync)]

use std::collections::VecDeque;
use std::sync::Arc;

use oxide_bytecode::module::Constant;

use crate::bindings;
use crate::vm::{TableGen, Vm};
use crate::vm_info;
use crate::vm_state::{GcState, IterState, ProfilingState, SymbolState};
use oxide_kernel::kernel::{KernelConfig, KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::{JsObject, JsString, PropAttributes};
use oxide_types::value::JsValue;

impl Vm {
    /// Cons（rope）节点保留的单元阈值：拼接总长（UTF-16 单元数）≤ 该值时
    /// 急切扁平。小链的"链接 + 首次消费扁平化"双重分配高于直接拷贝，直接
    /// 扁平更优；超过阈值后 O(n²) 拷贝成本超过节点开销，Cons 的 O(1) 链接
    /// 才值得。
    pub(crate) const CONS_FLATTEN_UNITS: usize = 128;

    /// 替换本 VM 独占的内建 P 对象：先恰好释放旧副本的属性区一次，
    /// 再让旧 Arc 归零（旧副本无 Drop 口径兜底，full_reset 重初始化
    /// 路径必须显式释放，否则逐测试累积）。
    pub(crate) fn swap_intrinsic_proto(slot: &mut P<JsObject>, new_obj: JsObject) {
        // SAFETY: 旧副本为本 VM 独占（其余引用均为本次替换前克隆），
        // 属性区仅此一处释放并置空（幂等）。
        unsafe {
            (&mut *slot.as_mut_ptr()).release_raw_heap();
        }
        *slot = P::new(new_obj);
    }

    /// 以最小配置创建独立 VM：新建 `KernelCore` + `KernelSession` 并初始化内置对象。
    pub fn new() -> Self {
        let core = KernelCore::new(KernelConfig::minimal());
        let mut session = KernelSession::new(&core);
        bindings::init_kernel_builtins(&core, &mut session);
        // 在 builtin 绑定之后取 id：绑定过程已 intern "length"，此处命中缓存得到
        // 非 0 的稳定 id（保持枚举层"id 0 哨兵"的既有约定，见 walk_own_keys）。
        let length_si = core.perm_interner().intern("length").0;
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        // 提前缓存执行期字符串 GC 初始水位（构造后 config 不再变化）。
        let gc_threshold = core.config().session_gc_threshold;
        let mut vm = Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Arc::default(),
            active_immutables: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            frames: smallvec::SmallVec::new(),
            kernel_core: core,
            session,
            length_si,
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
            // gen 0 预登记空表占位：函数对象恒在 run 内创建（彼时 current_gen
            // ≥ 1），gen 0 表只是首 run 前路径的占位，首 run 边界即被回收。
            tables: std::collections::HashMap::from([(
                0u32,
                Box::new(TableGen {
                    modules: Arc::new(Vec::new()),
                    immutables: Vec::new(),
                }),
            )]),
            current_gen: 0,
            saved_bytecode_stack: Vec::new(),
            saved_immutables_stack: Vec::new(),
            save_stack: Vec::new(),
            spill_stack: Vec::new(),
            native_overflow_base: 0,
            native_overflow_count: 0,
            try_stack: Vec::new(),
            exception_value: None,
            last_uncaught_value: None,
            pending_exception: None,
            pending_error_kind: None,
            pending_completion: None,
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            reentry_hops: 0,
            inline_args_base: 0,
            inline_args_count: 0,
            accessor_frame_target_reg: None,
            inline_callee: None,
            inline_strict: false,
            inline_frames_base: 0,
            top_level_strict: false,
            top_level_this: JsValue::undefined(),
            inline_reg_pool: None,
            generator_suspended: None,
            delegated_iterator: None,
            generator_dispatch: false,
            generator_init_step: false,
            generator_body_started: false,
            async_context: None,
            async_suspended: false,
            async_dispatch: false,
            construct_dispatch: false,
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
                session_cell_ptrs: std::cell::RefCell::new(Vec::new()),
                session_bytes_allocated: 0,
                session_bytes_peak: 0,
                run_alloc_peak: 0,
                string_gc_watermark: gc_threshold,
                gc_threshold_cached: gc_threshold,
                gc_watermark: gc_threshold,
                gc_gate_retry_alloc: 0,
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
            },
            profiling: ProfilingState {
                ic_hits: std::cell::Cell::new(0),
                ic_misses: std::cell::Cell::new(0),
                instruction_count: 0,
            },
            cell_stack: Vec::new(),
            template_objects: std::collections::HashMap::new(),
            active_flat_id: 0,
            active_table_gen: 0,
            saved_flat_id_stack: Vec::new(),
            saved_table_gen_stack: Vec::new(),
        };
        vm.init_generator_intrinsics();
        vm.init_promise_intrinsics();
        vm.init_async_intrinsics();
        vm.init_async_generator_intrinsics();
        // Promise 全局绑定发生在快照采集之后，重录快照避免首次 full_reset 误判脏。
        vm.session.record_snapshot();
        // 边界守卫计数：VM 完整构造后登记，与 `Drop for Vm` 的注销恰好配对。
        vm.kernel_core.note_vm_started();
        vm_info!("Vm created");
        vm
    }

    /// 复用共享 `KernelCore` 创建 VM（VM 池路径），共享 intern/shape/code 缓存。
    pub fn with_kernel_core(core: Arc<KernelCore>) -> Self {
        let mut session = KernelSession::new(&core);
        bindings::init_kernel_builtins(&core, &mut session);
        // 在 builtin 绑定之后取 id：绑定过程已 intern "length"，此处命中缓存得到
        // 非 0 的稳定 id（保持枚举层"id 0 哨兵"的既有约定，见 walk_own_keys）。
        let length_si = core.perm_interner().intern("length").0;
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        // 提前缓存执行期字符串 GC 初始水位（构造后 config 不再变化）。
        let gc_threshold = core.config().session_gc_threshold;
        let mut vm = Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Arc::default(),
            active_immutables: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            frames: smallvec::SmallVec::new(),
            kernel_core: core,
            session,
            length_si,
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
            // gen 0 预登记空表占位：函数对象恒在 run 内创建（彼时 current_gen
            // ≥ 1），gen 0 表只是首 run 前路径的占位，首 run 边界即被回收。
            tables: std::collections::HashMap::from([(
                0u32,
                Box::new(TableGen {
                    modules: Arc::new(Vec::new()),
                    immutables: Vec::new(),
                }),
            )]),
            current_gen: 0,
            saved_bytecode_stack: Vec::new(),
            saved_immutables_stack: Vec::new(),
            save_stack: Vec::new(),
            spill_stack: Vec::new(),
            native_overflow_base: 0,
            native_overflow_count: 0,
            try_stack: Vec::new(),
            exception_value: None,
            last_uncaught_value: None,
            pending_exception: None,
            pending_error_kind: None,
            pending_completion: None,
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            reentry_hops: 0,
            inline_args_base: 0,
            inline_args_count: 0,
            accessor_frame_target_reg: None,
            inline_callee: None,
            inline_strict: false,
            inline_frames_base: 0,
            top_level_strict: false,
            top_level_this: JsValue::undefined(),
            inline_reg_pool: None,
            generator_suspended: None,
            delegated_iterator: None,
            generator_dispatch: false,
            generator_init_step: false,
            generator_body_started: false,
            async_context: None,
            async_suspended: false,
            async_dispatch: false,
            construct_dispatch: false,
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
                session_cell_ptrs: std::cell::RefCell::new(Vec::new()),
                session_bytes_allocated: 0,
                session_bytes_peak: 0,
                run_alloc_peak: 0,
                string_gc_watermark: gc_threshold,
                gc_threshold_cached: gc_threshold,
                gc_watermark: gc_threshold,
                gc_gate_retry_alloc: 0,
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
            },
            profiling: ProfilingState {
                ic_hits: std::cell::Cell::new(0),
                ic_misses: std::cell::Cell::new(0),
                instruction_count: 0,
            },
            cell_stack: Vec::new(),
            template_objects: std::collections::HashMap::new(),
            active_flat_id: 0,
            active_table_gen: 0,
            saved_flat_id_stack: Vec::new(),
            saved_table_gen_stack: Vec::new(),
        };
        vm.init_generator_intrinsics();
        vm.init_promise_intrinsics();
        vm.init_async_intrinsics();
        vm.init_async_generator_intrinsics();
        // Promise 全局绑定发生在快照采集之后，重录快照避免首次 full_reset 误判脏。
        vm.session.record_snapshot();
        // 边界守卫计数：VM 完整构造后登记，与 `Drop for Vm` 的注销恰好配对。
        vm.kernel_core.note_vm_started();
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
        // session 对象只能来自用户写 + promote，跨 full_reset 若保留 global 会悬垂：
        // 覆盖既有 global 槽的写入不递增 generation（脏检测依赖 generation 对比），
        // 此处强制 bump 使带 session 对象时 global 必然重建。
        if !self.gc_state.session_object_ptrs.is_empty() {
            let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
            unsafe { &mut *global_ptr }.bump_generation();
        }
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
        // 模板对象缓存中的 JsValue 指向将被重建的 session 对象：全量重置后悬垂，
        // 必须随 session 一并清空。
        self.template_objects.clear();
        self.saved_flat_id_stack.clear();
        self.saved_table_gen_stack.clear();
        self.active_flat_id = 0;
        self.active_table_gen = 0;
        self.bytecode = Arc::default();
        // 注册表随 session 重建整体清空：session 对象（唯一按代际保活表者）全部
        // 消失，残留表无引用源。回到构造期同态：gen 0 空表占位 + current_gen = 0。
        self.tables.clear();
        self.current_gen = 0;
        self.tables.insert(
            0,
            Box::new(TableGen {
                modules: Arc::new(Vec::new()),
                immutables: Vec::new(),
            }),
        );
        self.active_immutables = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        self.teardown_session_heap_data();
        self.epoch.reset();
        self.gc_state.epoch_object_ptrs.clear();
        // 换新 Bump：旧 session arena 全量归还系统分配器（与 sweep 路径同构），
        // 容量不跨 full_reset 保留。
        self.gc_state.session_epoch = bumpalo::Bump::new();
        self.gc_state.session_bytes_allocated = 0;
        self.gc_state.session_bytes_peak = 0;
        self.gc_state.run_alloc_peak = 0;
        self.gc_state.string_gc_watermark = self.kernel_core.config().session_gc_threshold;
        // 两档收集水位与门控重扫锚同点复位（同式：阈值增量起算）。
        self.gc_state.gc_watermark = self.gc_state.gc_threshold_cached;
        self.gc_state.gc_gate_retry_alloc = 0;
        self.gc_state.session_gc = crate::session_gc::SessionGc::new();
        self.symbols.reset();
        self.root_reg_limit = 0;
        self.active_reg_limit = 0;
    }

    /// 释放全部 session 堆数据：epoch 对象堆数据 + session 对象堆数据 + upvalue
    /// 列表 + session 串 + BigInt box + upvalue cell box。
    ///
    /// 供 `full_reset` 与 `Drop` 共用——对象本体（bumpalo arena / epoch bump）由调用方
    /// 重置，本函数只释放手工管理的 Box 指针（属性向量、各原生盒、串、BigInt、cell）。
    /// 原生盒在 GC 搬移/晋升时已深拷贝为单所有权，此处恰好释放一次。
    pub(crate) fn teardown_session_heap_data(&mut self) {
        // upvalue 列表先于对象表清空释放：去重枚举依赖两份对象表尚存。
        self.free_session_upvalues();
        self.free_epoch_object_heap_data();
        for ptr in self.gc_state.session_object_ptrs.drain(..) {
            crate::session_gc::SessionGc::drop_object_heap_data(ptr, true);
        }
        self.free_session_string_heap_data();
        self.free_session_bigint_heap_data();
        self.gc_state.free_cells();
    }

    /// 集中释放 upvalue 列表（`Box<Vec<*mut Cell>>`）：原件与晋升克隆经
    /// `clone_for_session_epoch` 共享同一 Box 分配，逐对象路径释放会双放，
    /// 只在此处按指针去重后统一释放。须在两份对象表清空之前调用
    /// （枚举仍存对象完成去重），此时对象已死、Box 无其他读者。
    fn free_session_upvalues(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let ptrs = self
            .gc_state
            .epoch_object_ptrs
            .iter()
            .chain(self.gc_state.session_object_ptrs.iter());
        for &ptr in ptrs {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: ptr 来自 epoch/session 对象表登记，收尾时仍指向 arena 内合法对象。
            let up = unsafe { (*ptr).upvalues };
            if up.is_null() || !seen.insert(up) {
                continue;
            }
            // SAFETY: up 由 set_upvalues 的 Box::into_raw 分配，去重保证恰好释放一次。
            unsafe {
                drop(Box::from_raw(up as *mut Vec<*mut oxide_types::object::Cell>));
                (*ptr).upvalues = std::ptr::null_mut();
            }
        }
    }

    /// 释放 VM 内建原型 P 对象（生成器/Promise/异步族与 Object 原型）的堆外属性区。
    ///
    /// # 注意事项
    /// 对象本体随 Arc 引用归零释放（字段 drop）；属性区须在此显式释放一次
    /// （`JsObject` 无 Drop 口径）。仅释放本 VM 独占（强引用计数为 1）的副本：
    /// `object_prototype` 与 session world 共享 Arc，session 存活期下一测试仍经
    /// world 引用同一对象，其属性区归 session 收尾（`teardown_builtins`）释放。
    pub(crate) fn teardown_intrinsic_protos(&mut self) {
        for p in [
            &self.object_prototype,
            &self.generator_proto,
            &self.generator_function_proto,
            &self.promise_constructor,
            &self.promise_proto,
            &self.aggregate_error_constructor,
            &self.aggregate_error_proto,
            &self.async_function_proto,
            &self.async_generator_proto,
            &self.async_generator_function_proto,
        ] {
            if p.strong_count() != 1 {
                continue;
            }
            // SAFETY: 独占副本的属性区仅此一处释放并置空（幂等）。
            unsafe {
                (&mut *p.as_mut_ptr()).release_raw_heap();
            }
        }
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
        self.native_overflow_base = 0;
        self.native_overflow_count = 0;
        self.cell_stack.clear();
        self.try_stack.clear();
        self.exception_value = None;
        // 未捕获异常侧通道持原始 epoch 对象指针：执行期状态，跨 run/reset 不保留，
        // 池回收后残留将悬垂。
        self.last_uncaught_value = None;
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
        // 重入 hop 计数是执行期状态：分配上限按 run 起算，跨 run/reset 不保留。
        self.reentry_hops = 0;
        // inline 窗口缓冲池内容为已废弃快照，跨 run/reset 不保留。
        self.inline_reg_pool = None;
        // 微任务队列是执行期状态：跨 run 不保留。
        self.job_queue.clear();
    }

    /// 轻量重置：清空执行状态并回收 epoch 内存，但保留 session 字符串与 builtin。
    pub fn reset(&mut self) {
        self.clear_execution_state();
        self.maybe_collect_session_gc();
        // session 对象可持有 epoch 子引用（函数对象捕获、原生盒直插）：epoch
        // 重置前把 epoch 子引用原地克隆晋升进 session，避免悬垂指针。
        self.promote_session_epoch_refs();
        self.bytecode = Arc::default();
        // 表代际注册表不动：存活函数对象（含挂起帧 callee）按创建期代际仍须
        // 命中原表，跨 run 调用与恢复靠它成立。active_immutables 指向的旧表
        // 指针作废，下次 run 重装。
        self.active_immutables = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        self.free_epoch_dead_upvalues();
        self.free_epoch_object_heap_data();
        self.epoch.reset();
        self.gc_state.epoch_object_ptrs.clear();
        // 单 run 分配包络按 run 边界重起算（与 run_alloc_bytes 起算口径同源）。
        self.gc_state.run_alloc_peak = 0;
        // 两档收集水位与包络同起算：旧 run 的存活包络不延续到新 run 的触发判定。
        self.gc_state.gc_watermark = self.gc_state.gc_threshold_cached;
        self.gc_state.gc_gate_retry_alloc = 0;
        self.root_reg_limit = 0;
        self.active_reg_limit = 0;
    }

    /// 释放随本次 epoch 重置死亡的 epoch 对象的 upvalue 列表。
    ///
    /// 原件与晋升克隆共享同一 Box 分配：共享项归 session 克隆持有，
    /// 留待收尾（`free_session_upvalues`）统一释放，此处只放独占项。
    fn free_epoch_dead_upvalues(&mut self) {
        let mut shared: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &ptr in self.gc_state.session_object_ptrs.iter() {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: session 对象表此刻未清空，指向 session arena 内合法对象。
            let up = unsafe { (*ptr).upvalues } as usize;
            if up != 0 {
                shared.insert(up);
            }
        }
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &ptr in self.gc_state.epoch_object_ptrs.iter() {
            if ptr.is_null() {
                continue;
            }
            // SAFETY: epoch 对象表此刻未清空，指向 epoch arena 内合法对象。
            let up = unsafe { (*ptr).upvalues } as usize;
            if up == 0 || shared.contains(&up) || !seen.insert(up) {
                continue;
            }
            // SAFETY: up 由 set_upvalues 的 Box::into_raw 分配，去重与共享集
            // 保证本路径恰好释放一次。
            unsafe {
                drop(Box::from_raw(up as *mut Vec<*mut oxide_types::object::Cell>));
                (*ptr).upvalues = std::ptr::null_mut();
            }
        }
    }

    /// 分配一个可被 session GC 回收的字符串 `JsValue`（session-heap 字符串）。
    pub fn new_string(&mut self, s: &str) -> JsValue {
        self.new_string_owned(s.to_string())
    }

    /// 同 `new_string`，但以 move 接收 `String`，避免一次克隆。
    ///
    /// # 副作用
    /// - 累计 session 字符串字节账目；回收不在分配点触发（见注意事项）。
    ///
    /// # 注意事项
    /// - 分配点不触发 GC：builtin 函数栈上的局部 `JsValue` 与构造中对象不在
    ///   mark 根清单内，分配前触发会释放"仅存于局部/构造中"的活串（悬垂）。
    ///   回收统一在 dispatch 指令边界检查水位触发——此时 builtin 局部值已落地
    ///   为执行根，任何分配点的局部持有跨分配点均安全。
    /// - 热路径仅 1 次字节账目累加，无阈值读取与分支。
    pub fn new_string_owned(&mut self, s: String) -> JsValue {
        let len = s.len();
        let ptr = Box::into_raw(Box::new(JsString::new(s)));
        self.register_session_string(ptr, len)
    }

    /// 登记一个 session 字符串并记账（`new_string_owned` 与 `new_cons_string`
    /// 共用），返回字符串值。
    ///
    /// # 步骤
    /// 1. 登记指针到 session 字符串表。
    /// 2. 记账 `size_of::<JsString>() + bytes`。
    ///
    /// # 注意事项
    /// - 本方法**不触发**回收：执行期字符串 GC 统一在 dispatch 指令边界检查
    ///   水位触发（此时 builtin 局部值已落地为执行根，分配点触发会误释放
    ///   仅存于局部/构造中的活串）。
    fn register_session_string(&mut self, ptr: *mut JsString, bytes: usize) -> JsValue {
        self.gc_state.session_string_ptrs.push(ptr);
        self.gc_state.session_bytes_allocated += std::mem::size_of::<JsString>() + bytes;
        JsValue::string(ptr)
    }

    /// 分配 Cons（rope）节点：左右子节点 O(1) 链接，不拷贝文本。
    ///
    /// # 边界与前提
    /// - `left`/`right` 必须均为字符串值；仅在 `+`/`+=` 的 `concat_strings`
    ///   接线点调用（CONCAT_N 保持急切扁平，不经此路径）。
    ///
    /// # 副作用
    /// - 登记节点到 session 字符串表（账目 = `size_of::<JsString>() + payload_bytes`）。
    ///
    /// # 注意事项
    /// - 调用方须保证 `left`/`right` 在调用期间存活：子节点（coerce 新鲜字符串 /
    ///   新建叶子）要么已是执行根，要么在写入根前不被任何回收点触达——当前
    ///   回收点仅在指令边界，链接结果写寄存器先于下一次检查，天然满足。
    /// - 调用方（`concat_strings`）已按单元阈值过滤：仅大链（总长 >
    ///   [`Self::CONS_FLATTEN_UNITS`]）进入本方法，小链由其急切扁平。
    #[inline]
    pub fn new_cons_string(&mut self, left: JsValue, right: JsValue) -> JsValue {
        debug_assert!(left.is_string() && right.is_string(), "new_cons_string 只接收字符串操作数");
        let left_ptr = left.as_string_ptr_mut();
        let right_ptr = right.as_string_ptr_mut();
        // SAFETY: new_cons 的调用方（本函数）负责保证子节点随节点存活。
        let ptr = Box::into_raw(Box::new(unsafe { JsString::new_cons(left_ptr, right_ptr) }));
        // SAFETY: ptr 指向刚创建的存活节点，账目口径 = 单元数 × 2。
        let bytes = unsafe { (*ptr).payload_bytes() };
        self.register_session_string(ptr, bytes)
    }

    /// 以单元序列分配可被 session GC 回收的字符串值：智能路由（含孤立
    /// surrogate 落 FlatU16，否则 Flat），语义与副作用同 `new_string_owned`。
    pub fn new_string_units_owned(&mut self, units: Vec<u16>) -> JsValue {
        let ptr = Box::into_raw(Box::new(JsString::from_units(units)));
        // SAFETY: ptr 指向刚创建的存活 JsString。
        let bytes = unsafe { (*ptr).payload_bytes() };
        self.register_session_string(ptr, bytes)
    }

    /// 同 `new_string_units_owned`，以借用单元序列接收。
    pub fn new_string_units(&mut self, units: &[u16]) -> JsValue {
        self.new_string_units_owned(units.to_vec())
    }

    /// 单单元的属性值：ASCII 单元命中共享 perm 串（零分配、可指针短路），
    /// 非 ASCII（含孤立 surrogate）物化为 1 单元会话串。
    pub(crate) fn unit_char_value(&mut self, u: u16) -> JsValue {
        if let Some(v) = oxide_runtime_api::VmHost::single_unit(self, u) {
            return v;
        }
        self.new_string_units(&[u])
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
            // SAFETY: 每个指针来自 new_string/new_cons_string 的 Box::into_raw，
            // 且只在这里（或 sweep）恰好释放一次；内部连带释放 rope 扁平化产物。
            unsafe {
                crate::session_gc::SessionGc::drop_session_string_box(ptr);
            }
        }
    }

    /// 为 BytecodeFunc 常量创建函数 JsObject。
    /// 当 `is_arrow` 为 true 时，捕获当前 `this`（regs[254]），供调用时词法 this 绑定。
    ///
    /// # 边界与前提
    /// - `gen` 为函数对象所属表代际：`sub_idx` 的口径与对象头的 `table_gen`
    ///   盖写均以它为准（闭包创建传执行帧代际，动态构造传当前代际——动态扩表
    ///   追加进当前代际平表）。
    pub(crate) fn create_function_object(
        &mut self, sub_idx: u32, gen: u32, is_arrow: bool, is_class_constructor: bool, is_derived_constructor: bool,
        needs_home_object: bool,
    ) -> JsValue {
        // 生成器函数对象：原型为 %GeneratorFunction.prototype%（constructor 链解析到
        // "GeneratorFunction"），且不像普通函数那样拥有 `prototype` 属性。
        // 标志按函数对象所属代际平表解析：跨 run 调用时执行帧代际可异于当前代际，
        // sub_idx 口径与 gen 同域。
        let table = self.tables.get(&gen).expect("函数对象的代际表须在注册表中");
        let is_generator = table.modules.get(sub_idx as usize).map(|m| m.is_generator).unwrap_or(false);
        let is_async = table.modules.get(sub_idx as usize).map(|m| m.is_async).unwrap_or(false);
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
        // 记录创建期表代际：调用点按 (table_gen, sub_module_index) 解析子模块
        // 平表，跨 run 换表后存活函数仍命中原表（注册表按代际保活）。
        obj.set_table_gen(gen);
        obj.set_class_constructor(is_class_constructor);
        obj.set_derived_constructor(is_derived_constructor);
        let _ = needs_home_object;
        if is_arrow {
            obj.set_arrow(true);
            obj.set_captured_this(self.regs[254]);
        }
        // 函数对象直接 session 分配：寿命从调用级延至 session 级（session GC
        // mark/sweep 回收）。若按 epoch 分配，写入全局等逃逸根时 promote 屏障会
        // 深克隆进 session，全局属性与局部槽指针分裂、严格相等恒 false。
        obj.set_session_epoch(true);
        let obj_ptr = self.gc_state.session_epoch.alloc(obj) as *mut JsObject;
        self.gc_state.session_object_ptrs.push(obj_ptr);
        // 直 session 分配计入堆账目（与 promote 同式：对象头 + 对象堆数据）。
        self.gc_state.session_bytes_allocated += std::mem::size_of::<JsObject>()
            + crate::session_gc::SessionGc::object_heap_data_bytes(unsafe { &*obj_ptr }) as usize;
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
            // prototype 子对象与函数本体同走 session 分配：`f.prototype ===
            // globalThis.f.prototype` 要求两侧同一对象，epoch 分配会在逃逸写时
            // 被递归克隆出第二份。
            let mut prototype = JsObject::new_empty(EMPTY_SHAPE_ID, proto_of_proto);
            prototype.set_session_epoch(true);
            let prototype_obj = self.gc_state.session_epoch.alloc(prototype) as *mut JsObject;
            self.gc_state.session_object_ptrs.push(prototype_obj);
            // 直 session 分配计入堆账目（与 promote 同式：对象头 + 对象堆数据）。
            self.gc_state.session_bytes_allocated += std::mem::size_of::<JsObject>()
                + crate::session_gc::SessionGc::object_heap_data_bytes(unsafe { &*prototype_obj }) as usize;
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
        vm.run(&Arc::new(module)).expect("vm run failed")
    }

    /// 计数往返：两条构造路径（独立核/共享核）的登记与 Drop 注销恰好配对，
    /// 全部 drop 后计数归零，kernel 可干净 drop（Drop 断言无残留）。
    #[test]
    fn active_vms_count_roundtrip() {
        let vm = Vm::new();
        assert_eq!(vm.kernel_core.active_vms(), 1);
        drop(vm);

        let core = KernelCore::new(KernelConfig::minimal());
        assert_eq!(core.active_vms(), 0);
        let v1 = Vm::with_kernel_core(Arc::clone(&core));
        let v2 = Vm::with_kernel_core(Arc::clone(&core));
        let v3 = Vm::with_kernel_core(Arc::clone(&core));
        assert_eq!(core.active_vms(), 3);
        drop(v3);
        assert_eq!(core.active_vms(), 2);
        drop(v1);
        drop(v2);
        assert_eq!(core.active_vms(), 0);
        drop(core);
    }

    /// 守卫判别：持活 VM 调 sweep 触发 debug_assert。VM 声明在 kernel 之后，
    /// panic unwind 时先注销计数再 drop kernel，drop 断言不受干扰。
    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "no live VMs"))]
    fn sweep_runner_forges_rejects_live_vm() {
        let core = KernelCore::new(KernelConfig::minimal());
        let _vm = Vm::with_kernel_core(Arc::clone(&core));
        core.sweep_runner_forges();
    }

    #[test]
    fn full_reset_with_session_objects_forces_global_rebuild() {
        let mut vm = Vm::new();
        // 池路径场景：`globalThis.Array = {}` 覆盖既有 global 槽（不递增 generation），
        // 新值 `{}` 经 promote 进入 session——global 保留时该指针将悬垂。
        let _ = run_source(&mut vm, "globalThis.Array = {}; 0");
        assert!(!vm.gc_state.session_object_ptrs.is_empty(), "覆盖写应触发 promote 进入 session");
        let old_global = vm.session.global_object.as_ptr();

        vm.full_reset();

        // global 必须重建：旧 global 与其 session 对象随 epoch 释放，Array 恢复内置构造器。
        assert!(!std::ptr::eq(old_global, vm.session.global_object.as_ptr()));
        assert!(std::ptr::eq(
            global_prop(&vm, "Array").as_js_object_ptr(),
            vm.session.builtin_world().array_constructor.as_ptr() as *mut JsObject
        ));
        assert!(!vm.session.is_dirty_since_snapshot());
    }

    /// P 原型上方法槽的 wrapper 对象原始指针（rebuild 跨轮复用验证用）。
    fn method_wrapper_ptr(vm: &Vm, proto_ptr: *const JsObject, name: &str) -> *const JsObject {
        let si = vm.kernel_core.perm_interner().intern(name).0;
        // SAFETY: proto_ptr 是本 session 的 P 原型，安全点内无并发读者。
        let proto = unsafe { &*proto_ptr };
        let pos = vm
            .kernel_core
            .shape_forge()
            .lookup_position(proto.shape_id(), si)
            .unwrap_or_else(|| panic!("{name} 槽位应存在"));
        let val = proto.get_prop_at(pos);
        assert!(val.is_object(), "{name} 槽位应为 wrapper 对象: {val:?}");
        val.as_js_object_ptr()
    }

    /// global 属性槽数（槽位原位更新跨轮不追加验证用）。
    fn global_slot_count(vm: &Vm) -> usize {
        let g = vm.session.global_object.as_ptr() as *mut JsObject;
        // SAFETY: global 是本 session 对象，安全点内无并发读者。
        unsafe { (*g).prop_vec_len() }
    }

    /// 选择性重建 wrapper 复用与 global 槽原位更新：每轮「原型脏写 + full_reset」
    /// 后存活方法 wrapper 应为同一对象（同 raw 指针、同 shape），释放表计数与
    /// global 属性槽数跨轮不增长，重建后方法行为正确。
    #[test]
    fn full_reset_rebuild_reuses_method_wrappers_across_rounds() {
        let mut vm = Vm::new();
        let _ = run_source(&mut vm, "0");
        let push_before = method_wrapper_ptr(&vm, vm.session.builtin_world().array_proto.as_ptr(), "push");
        let push_shape_before = unsafe { &*push_before }.shape_id();
        let registry_before = vm.session.builtin_world().leaked_object_count();
        let slots_before = global_slot_count(&vm);

        for i in 0..3u32 {
            // 新键写才 bump 原型世代；四家族逐轮全脏，function 家族重建同时
            // 覆盖 wrapper proto 槽重指路径。
            let source = format!(
                "Object.prototype['w{i}'] = 1; Array.prototype['w{i}'] = 2; String.prototype['w{i}'] = 3; \
                 Function.prototype['w{i}'] = 4; 0"
            );
            let _ = run_source(&mut vm, &source);
            vm.full_reset();
        }

        let push_after = method_wrapper_ptr(&vm, vm.session.builtin_world().array_proto.as_ptr(), "push");
        assert!(std::ptr::eq(push_before, push_after), "存活方法 wrapper 应复用（同 raw 指针）");
        assert_eq!(push_shape_before, unsafe { &*push_after }.shape_id(), "wrapper shape 应稳定");
        let registry_after = vm.session.builtin_world().leaked_object_count();
        assert!(
            registry_after <= registry_before,
            "释放表计数跨轮不应增长: {registry_before} -> {registry_after}"
        );
        let slots_after = global_slot_count(&vm);
        assert!(slots_after <= slots_before, "global 槽数跨轮不应增长: {slots_before} -> {slots_after}");
        assert_eq!(run_source(&mut vm, "[1, 2].push(3)"), JsValue::int(3));
        assert_eq!(run_source(&mut vm, "String.prototype.charCodeAt.call('A', 0)"), JsValue::int(65));
    }

    /// global 槽位原位更新锚点：错误/资源栈家族脏重建（子类型构造器经 Box 自建
    /// 路径）多轮后 global 属性槽数不增长、Error 槽指向新构造器。
    #[test]
    fn full_reset_rebuild_keeps_global_slot_count_flat() {
        let mut vm = Vm::new();
        let _ = run_source(&mut vm, "0");
        let slots_before = global_slot_count(&vm);

        for i in 0..3u32 {
            // 新键写脏错误家族（子类型原型重建触发构造器 Box 路径）与对象家族。
            let source = format!(
                "Error.prototype['e{i}'] = 1; TypeError.prototype['e{i}'] = 2; Object.prototype['e{i}'] = 3; 0"
            );
            let _ = run_source(&mut vm, &source);
            vm.full_reset();
        }

        let slots_after = global_slot_count(&vm);
        assert!(slots_after <= slots_before, "global 槽数跨轮不应增长: {slots_before} -> {slots_after}");
        // Error 槽应指向本轮重建的构造器（非滞留旧指针）。
        assert!(std::ptr::eq(
            global_prop(&vm, "Error").as_js_object_ptr(),
            vm.session.builtin_world().error_constructor.as_ptr() as *mut JsObject
        ));
        assert!(global_prop(&vm, "TypeError").is_object());
        assert_eq!(run_source(&mut vm, "new TypeError('x') instanceof TypeError"), JsValue::bool(true));
    }

    fn vm_with_low_threshold() -> Vm {
        let mut cfg = KernelConfig::minimal();
        cfg.set_session_gc_threshold(1);
        Vm::with_kernel_core(KernelCore::new(cfg))
    }

    /// 单 run 分配上限拦截失控分配：死循环持续 push 的 run 须在触达步数上限前
    /// 以 memory limit 错误终止。
    #[test]
    fn run_alloc_cap_stops_runaway_allocation() {
        let mut cfg = KernelConfig::minimal();
        cfg.max_alloc_bytes = Some(256 * 1024);
        let mut vm = Vm::with_kernel_core(KernelCore::new(cfg));
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, "var a = []; while (true) { a.push(1); }").expect("parse failed");
        let module = oxide_compiler::compiler::Compiler::new()
            .compile(&program)
            .expect("compile failed");
        let err = vm
            .run(&Arc::new(module))
            .expect_err("runaway allocation must hit the alloc cap");
        assert!(err.contains("memory limit"), "unexpected error: {err}");
        assert!(!err.contains("step limit"), "cap 应先于步数上限生效: {err}");
    }

    /// 上限不误伤正常规模分配：同配置下 1 万元素数组构造正常完成。
    #[test]
    fn run_alloc_cap_allows_normal_allocation() {
        let mut cfg = KernelConfig::minimal();
        cfg.max_alloc_bytes = Some(4 * 1024 * 1024);
        let mut vm = Vm::with_kernel_core(KernelCore::new(cfg));
        let result = run_source(&mut vm, "var a = []; for (var i = 0; i < 10000; i++) { a.push(i); } a.length");
        assert!(result.is_int(), "expected int length, got {result:?}");
        assert_eq!(result.as_int(), 10000);
    }

    /// native 终端循环泵送小 JS 重入：每次重入 dispatch 远短于循环内 64 指令采样点，
    /// 顶层 steps 不推进——重入边界每 64 hop 强制采样兜底，失控分配以 memory limit
    /// 终止而非无限循环。
    #[test]
    fn reentry_pump_hits_alloc_cap() {
        let mut cfg = KernelConfig::minimal();
        cfg.max_alloc_bytes = Some(256 * 1024);
        let mut vm = Vm::with_kernel_core(KernelCore::new(cfg));
        // 永不 done 的生成器经鸭子对象交给 toArray：native 循环每步泵送两个短重入
        // （闭包调用 + 生成器恢复）并新产一个结果对象，分配无界增长。
        let source = "var g = (function* () { for (var i = 0; ; ++i) { yield i; } })(); \
                      var obj = { next: function () { return g.next(); } }; \
                      Iterator.from(obj).toArray();";
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, source).expect("parse failed");
        let module = oxide_compiler::compiler::Compiler::new()
            .compile(&program)
            .expect("compile failed");
        let err = vm.run(&Arc::new(module)).expect_err("reentry pump must hit the alloc cap");
        assert!(err.contains("memory limit"), "unexpected error: {err}");
    }

    /// 直接恢复生成器一步：等价 `it.next()`——回归聚焦 GC 搬移后的状态盒
    /// 有效性（同 run 内恢复，不经 run 边界换表）。
    fn resume_one_step(vm: &mut Vm, gen: JsValue) -> JsValue {
        match vm.resume_generator(gen, crate::generator::GeneratorResumeMode::Next(JsValue::undefined())) {
            Ok(crate::generator::GeneratorStep::Suspended { value }) => value,
            Ok(crate::generator::GeneratorStep::Completed { value }) => value,
            Ok(other) => panic!(
                "unexpected step: {:?}",
                match other {
                    crate::generator::GeneratorStep::Thrown { value } => format!("Thrown({value})"),
                    crate::generator::GeneratorStep::SuspendedRaw { value } => format!("SuspendedRaw({value})"),
                    _ => String::new(),
                }
            ),
            Err(e) => panic!("resume failed: {e}"),
        }
    }

    #[test]
    fn generator_survives_object_sweep_and_resumes() {
        let mut vm = vm_with_low_threshold();
        let _ = run_source(&mut vm, "function* g(){ yield 1; yield 2; } globalThis.it = g(); globalThis.it.next(); 0");

        // 直接触发完整收集（保留执行上下文）：存活生成器克隆进新 arena，
        // 状态盒深拷贝为新 Box（走收集入口而非 run 边界：聚焦 GC 搬移本身）。
        vm.maybe_collect_session_gc();
        assert!(vm.session_gc_stats().total_collections > 0, "应触发对象收集");

        // 从 global 取 sweep 重写后的生成器（同 run，模块表未换发，可恢复）。
        let it = global_prop(&vm, "it");
        assert_eq!(resume_one_step(&mut vm, it), JsValue::int(2), "sweep 后应恢复第二次 yield");
    }

    #[test]
    fn generator_captured_upvalue_survives_sweep() {
        let mut vm = vm_with_low_threshold();
        // x 函数作用域局部：被生成器 g 经 cell 捕获（顶层 var 直连全局属性不走 cell）。
        let _ = run_source(
            &mut vm,
            "function outer() { var x = 0; function* g(){ x++; yield x; x++; yield x; } globalThis.it = g(); globalThis.it.next(); return 0; } outer(); 0",
        );

        vm.maybe_collect_session_gc();
        assert!(vm.session_gc_stats().total_collections > 0, "应触发对象收集");

        // 挂起帧 cell_stack 与闭包 upvalues 中的 cell 独立堆分配（地址稳定），
        // 恢复后继续读写捕获变量。
        let it = global_prop(&vm, "it");
        assert_eq!(resume_one_step(&mut vm, it), JsValue::int(2), "sweep 后应恢复捕获变量读写");
    }

    #[test]
    fn generator_promoted_clone_owns_independent_state_box() {
        let mut vm = Vm::new();
        // `(function(){ var it = g(); it.next(); return it; })()`：it 为函数局部（非顶层
        // var，不经全局属性逃逸），保持 epoch 生成器对象（未 promote）。
        let it = run_source(
            &mut vm,
            "function* g(){ yield 1; yield 2; } (function(){ var it = g(); it.next(); return it; })()",
        );
        assert!(it.is_object());
        let epoch_ptr = it.as_js_object_ptr();
        let epoch_box = unsafe { (*epoch_ptr).native_data() };

        // 手动 promote：克隆应深拷贝状态盒（新 Box），与源盒互不共享。
        let promoted = vm.promote_object(epoch_ptr);
        assert!(!std::ptr::eq(promoted, epoch_ptr));
        let promoted_box = unsafe { (*promoted).native_data() };
        assert!(!std::ptr::eq(epoch_box, promoted_box), "promote 应深拷贝生成器状态盒");

        // 模拟 full_reset 的 epoch 侧释放：源对象与其状态盒随 epoch 回收，
        // 并从追踪表移除登记（克隆的后续回收仍由 VM 统一处理）。
        let _ = crate::session_gc::SessionGc::drop_object_heap_data(epoch_ptr, false);
        vm.gc_state.epoch_object_ptrs.retain(|&p| !std::ptr::eq(p, epoch_ptr));

        // 克隆直接恢复执行：读新盒中的挂起状态，不得悬垂。
        assert_eq!(resume_one_step(&mut vm, JsValue::from_js_object(promoted)), JsValue::int(2));
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
        assert_eq!(run_source(&mut vm, "Array.prototype.push.apply([], [1, 2, 3])"), JsValue::int(3));
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

    /// Function 家族脏重建：未重建家族的方法 wrapper 是跨重置存活对象，其 proto
    /// 槽（绑定时固化为旧 fn_proto）必须被重指到新 fn_proto，旧原型链不可再读。
    #[test]
    fn full_reset_dirty_function_repoints_retained_wrapper_proto() {
        let mut vm = Vm::new();
        let old_fn_proto = vm.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
        // 保留方法 wrapper：array 家族不重建，wrapper 对象与其 proto 槽跨重置存活。
        let array_proto = vm.session.builtin_world().array_proto.as_ptr() as *mut JsObject;
        let push_si = vm.kernel_core.perm_interner().intern("push").0;
        let push = vm
            .resolve_property(unsafe { &*array_proto }, push_si)
            .expect("Array.prototype.push");
        let push_ptr = push.as_js_object_ptr();
        assert!(std::ptr::eq(unsafe { (*push_ptr).proto().as_js_object_ptr() }, old_fn_proto));

        unsafe { &mut *old_fn_proto }.bump_generation();

        vm.full_reset();

        let new_fn_proto = vm.session.builtin_world().function_proto.as_ptr() as *mut JsObject;
        assert!(!std::ptr::eq(new_fn_proto, old_fn_proto));
        // 保留 wrapper proto 槽已重指新 fn_proto，call 链走新原型。
        assert!(std::ptr::eq(unsafe { (*push_ptr).proto().as_js_object_ptr() }, new_fn_proto));
        assert_eq!(run_source(&mut vm, "Array.prototype.push.call([1], 2)"), JsValue::int(2));
        assert!(!vm.session.is_dirty_since_snapshot());
    }

    /// 新键写脏 object/array/string/function 四家族（S2 脏源形态）：full_reset 后
    /// 跨家族读语义保持——保留 wrapper 原型链经重指后取 call、同家族
    /// constructor/prototype 对自洽、保留原型链指向新 Object.prototype。
    #[test]
    fn full_reset_dirty_four_families_cross_family_reads() {
        let mut vm = Vm::new();
        run_source(
            &mut vm,
            "Object.prototype['d'] = 1; Array.prototype['d'] = 2; String.prototype['d'] = 3; Function.prototype['d'] = 4;",
        );
        assert!(vm.session.is_dirty_since_snapshot());

        vm.full_reset();

        assert_eq!(run_source(&mut vm, "Array.prototype.push.call([1], 2)"), JsValue::int(2));
        let replaced = run_source(&mut vm, "String.prototype.replace.call('a', 'a', 'b')");
        assert_eq!(vm.lookup_str(replaced).as_deref(), Some("b"));
        let has = run_source(&mut vm, "Object.prototype.hasOwnProperty.call({x: 1}, 'x')");
        assert_eq!(has, JsValue::bool(true));
        assert_eq!(
            run_source(&mut vm, "Object.getPrototypeOf(Array.prototype) === Object.prototype"),
            JsValue::bool(true)
        );
        assert_eq!(run_source(&mut vm, "Array.prototype.constructor === Array"), JsValue::bool(true));
        assert!(!vm.session.is_dirty_since_snapshot());
    }

    #[test]
    fn session_epoch_survives_reset() {
        let mut vm = Vm::new();
        let session_ptr = vm.gc_state.session_epoch.alloc(123i32) as *mut i32;

        vm.reset();

        assert!(unsafe { *session_ptr } == 123);
    }

    /// 未捕获异常侧通道是执行期状态：原生错误后可能残留原值，reset/full_reset
    /// 边界须随执行状态一并清空——否则其持有的 epoch 对象指针在池回收后悬垂
    /// （后续原生错误经 raise_call_error 消费残留值即 UAF）。
    #[test]
    fn reset_drops_stale_uncaught_value() {
        let mut vm = Vm::new();
        let _ = run_source(&mut vm, "0");
        vm.last_uncaught_value = Some(JsValue::float(42.0));
        vm.reset();
        assert!(vm.last_uncaught_value.is_none(), "reset 应清空未捕获异常侧通道");

        vm.last_uncaught_value = Some(JsValue::float(42.0));
        vm.full_reset();
        assert!(vm.last_uncaught_value.is_none(), "full_reset 应清空未捕获异常侧通道");
    }

    /// run 边界换新 Bump 后双 arena 保留锚恰 0：重源 run 冲高水位，
    /// full_reset 后 epoch/session 两 arena 均空（容量不跨 reset 保留）。
    #[test]
    fn full_reset_zeroes_arena_retained() {
        let mut vm = Vm::new();
        run_source(
            &mut vm,
            "var t = 0; for (var i = 0; i < 20000; i++) { var o = { s: 'ab' + i, a: [i] }; t += o.s.length + o.a.length; } t",
        );
        assert!(vm.epoch.bump().allocated_bytes() > 0, "重源 run 应冲高 epoch arena 水位");

        vm.full_reset();

        assert_eq!(vm.epoch.bump().allocated_bytes(), 0);
        assert_eq!(vm.gc_state.session_epoch.allocated_bytes(), 0);
    }

    /// 直 session 分配站（函数对象 + prototype 子对象）计入 session 堆账目：
    /// 分配点计数 = 对象头（属性区容量扩张属 promote 同口径的既有盲区，不钉）。
    /// 另含 CREATE_CLOSURE 站将推断函数名物化为 session 串（name 数据属性，
    /// 既有行为），钉值一并计入。
    #[test]
    fn direct_session_alloc_counted_in_session_bytes() {
        let mut vm = Vm::new();
        let base = vm.session_bytes_allocated();
        run_source(&mut vm, "var f = function(){}; 0");
        let delta = vm.session_bytes_allocated() - base;
        let name_len = "f".len();
        assert_eq!(delta, 2 * std::mem::size_of::<JsObject>() + std::mem::size_of::<JsString>() + name_len);
    }

    /// 标签模板 cooked/raw 数组直 session 分配计入 session 堆账目：
    /// 含模板的 run 账目增量须超出同等函数对象口径（两数组各含对象头 + 元素区）。
    /// 单 run 内完成（标签函数定义 + 模板调用）：账目口径与函数对象钉同 run 对齐。
    #[test]
    fn tagged_template_object_counted_in_session_bytes() {
        let mut vm = Vm::new();
        let base = vm.session_bytes_allocated();
        // 对照 run：仅函数对象（本体 + prototype 子对象）。
        run_source(&mut vm, "var tag = function(){ return 0; }; 0");
        let fn_cost = vm.session_bytes_allocated() - base;
        // 实验 run：同函数 + 标签模板（cookeD/raw 两数组）。
        let before = vm.session_bytes_allocated();
        run_source(&mut vm, "var tag2 = function(){ return 0; }; tag2`a${1}b`; 0");
        let both_cost = vm.session_bytes_allocated() - before;
        assert!(
            both_cost > fn_cost + 2 * std::mem::size_of::<JsObject>(),
            "模板数组未计入账目: both={both_cost} fn={fn_cost}"
        );
    }

    #[test]
    fn immutables_filled_once_per_module() {
        let mut vm = Vm::new();
        // `f` 递归（同一子模块进入 4 次），其不可变常量经 OnceLock 只转换一次。
        let result = run_source(&mut vm, "function f(n){ if(n<=0){ return 'done'; } return f(n-1); } f(3)");
        assert!(result.is_string());
        assert_eq!(vm.lookup_str(result).as_deref(), Some("done"));
        // 缓存 = 顶层模块 + 1 个子模块（f）；子模块槽由这些调用初始化。
        assert_eq!(vm.current_table().immutables.len(), 2);
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
    fn session_epoch_replacement_is_only_in_full_reset_state_clear() {
        let src = include_str!("vm_support.rs");
        let production = src.split("#[cfg(test)]").next().expect("production source");
        assert_eq!(production.matches("self.gc_state.session_epoch = bumpalo::Bump::new()").count(), 1);
        assert!(production.contains("fn clear_full_reset_state(&mut self)"));
        assert!(production.contains("self.gc_state.session_epoch = bumpalo::Bump::new();"));
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
    fn full_reset_object_dirty_rebinds_iterator_family() {
        let mut vm = Vm::new();
        let old_object_proto = vm.session.builtin_world().object_proto.as_ptr();
        let old_iterator_proto = vm.session.builtin_world().iterator_proto.as_ptr();
        // 用户修改 Object.prototype：object 家族世代递增，global 未动。
        unsafe { &mut *(old_object_proto as *mut JsObject) }.bump_generation();

        vm.full_reset();

        // object 家族与迭代器原型全部重建（新原型链到新 Object.prototype）。
        assert!(!std::ptr::eq(old_object_proto, vm.session.builtin_world().object_proto.as_ptr()));
        assert!(!std::ptr::eq(old_iterator_proto, vm.session.builtin_world().iterator_proto.as_ptr()));
        // global 保留（dirty.global=false）：其 Iterator 函数对象的 prototype
        // 属性须对齐到重建后的 %IteratorPrototype%。
        let iter_val = global_prop(&vm, "Iterator");
        let si_prototype = vm.kernel_core.perm_interner().intern("prototype").0;
        let iter_obj = unsafe { &*iter_val.as_js_object_ptr() };
        let proto_pos = vm
            .kernel_core
            .shape_forge()
            .lookup_position(iter_obj.shape_id(), si_prototype)
            .expect("Iterator should have prototype slot");
        assert!(std::ptr::eq(
            iter_obj.get_prop_at(proto_pos).as_js_object_ptr(),
            vm.session.builtin_world().iterator_proto.as_ptr() as *mut JsObject
        ));
        // full_reset 后 session 干净；此后 run_source 执行才重新累积世代变化。
        assert!(!vm.session.is_dirty_since_snapshot());
        // 迭代器家族功能完整：原型 next 就位，for-of/spread/Array.from/Iterator.from/
        // Map/Set/String/yield* 全部可用，原型链与 Iterator.prototype 一致。
        let r = run_source(&mut vm, "[...[1,2,3]].join(',')");
        assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2,3"));
        let r = run_source(&mut vm, "Array.from([1,2]).join(',')");
        assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2"));
        let r = run_source(&mut vm, "Iterator.from({next:function(){return {value:42,done:false}}}).next().value");
        assert_eq!(r, JsValue::int(42));
        let r = run_source(&mut vm, "[...new Map([[1,2],[3,4]])].map(function(x){return x.join(':')}).join(';')");
        assert_eq!(vm.lookup_str(r).as_deref(), Some("1:2;3:4"));
        let r = run_source(&mut vm, "[...new Set([1,2,3])].join(',')");
        assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2,3"));
        let r = run_source(&mut vm, "[...'ab'].join(',')");
        assert_eq!(vm.lookup_str(r).as_deref(), Some("a,b"));
        let r = run_source(&mut vm, "function* g(){yield* [1,2]} [...g()].join(',')");
        assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2"));
        let r = run_source(
            &mut vm,
            "Object.getPrototypeOf(Object.getPrototypeOf([].values())) === Iterator.prototype",
        );
        assert_eq!(r, JsValue::bool(true));
        let r = run_source(&mut vm, "var it=[].values(); it[Symbol.iterator]()===it");
        assert_eq!(r, JsValue::bool(true));
    }

    #[test]
    fn full_reset_global_dirty_keeps_iterator_proto_slots_stable() {
        let mut vm = Vm::new();
        let arr_iter_proto = vm.session.builtin_world().array_iterator_proto.as_ptr() as *mut JsObject;
        let slots_before = unsafe { &*arr_iter_proto }.hash_props_vec().map_or(0, |v| v.len());
        // global 世代递增（用户写 global），builtin 家族未动。
        unsafe { &mut *(vm.session.global_object.as_ptr() as *mut JsObject) }.bump_generation();

        vm.full_reset();

        // builtin 未脏 → 迭代器原型保留原对象且属性槽不膨胀（重复 full_reset 不再追加）。
        let arr_iter_proto_after = vm.session.builtin_world().array_iterator_proto.as_ptr() as *mut JsObject;
        assert!(std::ptr::eq(arr_iter_proto, arr_iter_proto_after));
        let slots_after = unsafe { &*arr_iter_proto_after }.hash_props_vec().map_or(0, |v| v.len());
        assert_eq!(slots_before, slots_after);
        assert!(!vm.session.is_dirty_since_snapshot());
        // 迭代器功能经保留原型仍完整（run_source 起再次累积世代变化）。
        let r = run_source(&mut vm, "[...[1,2,3]].join(',')");
        assert_eq!(vm.lookup_str(r).as_deref(), Some("1,2,3"));
        let r = run_source(&mut vm, "var it=new Set([1]).values(); it[Symbol.iterator]()===it");
        assert_eq!(r, JsValue::bool(true));
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
