#![allow(clippy::arc_with_non_send_sync)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;

use oxide_bytecode::module::Constant;

use crate::bindings;
use crate::realm::Realm;
use crate::vm::{TableGen, Vm};
use crate::vm_info;
use crate::vm_state::{GcState, IterState, ProfilingState, SampleState, SymbolState};
use oxide_kernel::kernel::{KernelConfig, KernelCore, KernelSession};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::P;
use oxide_types::object::{JsObject, JsString, PropAttributes};
use oxide_types::value::JsValue;

/// 读 GC 压力模式开关：环境变量 `OXIDE_GC_PRESSURE` 存在即开启（取值不解析）。
///
/// 每次 VM 构造读一次，不做进程级缓存——缓存会在测试二进制内先构造的 VM 处
/// 锁死取值，后跑的测试再设环境变量不生效。VM 构造不是热路径（每测试/每池/
/// 每进程一次），读环境变量的开销可忽略。
fn gc_pressure_mode_from_env() -> bool {
    std::env::var_os("OXIDE_GC_PRESSURE").is_some()
}

impl Vm {
    /// Cons（rope）节点保留的单元阈值：拼接总长（UTF-16 单元数）≤ 该值时
    /// 急切扁平。小链的"链接 + 首次消费扁平化"双重分配高于直接拷贝，直接
    /// 扁平更优；超过阈值后 O(n²) 拷贝成本超过节点开销，Cons 的 O(1) 链接
    /// 才值得。
    pub(crate) const CONS_FLATTEN_UNITS: usize = 128;

    /// 替换本 VM 独占的内建原型对象（生成器 / Promise / 异步族与 Object 的
    /// 原型，下文简称 P 原型）：先恰好释放旧副本的属性区一次，
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
        // realm 编号在构造时分配并固化：首个 realm 编号为 0（与旧符号编码一致）。
        // 须在 builtin 绑定之前分配：绑定按 (realm 编号, 局部下标) 编码 well-known 符号键。
        let realm_id = core.alloc_realm_id();
        bindings::init_kernel_builtins(&core, &mut session, realm_id);
        // 在 builtin 绑定之后取 id：绑定过程已 intern "length"，此处命中缓存得到
        // 稳定的 id，用于运行期常见属性（如函数 length 槽）的快路径。
        let length_si = core.perm_interner().intern("length").0;
        let length_perm_ptr = core.perm_interner().string_ptr(length_si);
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        // 提前缓存执行期字符串 GC 初始水位（构造后 config 不再变化）。
        let gc_threshold = core.config().session_gc_threshold;
        let mut vm = Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Arc::default(),
            active_immutables: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            frames: smallvec::SmallVec::new(),
            next_frame_id: 0,
            kernel_core: core,
            // 缺省 no-op 编译服务：动态编译返 Err，生产 entry points 与
            // 动态编译测试经 set_compiler_service 注入真实实现。
            compiler: Arc::new(crate::vm::NoopCompilerService),
            realm: Arc::new(Realm {
                realm_id,
                session: RefCell::new(session),
                object_prototype: RefCell::new(obj_proto),
                generator_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                generator_function_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                promise_constructor: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                promise_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                aggregate_error_constructor: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                aggregate_error_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                async_function_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                async_generator_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                async_generator_function_proto: RefCell::new(P::new(JsObject::new_empty(
                    EMPTY_SHAPE_ID,
                    JsValue::null(),
                ))),
                gc: RefCell::new(GcState {
                    session_gc: crate::session_gc::SessionGc::new(),
                    session_object_ptrs: Vec::new(),
                    session_string_ptrs: Vec::new(),
                    session_bigint_ptrs: RefCell::new(Vec::new()),
                    session_cell_ptrs: RefCell::new(Vec::new()),
                    session_bytes_allocated: 0,
                    session_bytes_peak: 0,
                    run_alloc_peak: 0,
                    string_gc_watermark: gc_threshold,
                    gc_threshold_cached: gc_threshold,
                    gc_watermark: gc_threshold,
                    pending_forced_collect: false,
                    gc_pressure_mode: gc_pressure_mode_from_env(),
                    broadcast_channels: std::collections::HashMap::new(),
                }),
                symbols: RefCell::new(SymbolState {
                    symbol_descriptions: Vec::new(),
                    symbol_registry: std::collections::HashMap::new(),
                }),
            }),
            length_si,
            length_perm_ptr,
            number_to_string_cache_keys: [0u64; 16],
            number_to_string_cache_vals: [JsValue::undefined(); 16],
            job_queue: VecDeque::new(),
            atomics_waiters: std::collections::HashMap::new(),
            math_rng_state: 0,
            // gen 0 预登记空表占位：函数对象恒在 run 内创建（彼时 current_gen
            // ≥ 1），gen 0 表只是首 run 前路径的占位，首 run 边界即被回收。
            tables: std::collections::HashMap::from([(
                0u32,
                Box::new(TableGen {
                    modules: Arc::new(Vec::new()),
                    immutables: Vec::new(),
                    si_tables: Vec::new(),
                }),
            )]),
            current_gen: 0,
            saved_bytecode_stack: Vec::new(),
            saved_immutables_stack: Vec::new(),
            save_stack: Vec::new(),
            spill_stack: Vec::new(),
            dispose_stack: Vec::new(),
            dispose_marks: Vec::new(),
            disposing: false,
            native_overflow_base: 0,
            native_overflow_count: 0,
            try_stack: Vec::new(),
            exception_value: None,
            last_uncaught_value: None,
            evaluating_module: None,
            pending_length_exception: None,
            pending_exception: None,
            pending_error_kind: None,
            pending_completion: None,
            pending_async_escape: None,
            pending_async_dispose: None,
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            reentry_hops: 0,
            inline_args_base: 0,
            inline_args_count: 0,
            accessor_frame_target_reg: None,
            inline_callee: None,
            active_upvalues: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            inline_strict: false,
            inline_frames_base: 0,
            top_level_strict: false,
            top_level_this: JsValue::undefined(),
            inline_reg_pool: None,
            native_pack_end: 0,
            generator_suspended: None,
            delegated_iterator: None,
            generator_dispatch: false,
            generator_init_step: false,
            generator_body_started: false,
            async_context: None,
            async_suspended: false,
            async_dispatch: false,
            construct_dispatch: false,
            constructing_native: false,
            async_gen_context: None,
            async_gen_dispatch: false,
            async_gen_suspended: false,
            iters: IterState {
                for_in_iters: Vec::new(),
                for_of_iters: Vec::new(),
            },
            profiling: ProfilingState {
                ic_hits: std::cell::Cell::new(0),
                ic_misses: std::cell::Cell::new(0),
                instruction_count: 0,
            },
            sampling: SampleState {
                period: 0,
                top_k: 10,
                records: Vec::new(),
            },
            cell_stack: Vec::new(),
            template_objects: std::collections::HashMap::new(),
            active_flat_id: 0,
            active_table_gen: 0,
            saved_flat_id_stack: Vec::new(),
            saved_table_gen_stack: Vec::new(),
            trace_instructions: false,
            pc_watch: None,
            max_steps_override: None,
            worker_registry: std::collections::HashMap::new(),
            worker_next_id: 0,
            worker_objects: std::collections::HashMap::new(),
        };
        vm.init_generator_intrinsics();
        vm.init_promise_intrinsics();
        vm.init_async_intrinsics();
        vm.init_async_generator_intrinsics();
        // Promise 全局绑定发生在快照采集之后，重录快照避免首次 full_reset 误判脏。
        vm.realm.session.borrow_mut().record_snapshot();
        // 边界守卫计数：VM 完整构造后登记，与 `Drop for Vm` 的注销恰好配对。
        vm.kernel_core.note_vm_started();
        vm_info!("Vm created");
        vm
    }

    /// 复用共享 `KernelCore` 创建 VM（VM 池路径），共享 intern/shape/code 缓存。
    pub fn with_kernel_core(core: Arc<KernelCore>) -> Self {
        let mut session = KernelSession::new(&core);
        // realm 编号在构造时分配并固化：首个 realm 编号为 0（与旧符号编码一致）。
        // 须在 builtin 绑定之前分配：绑定按 (realm 编号, 局部下标) 编码 well-known 符号键。
        let realm_id = core.alloc_realm_id();
        bindings::init_kernel_builtins(&core, &mut session, realm_id);
        // 在 builtin 绑定之后取 id：绑定过程已 intern "length"，此处命中缓存得到
        // 稳定的 id，用于运行期常见属性（如函数 length 槽）的快路径。
        let length_si = core.perm_interner().intern("length").0;
        let length_perm_ptr = core.perm_interner().string_ptr(length_si);
        let obj_proto = P::clone(&session.builtin_world().object_proto);
        // 提前缓存执行期字符串 GC 初始水位（构造后 config 不再变化）。
        let gc_threshold = core.config().session_gc_threshold;
        let mut vm = Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Arc::default(),
            active_immutables: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            frames: smallvec::SmallVec::new(),
            next_frame_id: 0,
            kernel_core: core,
            // 缺省 no-op 编译服务：动态编译返 Err，生产 entry points 与
            // 动态编译测试经 set_compiler_service 注入真实实现。
            compiler: Arc::new(crate::vm::NoopCompilerService),
            realm: Arc::new(Realm {
                realm_id,
                session: RefCell::new(session),
                object_prototype: RefCell::new(obj_proto),
                generator_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                generator_function_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                promise_constructor: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                promise_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                aggregate_error_constructor: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                aggregate_error_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                async_function_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                async_generator_proto: RefCell::new(P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))),
                async_generator_function_proto: RefCell::new(P::new(JsObject::new_empty(
                    EMPTY_SHAPE_ID,
                    JsValue::null(),
                ))),
                gc: RefCell::new(GcState {
                    session_gc: crate::session_gc::SessionGc::new(),
                    session_object_ptrs: Vec::new(),
                    session_string_ptrs: Vec::new(),
                    session_bigint_ptrs: RefCell::new(Vec::new()),
                    session_cell_ptrs: RefCell::new(Vec::new()),
                    session_bytes_allocated: 0,
                    session_bytes_peak: 0,
                    run_alloc_peak: 0,
                    string_gc_watermark: gc_threshold,
                    gc_threshold_cached: gc_threshold,
                    gc_watermark: gc_threshold,
                    pending_forced_collect: false,
                    gc_pressure_mode: gc_pressure_mode_from_env(),
                    broadcast_channels: std::collections::HashMap::new(),
                }),
                symbols: RefCell::new(SymbolState {
                    symbol_descriptions: Vec::new(),
                    symbol_registry: std::collections::HashMap::new(),
                }),
            }),
            length_si,
            length_perm_ptr,
            number_to_string_cache_keys: [0u64; 16],
            number_to_string_cache_vals: [JsValue::undefined(); 16],
            job_queue: VecDeque::new(),
            atomics_waiters: std::collections::HashMap::new(),
            math_rng_state: 0,
            // gen 0 预登记空表占位：函数对象恒在 run 内创建（彼时 current_gen
            // ≥ 1），gen 0 表只是首 run 前路径的占位，首 run 边界即被回收。
            tables: std::collections::HashMap::from([(
                0u32,
                Box::new(TableGen {
                    modules: Arc::new(Vec::new()),
                    immutables: Vec::new(),
                    si_tables: Vec::new(),
                }),
            )]),
            current_gen: 0,
            saved_bytecode_stack: Vec::new(),
            saved_immutables_stack: Vec::new(),
            save_stack: Vec::new(),
            spill_stack: Vec::new(),
            dispose_stack: Vec::new(),
            dispose_marks: Vec::new(),
            disposing: false,
            native_overflow_base: 0,
            native_overflow_count: 0,
            try_stack: Vec::new(),
            exception_value: None,
            last_uncaught_value: None,
            evaluating_module: None,
            pending_length_exception: None,
            pending_exception: None,
            pending_error_kind: None,
            pending_completion: None,
            pending_async_escape: None,
            pending_async_dispose: None,
            root_reg_limit: 0,
            active_reg_limit: 0,
            native_call_depth: 0,
            reentry_hops: 0,
            inline_args_base: 0,
            inline_args_count: 0,
            accessor_frame_target_reg: None,
            inline_callee: None,
            active_upvalues: std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
            inline_strict: false,
            inline_frames_base: 0,
            top_level_strict: false,
            top_level_this: JsValue::undefined(),
            inline_reg_pool: None,
            native_pack_end: 0,
            generator_suspended: None,
            delegated_iterator: None,
            generator_dispatch: false,
            generator_init_step: false,
            generator_body_started: false,
            async_context: None,
            async_suspended: false,
            async_dispatch: false,
            construct_dispatch: false,
            constructing_native: false,
            async_gen_context: None,
            async_gen_dispatch: false,
            async_gen_suspended: false,
            iters: IterState {
                for_in_iters: Vec::new(),
                for_of_iters: Vec::new(),
            },
            profiling: ProfilingState {
                ic_hits: std::cell::Cell::new(0),
                ic_misses: std::cell::Cell::new(0),
                instruction_count: 0,
            },
            sampling: SampleState {
                period: 0,
                top_k: 10,
                records: Vec::new(),
            },
            cell_stack: Vec::new(),
            template_objects: std::collections::HashMap::new(),
            active_flat_id: 0,
            active_table_gen: 0,
            saved_flat_id_stack: Vec::new(),
            saved_table_gen_stack: Vec::new(),
            trace_instructions: false,
            pc_watch: None,
            max_steps_override: None,
            worker_registry: std::collections::HashMap::new(),
            worker_next_id: 0,
            worker_objects: std::collections::HashMap::new(),
        };
        vm.init_generator_intrinsics();
        vm.init_promise_intrinsics();
        vm.init_async_intrinsics();
        vm.init_async_generator_intrinsics();
        // Promise 全局绑定发生在快照采集之后，重录快照避免首次 full_reset 误判脏。
        vm.realm.session.borrow_mut().record_snapshot();
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
        // 池回收路径：若有活跃 worker，先终止并 join 全部（防线程泄漏与注册表
        // 残留：worker 线程各持自有 Vm，不 join 则线程孤儿、registry 条目残留）。
        self.shutdown_workers();
        // 防御兜底：属性值写原语已推进 generation，常规覆盖写由快照对比发现；
        // 若未来出现绕过属性写原语的裸属性区改写，session 对象会被 session GC 回收，
        // 保留 global 将持悬垂指针，故带 session 对象时强制 bump 保证 global 重建。
        if !self.realm.gc.borrow().session_object_ptrs.is_empty() {
            let global_ptr = self.realm.session.borrow().global_object().as_ptr() as *mut JsObject;
            unsafe { &mut *global_ptr }.bump_generation();
        }
        let dirty = self.realm.session.borrow_mut().selective_reset(&self.kernel_core);
        if dirty.any_builtin_dirty() {
            bindings::rebind_dirty_builtins(
                &self.kernel_core,
                &mut self.realm.session.borrow_mut(),
                Some(&dirty),
                self.realm_id(),
            );
        }
        if dirty.global {
            let global_ptr = self.realm.session.borrow().global_object().as_ptr() as *mut JsObject;
            let global = unsafe { &mut *global_ptr };
            bindings::bind_global_builtin_slots(
                &self.kernel_core,
                &self.realm.session.borrow_mut(),
                global,
                self.realm_id(),
            );
        }
        *self.realm.object_prototype.borrow_mut() = P::clone(&self.realm.session.borrow().builtin_world().object_proto);
        self.init_generator_intrinsics();
        self.init_promise_intrinsics();
        self.init_async_intrinsics();
        self.init_async_generator_intrinsics();
        // 快照须在 Promise 全局绑定之后采集：绑定会修改 global 世代。
        self.realm.session.borrow_mut().record_snapshot();
        self.clear_full_reset_state();
        vm_info!("full_reset completed");
    }

    /// benchmark 专用重置路径：总是丢弃并重建整个 session 与内置对象。
    #[doc(hidden)]
    pub fn full_reset_legacy_for_bench(&mut self) {
        *self.realm.session.borrow_mut() = KernelSession::new(&self.kernel_core);
        bindings::init_kernel_builtins(&self.kernel_core, &mut self.realm.session.borrow_mut(), self.realm_id());
        *self.realm.object_prototype.borrow_mut() = P::clone(&self.realm.session.borrow().builtin_world().object_proto);
        self.init_generator_intrinsics();
        self.init_promise_intrinsics();
        self.init_async_intrinsics();
        self.init_async_generator_intrinsics();
        self.realm.session.borrow_mut().record_snapshot();
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
                si_tables: Vec::new(),
            }),
        );
        self.active_immutables = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        // f64→string 缓存槽指向即将释放的 session 串：须在 teardown 之前清空，
        // 否则下次命中返回悬垂指针。
        self.number_to_string_cache_keys = [0u64; 16];
        self.number_to_string_cache_vals = [JsValue::undefined(); 16];
        // BroadcastChannel 注册表持 session 对象裸指针：teardown 释放全部
        // session 对象前须清表，否则残留悬垂指针。
        self.realm.gc.borrow_mut().broadcast_channels.clear();
        // Worker 对象注册表持 session 对象指针：teardown 释放全部 session 对象
        // 前须清表，否则残留悬垂指针。
        self.worker_objects.clear();
        self.realm.teardown_session_heap_data();
        self.realm.gc.borrow_mut().session_bytes_allocated = 0;
        self.realm.gc.borrow_mut().session_bytes_peak = 0;
        self.realm.gc.borrow_mut().run_alloc_peak = 0;
        self.realm.gc.borrow_mut().string_gc_watermark = self.kernel_core.config().session_gc_threshold;
        // 执行期收集水位同点复位（同式：阈值增量起算）：旧 run 的存活包络
        // 不延续到新 run 的触发判定。
        let threshold = self.realm.gc.borrow().gc_threshold_cached;
        self.realm.gc.borrow_mut().gc_watermark = threshold;
        self.realm.gc.borrow_mut().session_gc = crate::session_gc::SessionGc::new();
        self.realm.symbols.borrow_mut().reset();
        self.root_reg_limit = 0;
        self.active_reg_limit = 0;
        // 步数上限覆盖随池回收清位：执行路径按请求必设覆盖（含 None），
        // 此处清位保证回收的 VM 不带旧覆盖回到默认态，循环 VM 回到内核配置。
        self.max_steps_override = None;
    }

    pub(crate) fn clear_execution_state(&mut self) {
        // 重置契约：
        // - 清空寄存器文件、pc、帧/迭代器栈、保存的执行栈、try 处理器、
        //   待处理异常、native 调用深度与 IC 命中/未命中计数。
        // - 保留 kernel 共享状态不变。
        // - `reset()` 额外清空 bytecode/constants 并重置 run 分配包络。
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.frames.clear();
        self.iters.reset();
        self.saved_bytecode_stack.clear();
        self.saved_immutables_stack.clear();
        self.save_stack.clear();
        self.spill_stack.clear();
        // 释放栈条目持资源值（session 对象指针）：跨 run/reset 残留会在下一 run
        // 被误释放或悬垂，随执行状态一并清空；水位栈同清，防陈旧水位误截断。
        self.dispose_stack.clear();
        self.dispose_marks.clear();
        self.disposing = false;
        self.native_overflow_base = 0;
        self.native_overflow_count = 0;
        // native pack 实参区上界与溢出描述符同构：仅一次 native 调用期间有效，
        // 跨 run/reset 残留会在非 pack 上下文的恢复边界误跳镜像重载。
        self.native_pack_end = 0;
        self.cell_stack.clear();
        self.try_stack.clear();
        self.exception_value = None;
        // 未捕获异常侧通道持原始 session 对象指针：执行期状态，跨 run/reset 不保留，
        // 池回收后残留将悬垂。
        self.last_uncaught_value = None;
        self.pending_length_exception = None;
        self.pending_exception = None;
        self.pending_error_kind = None;
        self.pending_completion = None;
        // 在途异步逃出是执行期状态：跨 run/reset 不保留，残留将悬垂。
        self.pending_async_escape = None;
        // 在途异步释放同属执行期状态：跨 run/reset 不保留，残留将悬垂。
        self.pending_async_dispose = None;
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
        // upvalue 活动镜像是执行期状态：跨 run/reset 后指针失效（callee 对象
        // 可能已回收），清零为 null 空切片。
        self.active_upvalues = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        // 微任务队列是执行期状态：跨 run 不保留。
        self.job_queue.clear();
        // Atomics waiter 表同属执行期状态：未结算 promise 清位后无强根。
        self.atomics_waiters.clear();
        // IC 命中/未命中计数是执行期状态：池化 Vm 跨 run 复用，不清零则
        // per-run 指标（ic_hit_rate）跨文件累积，报告失真。
        self.profiling.ic_hits.set(0);
        self.profiling.ic_misses.set(0);
        // 采样记录同属执行期状态：池化 Vm 跨 run 复用，不清空则上一 run 的
        // 样本混入本 run 的直方图，热点分布失真。
        self.sampling.clear_records();
        // GC 统计同属执行期状态：池化 Vm 跨 run 复用，不清零则 per-run
        // 指标（gc_trigger_count）跨文件累积，报告失真。
        let gc = &mut self.realm.gc.borrow_mut().session_gc;
        gc.total_collections = 0;
        gc.total_bytes_freed = 0;
        gc.total_objects_scanned = 0;
        gc.total_objects_live = 0;
        gc.total_objects_dead = 0;
        gc.last_collection_objects_scanned = 0;
        gc.last_collection_objects_live = 0;
        gc.last_collection_objects_dead = 0;
        gc.last_collection_bytes_freed = 0;
        gc.last_collection_duration_us = 0;
        gc.max_collection_duration_us = 0;
        gc.min_collection_duration_us = u64::MAX;
    }

    /// 轻量重置：清空执行状态并回收内存，但保留 session 字符串与 builtin。
    pub fn reset(&mut self) {
        self.clear_execution_state();
        self.maybe_collect_session_gc();
        self.bytecode = Arc::default();
        // 表代际注册表不动：存活函数对象（含挂起帧 callee）按创建期代际仍须
        // 命中原表，跨 run 调用与恢复靠它成立。active_immutables 指向的旧表
        // 指针作废，下次 run 重装。
        self.active_immutables = std::ptr::slice_from_raw_parts(std::ptr::null(), 0);
        // 单 run 分配包络按 run 边界重起算（与 run_alloc_bytes 起算口径同源）。
        self.realm.gc.borrow_mut().run_alloc_peak = 0;
        // 执行期收集水位与包络同起算：旧 run 的存活包络不延续到新 run 的触发判定。
        let threshold = self.realm.gc.borrow().gc_threshold_cached;
        self.realm.gc.borrow_mut().gc_watermark = threshold;
        self.root_reg_limit = 0;
        self.active_reg_limit = 0;
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

    /// 有限 double 转串，带十六槽 last-value 缓存：命中返回已登记的 session 串，
    /// miss 走原数值转串链并写槽。
    ///
    /// # 步骤
    /// 1. 非有限值（NaN/±Infinity）返回对应永久常量串，不进槽。
    /// 2. 槽位取位模式低 4 位；槽值为字符串且键全等时返回槽值。
    /// 3. miss 走原数值转串链新建 session 串，写键与值后返回。
    ///
    /// # 副作用
    /// - 槽值经 `for_each_value` 登记为 GC 根，strings-only 收集不释放；
    ///   `full_reset` 在 session 串释放前清空槽。
    ///
    /// # 注意事项
    /// - 空槽判定用值非字符串：+0.0 的位模式是 0，键不得作空标记。
    /// - direct-mapped last-value：异值撞槽即覆盖，返回前键须 64 位全等。
    pub fn number_to_string_cached(&mut self, d: f64) -> JsValue {
        if d.is_nan() {
            return JsValue::string(oxide_kernel::string_forge::const_string_ptr(4));
        }
        if d.is_infinite() {
            return JsValue::string(oxide_kernel::string_forge::const_string_ptr(if d > 0.0 { 5 } else { 6 }));
        }
        let bits = d.to_bits();
        let slot = (bits as usize) & 15;
        if self.number_to_string_cache_vals[slot].is_string() && self.number_to_string_cache_keys[slot] == bits {
            return self.number_to_string_cache_vals[slot];
        }
        let v = self.new_string_owned(oxide_runtime_api::js_number_to_string(d));
        self.number_to_string_cache_keys[slot] = bits;
        self.number_to_string_cache_vals[slot] = v;
        v
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
        self.realm.gc.borrow_mut().session_string_ptrs.push(ptr);
        self.realm.gc.borrow_mut().session_bytes_allocated += std::mem::size_of::<JsString>() + bytes;
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
            JsValue::from_js_object(self.realm.async_generator_function_proto.borrow().as_ptr() as *mut JsObject)
        } else if is_generator {
            JsValue::from_js_object(self.realm.generator_function_proto.borrow().as_ptr() as *mut JsObject)
        } else if is_async {
            JsValue::from_js_object(self.realm.async_function_proto.borrow().as_ptr() as *mut JsObject)
        } else {
            JsValue::from_js_object(self.realm.session.borrow().builtin_world().function_proto.as_ptr() as *mut JsObject)
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
        // mark/sweep 回收），写入全局等逃逸根时地址稳定，全局属性与局部槽
        // 指向同一对象，严格相等保持。
        let obj_ptr = self.alloc_session_object(obj);
        let func_val = JsValue::object(obj_ptr as *mut u8);

        if !is_arrow && !(is_async && !is_generator) {
            // 原型对象自身的 [[Prototype]]：生成器为 %GeneratorPrototype%，普通函数为 Object.prototype。
            // 纯异步函数（非生成器）无 `prototype` 属性：规范不为其建 prototype 子对象。
            let proto_of_proto = if is_async_generator {
                JsValue::from_js_object(self.realm.async_generator_proto.borrow().as_ptr() as *mut JsObject)
            } else if is_generator {
                JsValue::from_js_object(self.realm.generator_proto.borrow().as_ptr() as *mut JsObject)
            } else {
                JsValue::from_js_object(
                    self.realm.session.borrow().builtin_world().object_proto.as_ptr() as *mut JsObject
                )
            };
            // prototype 子对象与函数本体同走 session 分配：`f.prototype ===
            // globalThis.f.prototype` 要求两侧同一对象，逃逸写不会克隆出第二份。
            let prototype = JsObject::new_empty(EMPTY_SHAPE_ID, proto_of_proto);
            let prototype_obj = self.alloc_session_object(prototype);
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

            // 函数自身 `prototype` 属性：生成器 writable:true / enumerable:false /
            // configurable:false；普通函数与构造器 writable:true / enumerable:false /
            // configurable:true。显式落元数据——缺元数据按默认描述符（可枚举）
            // 判定，`prototype` 会泄漏进 keys / for-in / 聚合枚举。
            let prototype_si = self.kernel_core.perm_interner().intern("prototype").0;
            let func = unsafe { &mut *obj_ptr };
            let prototype_shape = self.kernel_core.shape_forge().make_shape(func.shape_id(), prototype_si);
            func.set_shape_id(prototype_shape);
            let prototype_pos = func.push_prop(prototype_val);
            if is_generator || is_async_generator {
                func.set_data_meta(prototype_pos, PropAttributes::new(true, false, false));
            } else {
                func.set_data_meta(prototype_pos, PropAttributes::new(true, false, true));
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
    /// box 指针登记进 `realm.gc.session_bigint_ptrs`，在 `full_reset` 统一释放。
    /// `&self` 使 `convert_immutables`（常量池 → JsValue）也能分配。
    pub fn new_bigint(&self, v: num_bigint::BigInt) -> JsValue {
        let ptr = Box::into_raw(Box::new(v));
        self.realm.gc.borrow_mut().session_bigint_ptrs.borrow_mut().push(ptr);
        JsValue::bigint(ptr)
    }

    /// 读取 BigInt 值；调用方须保证 `val.is_bigint()`。
    pub fn bigint_value(&self, val: JsValue) -> &num_bigint::BigInt {
        // SAFETY: bigint 指针由 new_bigint 经 Box::into_raw 产生，存活至 full_reset。
        unsafe { &*val.as_bigint_ptr() }
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
mod tests;
