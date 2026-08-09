//! 异步生成器运行时：yield 挂起与 await 挂起共存，next() 返回 Promise。
//!
//! 策略：`async function*` 函数调用返回异步生成器迭代器对象（状态存
//! `AsyncGeneratorState`），body 惰性执行（参数初始化在调用时刻完成，首次
//! `next()` 才执行 body）。`next()/return()/throw()` 返回 Promise 并把请求入队，
//! 逐条处理。恢复执行复用生成器的帧快照机制（`YIELD` 让出）与异步的微任务
//! 驱动（`AWAIT` 挂起）：
//! - `yield` 让出：结算当前请求的 Promise 为 `{value, done:false}`；
//! - `await` 挂起：保留当前请求，等微任务恢复后继续执行；
//! - 完成/异常：结算当前请求为 `{value, done:true}` 或 reject。
//!
//! `AWAIT` 经 `Vm::async_gen_suspended` 信号让内嵌 dispatch 返回，恢复方据此
//! 快照挂起状态；`YIELD` 复用 `Vm::generator_suspended` 信号。

use std::collections::VecDeque;
use std::sync::Arc;

use oxide_builtins::iterator::make_iter_result;
use oxide_bytecode::opcode;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::mem::P;
use oxide_types::object::{Cell, JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::generator::{DelegateOutcome, GeneratorResumeMode};
use crate::vm::{CallFrame, Completion, ForInIter, FrameContinuation, TryHandler, Vm};

/// 异步生成器执行阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AsyncGenPhase {
    /// 尚未开始执行（参数初始化已完成，首次 next 前）。
    New,
    /// 正在执行（body 运行在嵌套 dispatch 循环中）。
    Running,
    /// 已在 `yield` 挂起，next/return/throw 可恢复。
    YieldSuspended,
    /// 已在 `await` 挂起，仅微任务恢复。
    AwaitSuspended,
    /// 已正常返回 / 被 `return()` 提前结束。
    Completed,
}

/// 一次 next/return/throw 请求：注入模式 + 结算能力（promise/resolve/reject）。
pub(crate) struct AsyncGenRequest {
    pub mode: GeneratorResumeMode,
    pub promise: JsValue,
    pub resolve: JsValue,
    pub reject: JsValue,
}

/// 异步生成器挂起时的完整执行上下文快照 + 请求队列。
///
/// 存在异步生成器对象 `native_data`（`Box`），跨 next()/微任务存活；其中所有
/// `JsValue` 由 session GC 经异步生成器对象边追踪（见 `session_gc`）。
pub(crate) struct AsyncGeneratorState {
    pub phase: AsyncGenPhase,
    /// 异步生成器函数对象（首次执行时压帧用）。
    pub callee: JsValue,
    /// 首次调用实参。
    pub args: Vec<JsValue>,
    /// 首次调用时的方法接收者（`obj.g()` 的 `obj`），压帧时作 `this` 绑定。
    pub this_value: JsValue,
    /// 完成后返回值。
    pub result: JsValue,
    /// next/return/throw 请求队列（FIFO，逐条处理）。
    pub queue: VecDeque<AsyncGenRequest>,
    /// 正在处理的请求：yield 让出 / await 挂起 / 完成时结算。
    pub current: Option<AsyncGenRequest>,
    // ── 挂起时的执行上下文（与生成器同构） ──
    pub regs: Box<[JsValue; 256]>,
    pub pc: usize,
    pub bytecode: Vec<opcode::Instr>,
    /// 异步生成器模块 flat_id（恢复时重激活 immutables）。
    pub sub_idx: u32,
    pub active_reg_limit: u8,
    pub root_reg_limit: u8,
    /// 异步生成器帧（压入时由 push_bytecode_frame 构造，挂起时弹出存此）。
    pub frame: Option<CallFrame>,
    pub spill_stack: Vec<JsValue>,
    pub save_stack: Vec<JsValue>,
    pub cell_stack: Vec<Vec<*mut Cell>>,
    pub try_stack: Vec<TryHandler>,
    pub for_in_iters: Vec<*mut ForInIter<'static>>,
    pub for_of_iters: Vec<JsValue>,
    pub last_for_of_result: JsValue,
    /// `yield*` 委托中的内层迭代器：Some = 挂起在委托点，恢复时转发 next/return/throw。
    pub delegated_iterator: Option<JsValue>,
    pub saved_bytecode_stack: Vec<Vec<opcode::Instr>>,
    pub saved_immutables_stack: Vec<*const [JsValue]>,
    /// 在途异常/完成（throw 穿越 finally 挂起时保留，恢复后继续展开）。
    pub exception_value: Option<JsValue>,
    pub pending_exception: Option<JsValue>,
    pub pending_error_kind: Option<&'static str>,
    pub pending_completion: Option<Completion>,
}

/// 恢复闭包上区分 reject 角色的属性名（await 恢复闭包定位用）。
const AG_REJECT_PROP: &str = "__oxide_async_gen_reject__";
/// yield 值 unwrap 恢复闭包上存目标请求 promise 的属性名。
const AG_YIELD_PROMISE_PROP: &str = "__oxide_async_gen_yield_promise__";
/// yield 值 unwrap 恢复闭包上区分委托透传（raw）的属性名。
const AG_YIELD_RAW_PROP: &str = "__oxide_async_gen_yield_raw__";

impl Vm {
    /// 调用异步生成器函数返回的迭代器对象：创建异步生成器实例、挂状态并执行
    /// 调用时参数初始化。
    ///
    /// 实例 [[Prototype]] = 调用方 `g.prototype`（为对象时），否则回退
    /// `%AsyncGeneratorPrototype%`（GetPrototypeFromConstructor 语义）。
    pub(crate) fn create_async_generator_object(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        let gen_proto_val = JsValue::from_js_object(self.async_generator_proto.as_ptr() as *mut JsObject);
        let obj = self.epoch.alloc(JsObject::new_empty(EMPTY_SHAPE_ID, gen_proto_val));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_ASYNC_GENERATOR;
        let state = Box::new(AsyncGeneratorState {
            phase: AsyncGenPhase::New,
            callee,
            args: args.to_vec(),
            this_value,
            result: JsValue::undefined(),
            queue: VecDeque::new(),
            current: None,
            regs: Box::new([JsValue::undefined(); 256]),
            pc: 0,
            bytecode: Vec::new(),
            sub_idx: 0,
            active_reg_limit: 0,
            root_reg_limit: 0,
            frame: None,
            spill_stack: Vec::new(),
            save_stack: Vec::new(),
            cell_stack: Vec::new(),
            try_stack: Vec::new(),
            for_in_iters: Vec::new(),
            for_of_iters: Vec::new(),
            last_for_of_result: JsValue::undefined(),
            delegated_iterator: None,
            saved_bytecode_stack: Vec::new(),
            saved_immutables_stack: Vec::new(),
            exception_value: None,
            pending_exception: None,
            pending_error_kind: None,
            pending_completion: None,
        });
        obj_ref.set_native_data(Box::into_raw(state) as *mut u8);
        self.gc_state.track_epoch_object(obj);
        let gen_val = JsValue::from_js_object(obj);
        // 调用时参数初始化（含默认值/解构/arguments 创建）：副作用与异常在 `g()` 时刻生效。
        self.initialize_async_generator(gen_val)?;
        // 实例 [[Prototype]] 在参数初始化之后读取：参数默认值可能改写 `g.prototype`
        // （GetPrototypeFromConstructor 语义），为对象则用，否则回退 %AsyncGeneratorPrototype%。
        if callee.is_object() {
            let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
            let callee_obj = unsafe { &*callee.as_js_object_ptr() };
            if let Some(p) = self.resolve_property(callee_obj, proto_si) {
                if p.is_object() {
                    let obj_ref = unsafe { &mut *obj };
                    obj_ref.set_proto(p).ok();
                }
            }
        }
        Ok(gen_val)
    }

    /// 异步生成器调用时的参数初始化步：压入异步生成器帧，执行到 body 起点标记
    /// （SUSPEND_BODY）。
    ///
    /// 参数初始化抛错时恢复调用方状态并在调用方上下文重新抛；成功则把挂起在
    /// body 起点的状态快照存入异步生成器对象。
    ///
    /// # 副作用
    /// - 异步生成器对象状态从 `New` 转为 `YieldSuspended`（body 起点）。
    /// - 参数默认值的副作用、`arguments` 创建、解构的迭代副作用在调用时刻完成。
    fn initialize_async_generator(&mut self, gen_val: JsValue) -> Result<(), String> {
        let state_ptr = self.async_gen_state_ptr(gen_val);
        let saved = self.save_inline_state();
        self.generator_suspended = None;
        // 嵌套生成器创建（参数默认值里调用其它 generator）会改写 init 标志，须保存恢复。
        let prev_init_step = self.generator_init_step;
        let prev_body_started = self.generator_body_started;
        self.generator_body_started = false;
        self.generator_init_step = true;
        self.native_call_depth += 1;

        // 压入异步生成器帧（New）。
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.bytecode = Vec::new();
        self.active_reg_limit = 1;
        self.root_reg_limit = 1;
        self.try_stack.clear();
        self.iters.for_in_iters.clear();
        self.iters.for_of_iters.clear();
        self.iters.last_for_of_result = JsValue::undefined();
        self.spill_stack.clear();
        self.save_stack.clear();
        self.saved_bytecode_stack.clear();
        self.saved_immutables_stack.clear();
        self.cell_stack.clear();
        self.inline_callee = None;
        let callee = unsafe { (*state_ptr).callee };
        let this_value = unsafe { (*state_ptr).this_value };
        let args = unsafe { std::mem::take(&mut (*state_ptr).args) };
        let push_res = self.push_bytecode_frame(
            callee,
            this_value,
            &args,
            None,
            None,
            JsValue::undefined(),
            FrameContinuation::None,
        );
        unsafe { (*state_ptr).args = args };
        if let Err(e) = push_res {
            self.restore_inline_state(saved);
            self.generator_init_step = false;
            self.native_call_depth -= 1;
            return Err(e);
        }
        unsafe { (*state_ptr).phase = AsyncGenPhase::Running };

        let prev_dispatch = self.generator_dispatch;
        self.generator_dispatch = true;
        let result = self.dispatch();
        self.generator_dispatch = prev_dispatch;
        self.generator_init_step = prev_init_step;
        let body_started = self.generator_body_started;
        self.generator_body_started = prev_body_started;
        self.native_call_depth -= 1;

        if body_started {
            // 参数初始化完成：快照挂起在 body 起点，首次 next() 从此继续。
            unsafe { (*state_ptr).phase = AsyncGenPhase::YieldSuspended };
            self.snapshot_async_generator(state_ptr)?;
            self.restore_inline_state(saved);
            return Ok(());
        }

        // 参数初始化抛错/异常结束：恢复调用方上下文后重新抛出。
        let exc = self.last_uncaught_value.take().unwrap_or_else(|| match result {
            Ok(v) => {
                oxide_builtins::error::create_error(self, &format!("async generator initialization failed: {v:?}"))
            }
            Err(e) => oxide_builtins::error::create_error(self, &e),
        });
        let kind = self.thrown_error_kind(exc);
        unsafe { (*state_ptr).phase = AsyncGenPhase::Completed };
        self.restore_inline_state(saved);
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(kind);
        self.unwind().map(|_| ())
    }

    /// 取出异步生成器对象的状态指针（调用方须先校验 `is_async_generator_obj`）。
    fn async_gen_state_ptr(&self, gen_val: JsValue) -> *mut AsyncGeneratorState {
        unsafe { (*gen_val.as_js_object_ptr()).native_data() as *mut AsyncGeneratorState }
    }

    /// 入队一个请求；生成器处于可恢复状态（New/YieldSuspended/Completed）时立即
    /// 启动处理，正在执行 / await 挂起时留待当前请求结算后再处理。
    pub(crate) fn async_generator_enqueue(
        &mut self, gen_val: JsValue, mode: GeneratorResumeMode,
    ) -> Result<JsValue, JsValue> {
        if !gen_val.is_object() {
            return Err(oxide_builtins::error::create_type_error(
                self,
                "AsyncGenerator methods called on non-object",
            ));
        }
        let obj = unsafe { &*gen_val.as_js_object_ptr() };
        if !obj.is_async_generator_obj() {
            return Err(oxide_builtins::error::create_type_error(
                self,
                "AsyncGenerator methods called on incompatible receiver",
            ));
        }
        let (promise, resolve, reject) = self.new_promise_capability();
        let state_ptr = self.async_gen_state_ptr(gen_val);
        {
            let state = unsafe { &mut *state_ptr };
            state.queue.push_back(AsyncGenRequest { mode, promise, resolve, reject });
        }
        let phase = unsafe { (*state_ptr).phase };
        if matches!(phase, AsyncGenPhase::New | AsyncGenPhase::YieldSuspended | AsyncGenPhase::Completed) {
            if let Err(e) = self.async_generator_start(gen_val) {
                let exc = self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
                return Err(exc);
            }
        }
        Ok(promise)
    }

    /// 处理队首请求：New/YieldSuspended 恢复执行，Completed 直接结算。
    ///
    /// Running/AwaitSuspended 期间不启动（当前请求未结算，请求保持排队）。
    pub(crate) fn async_generator_start(&mut self, gen_val: JsValue) -> Result<(), String> {
        let state_ptr = self.async_gen_state_ptr(gen_val);
        let phase = unsafe { (*state_ptr).phase };
        if matches!(phase, AsyncGenPhase::Running | AsyncGenPhase::AwaitSuspended) {
            return Ok(());
        }
        let request = {
            let state = unsafe { &mut *state_ptr };
            match state.queue.pop_front() {
                Some(r) => r,
                None => return Ok(()),
            }
        };
        let phase = unsafe { (*state_ptr).phase };
        if phase == AsyncGenPhase::Completed {
            // 已完成：按请求模式直接结算。
            match request.mode {
                GeneratorResumeMode::Next(_) => {
                    let result = make_iter_result(self, JsValue::undefined(), true);
                    let _ = self.resolve_promise(request.promise, result);
                }
                GeneratorResumeMode::Return(v) => {
                    let result = make_iter_result(self, v, true);
                    let _ = self.resolve_promise(request.promise, result);
                }
                GeneratorResumeMode::Throw(e) => {
                    let _ = self.reject_promise(request.promise, e);
                }
            }
            if !unsafe { (*state_ptr).queue.is_empty() } {
                self.async_generator_start(gen_val)?;
            }
            return Ok(());
        }
        {
            let state = unsafe { &mut *state_ptr };
            state.current = Some(request);
        }
        self.resume_async_generator(gen_val)
    }

    /// 恢复异步生成器一步执行：执行到下一个 YIELD/AWAIT/RETURN/异常。
    ///
    /// 注入模式取自 `current` 请求（await 恢复闭包会改写 current 的 mode 以注入
    /// await 结果；next/return/throw 入队时 mode 已固定）。
    ///
    /// # 步骤
    /// 1. 用 `InlineSyncState` 保存调用方 VM 状态，把挂起快照灌回 VM。
    /// 2. 按模式注入：next 写 reg 0；throw 注入异常；return 完成穿越。
    /// 3. 嵌套 `dispatch()` 运行 body 到下一个 YIELD/AWAIT/RETURN/异常。
    /// 4. 让出则快照新状态并结算当前请求；完成/异常则标记阶段并结算。
    ///
    /// # 副作用
    /// - 修改异步生成器对象 `native_data` 中的快照与请求状态。
    /// - 嵌套 dispatch 期间 VM 完全被异步生成器状态占据。
    pub(crate) fn resume_async_generator(&mut self, gen_val: JsValue) -> Result<(), String> {
        let state_ptr = self.async_gen_state_ptr(gen_val);
        let saved = self.save_inline_state();
        self.generator_suspended = None;
        self.async_gen_suspended = false;
        let prev_ctx = self.async_context.take();
        let prev_gd = self.generator_dispatch;
        let prev_ad = self.async_dispatch;
        let prev_gen_ctx = self.async_gen_context.take();
        let prev_agd = self.async_gen_dispatch;
        self.generator_dispatch = true;
        self.async_dispatch = true;
        self.async_gen_dispatch = true;
        self.async_context = Some(gen_val);
        self.async_gen_context = Some(gen_val);
        self.native_call_depth += 1;

        {
            let state = unsafe { &mut *state_ptr };
            self.regs = *state.regs;
            self.pc = state.pc;
            self.bytecode = std::mem::take(&mut state.bytecode);
            let subs = Arc::clone(&self.sub_modules);
            if (state.sub_idx as usize) < subs.len() {
                self.activate_immutables(state.sub_idx as usize, &subs[state.sub_idx as usize].constants);
            } else {
                // 挂起状态跨 run：sub_modules 已重建，无法恢复（与生成器同限制）。
                self.async_gen_dispatch = prev_agd;
                self.async_gen_context = prev_gen_ctx;
                self.generator_dispatch = prev_gd;
                self.async_dispatch = prev_ad;
                self.async_context = prev_ctx;
                self.native_call_depth -= 1;
                self.restore_inline_state(saved);
                return Err("async generator suspended across runs is no longer valid".into());
            }
            self.active_reg_limit = state.active_reg_limit;
            self.root_reg_limit = state.root_reg_limit;
            self.frames.clear();
            if let Some(frame) = state.frame.take() {
                self.frames.push(frame);
            }
            self.spill_stack = std::mem::take(&mut state.spill_stack);
            self.save_stack = std::mem::take(&mut state.save_stack);
            self.cell_stack = std::mem::take(&mut state.cell_stack);
            self.try_stack = std::mem::take(&mut state.try_stack);
            self.iters.for_in_iters = std::mem::take(&mut state.for_in_iters);
            self.iters.for_of_iters = std::mem::take(&mut state.for_of_iters);
            self.iters.last_for_of_result = state.last_for_of_result;
            self.delegated_iterator = state.delegated_iterator.take();
            self.saved_bytecode_stack = std::mem::take(&mut state.saved_bytecode_stack);
            self.saved_immutables_stack = std::mem::take(&mut state.saved_immutables_stack);
            self.exception_value = state.exception_value.take();
            self.pending_exception = state.pending_exception.take();
            self.pending_error_kind = state.pending_error_kind.take();
            self.pending_completion = state.pending_completion.take();
            self.inline_callee = None;
            state.phase = AsyncGenPhase::Running;
        }

        // 委托恢复：把 next/return/throw 请求转发给内层迭代器。
        if self.delegated_iterator.is_some() {
            let mode = {
                let state = unsafe { &mut *state_ptr };
                state
                    .current
                    .as_ref()
                    .map(|c| c.mode)
                    .unwrap_or(GeneratorResumeMode::Next(JsValue::undefined()))
            };
            let forwarded = self.delegate_forward(mode);
            match forwarded {
                Err(e) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
                    self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                    return self.finish_async_gen_exc(state_ptr, saved, gen_val, exc);
                }
                Ok(DelegateOutcome::Suspend { value }) => {
                    self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                    // AsyncGeneratorYield：委托让出值经 promise unwrap 后原样透传。
                    let value_promise = if self.is_promise_value(value) {
                        value
                    } else {
                        let (p, _, _) = self.new_promise_capability();
                        let _ = self.resolve_promise(p, value);
                        p
                    };
                    let request = {
                        let state = unsafe { &mut *state_ptr };
                        state.current.take()
                    };
                    unsafe { (*state_ptr).phase = AsyncGenPhase::YieldSuspended };
                    self.snapshot_async_generator(state_ptr)?;
                    self.restore_inline_state(saved);
                    if let Some(req) = request {
                        let fulfill_fn = self.make_async_gen_yield_unwrap_fn(gen_val, req.promise, true, false);
                        let reject_fn = self.make_async_gen_yield_unwrap_fn(gen_val, req.promise, true, true);
                        let _ = self.perform_promise_then(value_promise, fulfill_fn, reject_fn);
                    }
                    return Ok(());
                }
                Ok(DelegateOutcome::Continue { value }) => {
                    self.regs[0] = value;
                }
                Ok(DelegateOutcome::Complete { value }) => {
                    let completed = self.complete_generator_return(value);
                    if let Some(completed) = completed? {
                        self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                        return self.finish_async_gen_ok(state_ptr, saved, gen_val, completed);
                    }
                    // 进入 finally 穿越：继续 dispatch。
                }
                Ok(DelegateOutcome::Unwind) => {
                    self.delegated_iterator = None;
                }
            }
        } else {
            // 注入模式：next(v) 写 reg 0；throw(e) 注入异常；return(v) 完成穿越。
            let mode = {
                let state = unsafe { &mut *state_ptr };
                state.current.as_ref().map(|c| c.mode)
            };
            if let Some(mode) = mode {
                match mode {
                    GeneratorResumeMode::Next(arg) => {
                        self.regs[0] = arg;
                    }
                    GeneratorResumeMode::Throw(exc) => {
                        self.exception_value = Some(exc);
                        self.pending_error_kind = Some(self.thrown_error_kind(exc));
                        if self.unwind().is_err() {
                            let thrown = self.last_uncaught_value.take().unwrap_or(exc);
                            self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                            return self.finish_async_gen_exc(state_ptr, saved, gen_val, thrown);
                        }
                    }
                    GeneratorResumeMode::Return(value) => {
                        let completed = self.complete_generator_return(value);
                        if let Some(completed) = completed? {
                            self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                            return self.finish_async_gen_ok(state_ptr, saved, gen_val, completed);
                        }
                        // 进入 finally 穿越：继续 dispatch。
                    }
                }
            }
        }

        let result = self.dispatch();
        self.async_gen_dispatch = prev_agd;
        self.async_gen_context = prev_gen_ctx;
        self.generator_dispatch = prev_gd;
        self.async_dispatch = prev_ad;
        self.async_context = prev_ctx;
        self.native_call_depth -= 1;

        // YIELD 让出：快照挂起状态，让出值经 promise unwrap 后结算当前请求。
        if let Some(value) = self.generator_suspended.take() {
            let delegating = self.delegated_iterator.is_some();
            // AsyncGeneratorYield：让出值 PromiseResolve 包装后 await 展开
            // （yield 一个 rejected promise 时 next() 的 promise 被 reject）。
            let value_promise = if self.is_promise_value(value) {
                value
            } else {
                let (p, _, _) = self.new_promise_capability();
                let _ = self.resolve_promise(p, value);
                p
            };
            let request = {
                let state = unsafe { &mut *state_ptr };
                state.current.take()
            };
            unsafe { (*state_ptr).phase = AsyncGenPhase::YieldSuspended };
            self.snapshot_async_generator(state_ptr)?;
            self.restore_inline_state(saved);
            if let Some(req) = request {
                let fulfill_fn = self.make_async_gen_yield_unwrap_fn(gen_val, req.promise, delegating, false);
                let reject_fn = self.make_async_gen_yield_unwrap_fn(gen_val, req.promise, delegating, true);
                let _ = self.perform_promise_then(value_promise, fulfill_fn, reject_fn);
            }
            return Ok(());
        }

        // AWAIT 挂起：快照挂起状态，当前请求保留，等微任务恢复。
        if self.async_gen_suspended {
            self.async_gen_suspended = false;
            unsafe { (*state_ptr).phase = AsyncGenPhase::AwaitSuspended };
            self.snapshot_async_generator(state_ptr)?;
            self.restore_inline_state(saved);
            return Ok(());
        }

        // 完成或异常逃逸。
        match result {
            Ok(value) => self.finish_async_gen_ok(state_ptr, saved, gen_val, value),
            Err(e) => {
                let exc = self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
                self.finish_async_gen_exc(state_ptr, saved, gen_val, exc)
            }
        }
    }

    /// 恢复嵌套 dispatch 覆盖的 VM 标志（dispatch 前提前返回的路径用）。
    fn restore_async_gen_flags(
        &mut self, prev_ctx: Option<JsValue>, prev_gen_ctx: Option<JsValue>, prev_gd: bool, prev_ad: bool,
        prev_agd: bool,
    ) {
        self.async_gen_dispatch = prev_agd;
        self.async_gen_context = prev_gen_ctx;
        self.generator_dispatch = prev_gd;
        self.async_dispatch = prev_ad;
        self.async_context = prev_ctx;
        self.native_call_depth -= 1;
    }

    /// 正常完成路径：标记阶段、恢复调用方、结算当前请求后处理队列。
    fn finish_async_gen_ok(
        &mut self, state_ptr: *mut AsyncGeneratorState, saved: Box<crate::vm::InlineSyncState>, gen_val: JsValue,
        value: JsValue,
    ) -> Result<(), String> {
        unsafe { (*state_ptr).phase = AsyncGenPhase::Completed };
        unsafe { (*state_ptr).result = value };
        self.restore_inline_state(saved);
        self.settle_current_request(state_ptr, Some(value), false, true)?;
        if !unsafe { (*state_ptr).queue.is_empty() } {
            self.async_generator_start(gen_val)?;
        }
        Ok(())
    }

    /// 异常结束路径：标记阶段、恢复调用方、reject 当前请求后处理队列。
    fn finish_async_gen_exc(
        &mut self, state_ptr: *mut AsyncGeneratorState, saved: Box<crate::vm::InlineSyncState>, gen_val: JsValue,
        exc: JsValue,
    ) -> Result<(), String> {
        unsafe { (*state_ptr).phase = AsyncGenPhase::Completed };
        unsafe { (*state_ptr).result = JsValue::undefined() };
        self.restore_inline_state(saved);
        let current = {
            let state = unsafe { &mut *state_ptr };
            state.current.take()
        };
        if let Some(req) = current {
            let _ = self.reject_promise(req.promise, exc);
        }
        if !unsafe { (*state_ptr).queue.is_empty() } {
            self.async_generator_start(gen_val)?;
        }
        Ok(())
    }

    /// 结算当前请求：`raw` 为 true（委托让出）时直接以内层原始结果对象 resolve，
    /// 否则包装为 `{value, done}`（done 由调用方指定：yield 让出 false，完成 true）。
    fn settle_current_request(
        &mut self, state_ptr: *mut AsyncGeneratorState, value: Option<JsValue>, raw: bool, done: bool,
    ) -> Result<(), String> {
        let current = {
            let state = unsafe { &mut *state_ptr };
            state.current.take()
        };
        if let Some(req) = current {
            let result = match value {
                Some(v) if raw => v,
                Some(v) => make_iter_result(self, v, done),
                None => make_iter_result(self, JsValue::undefined(), done),
            };
            let _ = self.resolve_promise(req.promise, result);
        }
        Ok(())
    }

    /// 把当前 VM 执行状态（异步生成器 body 刚在嵌套 dispatch 中让出）快照进状态盒。
    ///
    /// 异步生成器帧弹出存 `frame`，其余栈段整段搬入（嵌套循环期间这些栈只含
    /// 异步生成器数据）。请求队列与当前请求保留在状态盒中不动。
    fn snapshot_async_generator(&mut self, state_ptr: *mut AsyncGeneratorState) -> Result<(), String> {
        let state = unsafe { &mut *state_ptr };
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| "async generator frame missing on yield".to_string())?;
        state.frame = Some(frame);
        state.regs = Box::new(self.regs);
        state.pc = self.pc;
        state.bytecode = std::mem::take(&mut self.bytecode);
        state.sub_idx = if state.callee.is_object() {
            unsafe { (*state.callee.as_js_object_ptr()).sub_module_index() }
        } else {
            0
        };
        state.active_reg_limit = self.active_reg_limit;
        state.root_reg_limit = self.root_reg_limit;
        state.spill_stack = std::mem::take(&mut self.spill_stack);
        state.save_stack = std::mem::take(&mut self.save_stack);
        state.cell_stack = std::mem::take(&mut self.cell_stack);
        state.try_stack = std::mem::take(&mut self.try_stack);
        state.for_in_iters = std::mem::take(&mut self.iters.for_in_iters);
        state.for_of_iters = std::mem::take(&mut self.iters.for_of_iters);
        state.last_for_of_result = self.iters.last_for_of_result;
        state.delegated_iterator = self.delegated_iterator.take();
        state.saved_bytecode_stack = std::mem::take(&mut self.saved_bytecode_stack);
        state.saved_immutables_stack = std::mem::take(&mut self.saved_immutables_stack);
        state.exception_value = self.exception_value.take();
        state.pending_exception = self.pending_exception.take();
        state.pending_error_kind = self.pending_error_kind.take();
        state.pending_completion = self.pending_completion.take();
        Ok(())
    }

    /// 构造 await 恢复闭包：携带目标异步生成器上下文对象，区分 fulfill/reject 角色。
    pub(crate) fn make_async_gen_await_resume_fn(&mut self, ctx: JsValue, reject_role: bool) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: async_gen_await_resume_closure 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_gen_await_resume_closure as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        let ctx_si = self.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
        self.set_or_create_prop_value(obj, ctx_si, ctx);
        let role_si = self.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, role_si, JsValue::bool(reject_role));
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }

    /// 构造 yield 值 unwrap 恢复闭包：携带目标请求 promise、委托透传标志与
    /// 异步生成器上下文，settle 后驱动队列。
    fn make_async_gen_yield_unwrap_fn(
        &mut self, ctx: JsValue, promise: JsValue, raw: bool, reject_role: bool,
    ) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: async_gen_yield_unwrap_closure 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_gen_yield_unwrap_closure as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        let ctx_si = self.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
        self.set_or_create_prop_value(obj, ctx_si, ctx);
        let prom_si = self.kernel_core.perm_interner().intern(AG_YIELD_PROMISE_PROP).0;
        self.set_or_create_prop_value(obj, prom_si, promise);
        let raw_si = self.kernel_core.perm_interner().intern(AG_YIELD_RAW_PROP).0;
        self.set_or_create_prop_value(obj, raw_si, JsValue::bool(raw));
        let role_si = self.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, role_si, JsValue::bool(reject_role));
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }
}

/// await 恢复闭包：读自身 prop 的异步生成器上下文与角色，恢复异步生成器继续执行。
///
/// 把 await 结果注入当前请求（改写 current.mode），随后 resume。
fn async_gen_await_resume_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "await resume handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let ctx_si = vm.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
    let ctx = vm.resolve_property(callee_obj, ctx_si).unwrap_or(JsValue::undefined());
    let role_si = vm.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, role_si)
        .map_or(false, oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    // 把 await 结果写入当前请求的注入模式，resume 时按角色恢复。
    if ctx.is_object() {
        let obj = unsafe { &*ctx.as_js_object_ptr() };
        if obj.is_async_generator_obj() {
            let state = vm.async_gen_state_ptr(ctx);
            let state = unsafe { &mut *state };
            if let Some(req) = state.current.as_mut() {
                req.mode = if is_reject {
                    GeneratorResumeMode::Throw(value)
                } else {
                    GeneratorResumeMode::Next(value)
                };
            }
        }
    }
    match vm.resume_async_generator(ctx) {
        Ok(()) => NativeResult::Ok(JsValue::undefined()),
        Err(e) => NativeResult::Err(oxide_builtins::error::create_error(vm, &e)),
    }
}

/// yield 值 unwrap 恢复闭包：读自身 prop 的目标 promise 与角色，结算请求后驱动队列。
///
/// 不恢复生成器执行（生成器已让出，等待下一个 next/return/throw）。
fn async_gen_yield_unwrap_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "yield unwrap handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let ctx_si = vm.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
    let ctx = vm.resolve_property(callee_obj, ctx_si).unwrap_or(JsValue::undefined());
    let prom_si = vm.kernel_core.perm_interner().intern(AG_YIELD_PROMISE_PROP).0;
    let promise = vm.resolve_property(callee_obj, prom_si).unwrap_or(JsValue::undefined());
    let raw_si = vm.kernel_core.perm_interner().intern(AG_YIELD_RAW_PROP).0;
    let raw = vm
        .resolve_property(callee_obj, raw_si)
        .map_or(false, oxide_runtime_api::to_boolean);
    let role_si = vm.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, role_si)
        .map_or(false, oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if is_reject {
        // yield 值被拒：next() 的 promise reject，且生成器关闭（后续 next 直接 done）。
        let _ = vm.reject_promise(promise, value);
        if ctx.is_object() {
            let obj = unsafe { &*ctx.as_js_object_ptr() };
            if obj.is_async_generator_obj() {
                let state = vm.async_gen_state_ptr(ctx);
                // 仅在仍挂起在 yield 点（未被新的 next 恢复）时关闭。
                if unsafe { (*state).phase } == AsyncGenPhase::YieldSuspended {
                    unsafe { (*state).phase = AsyncGenPhase::Completed };
                    unsafe { (*state).result = JsValue::undefined() };
                }
            }
        }
    } else {
        let result = if raw { value } else { make_iter_result(vm, value, false) };
        let _ = vm.resolve_promise(promise, result);
    }
    if ctx.is_object() {
        let _ = vm.async_generator_start(ctx);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `%AsyncGeneratorPrototype%.next`：入队 next 请求并返回其结果 Promise。
pub(crate) fn async_generator_next(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match vm.async_generator_enqueue(this_val, GeneratorResumeMode::Next(arg)) {
        Ok(p) => NativeResult::Ok(p),
        Err(e) => NativeResult::Err(e),
    }
}

/// `%AsyncGeneratorPrototype%.return`：入队提前结束请求并返回其结果 Promise。
pub(crate) fn async_generator_return(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match vm.async_generator_enqueue(this_val, GeneratorResumeMode::Return(arg)) {
        Ok(p) => NativeResult::Ok(p),
        Err(e) => NativeResult::Err(e),
    }
}

/// `%AsyncGeneratorPrototype%.throw`：入队异常注入请求并返回其结果 Promise。
pub(crate) fn async_generator_throw(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match vm.async_generator_enqueue(this_val, GeneratorResumeMode::Throw(arg)) {
        Ok(p) => NativeResult::Ok(p),
        Err(e) => NativeResult::Err(e),
    }
}

/// `%AsyncGeneratorPrototype%[@@asyncIterator]`：异步生成器自身即是异步迭代器，返回 `this`。
pub(crate) fn async_generator_symbol_async_iterator(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    NativeResult::Ok(this_val)
}

/// 初始化/重建异步生成器内建对象：`%AsyncGeneratorPrototype%`、
/// `%AsyncGeneratorFunction.prototype%` 与占位 `%AsyncGeneratorFunction%`。
///
/// 两个原型对象存于 VM 字段（session 生命周期），`full_reset` 后重建。
pub(crate) fn init_async_generator_intrinsics(vm: &mut Vm) {
    let sf = vm.kernel_core.perm_interner().as_ref();
    let sh = vm.kernel_core.shape_forge().as_ref();
    let fn_proto_val = vm.session.builtin_world().fn_proto_val();
    let object_proto_val = JsValue::from_js_object(vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject);

    // %AsyncGeneratorPrototype%：proto = Object.prototype，方法 next/return/throw。
    let mut ag_proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, object_proto_val));
    oxide_kernel::bind_methods_static!(
        &mut ag_proto,
        sf,
        sh,
        fn_proto_val,
        ("next", async_generator_next as *const (), 1),
        ("return", async_generator_return as *const (), 1),
        ("throw", async_generator_throw as *const (), 1),
    );
    // @@toStringTag（Object.prototype.toString → "[object AsyncGenerator]"）。
    let tag_si = sf.intern("@@toStringTag").0;
    let tag_shape = sh.make_shape(ag_proto.shape_id(), tag_si);
    ag_proto.set_shape_id(tag_shape);
    let tag_pos = ag_proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AsyncGenerator").0)));
    ag_proto.set_data_meta(tag_pos, PropAttributes::new(false, false, true));
    // @@asyncIterator：返回自身（异步生成器是异步可迭代对象，供 for-await-of 消费）。
    let aiter_key = oxide_types::private_key::make_well_known_symbol_key(8);
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method_key_static(
        &mut ag_proto,
        sh,
        sf,
        aiter_key,
        "@@asyncIterator",
        unsafe { oxide_types::object::NativeFnPtr::from_raw(async_generator_symbol_async_iterator as *const ()) },
        0,
        fn_proto_val,
    );
    vm.async_generator_proto = P::new(*ag_proto);

    // %AsyncGeneratorFunction.prototype%：proto = Function.prototype，constructor = %AsyncGeneratorFunction%。
    let mut agf_ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    agf_ctor.set_function(true);
    agf_ctor.set_native_arg_count(1);
    // %AsyncGeneratorFunction% 构造器：动态创建未实现，调用抛 TypeError。
    agf_ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_generator_function_stub as *const ()) }));
    let mut agf_proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    // prototype/name 属性（构造器形状 [prototype, name]）。
    let proto_si = sf.intern("prototype").0;
    let ctor_shape = sh.make_shape(agf_ctor.shape_id(), proto_si);
    agf_ctor.set_shape_id(ctor_shape);
    let ppos = agf_ctor.push_prop(JsValue::from_js_object(agf_proto.as_mut() as *mut JsObject));
    agf_ctor.set_data_meta(ppos, PropAttributes::new(false, false, false));
    let name_si = sf.intern("name").0;
    let name_shape = sh.make_shape(agf_ctor.shape_id(), name_si);
    agf_ctor.set_shape_id(name_shape);
    let npos = agf_ctor.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AsyncGeneratorFunction").0)));
    agf_ctor.set_data_meta(npos, PropAttributes::new(false, false, true));
    // agf_proto.constructor = %AsyncGeneratorFunction%（构造器对象泄漏持有，与 builtin 方法 wrapper 同生命周期）。
    let ctor_si = sf.intern("constructor").0;
    let ctor2_shape = sh.make_shape(agf_proto.shape_id(), ctor_si);
    agf_proto.set_shape_id(ctor2_shape);
    let cpos = agf_proto.push_prop(JsValue::from_js_object(Box::into_raw(agf_ctor)));
    agf_proto.set_data_meta(cpos, PropAttributes::new(false, false, true));
    // agf_proto.prototype = %AsyncGeneratorPrototype%。
    let proto2_si = sf.intern("prototype").0;
    let proto2_shape = sh.make_shape(agf_proto.shape_id(), proto2_si);
    agf_proto.set_shape_id(proto2_shape);
    let ppos2 = agf_proto.push_prop(JsValue::from_js_object(vm.async_generator_proto.as_ptr() as *mut JsObject));
    agf_proto.set_data_meta(ppos2, PropAttributes::new(false, false, false));
    // agf_proto[Symbol.toStringTag] = "AsyncGeneratorFunction"（数据属性，w/e/c = false/false/true）。
    let tag2_key = oxide_types::private_key::make_well_known_symbol_key(0);
    let tag2_shape = sh.make_shape(agf_proto.shape_id(), tag2_key);
    agf_proto.set_shape_id(tag2_shape);
    let tag2_pos = agf_proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AsyncGeneratorFunction").0)));
    agf_proto.set_data_meta(tag2_pos, PropAttributes::new(false, false, true));

    vm.async_generator_function_proto = P::new(*agf_proto);
}

/// `%AsyncGeneratorFunction%` 占位：动态异步生成器函数创建未实现，调用抛错。
fn async_generator_function_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    NativeResult::Err(oxide_builtins::error::create_type_error(
        vm,
        "AsyncGeneratorFunction constructor is not supported",
    ))
}

// ── session GC 支撑：状态快照中的 JsValues 作为异步生成器对象边追踪 ──

fn async_gen_state_mut(obj: &JsObject) -> Option<&mut AsyncGeneratorState> {
    let ptr = obj.native_data() as *mut AsyncGeneratorState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 在 create_async_generator_object 中由 Box::into_raw 分配，
        // 生命周期与异步生成器对象一致；GC 只在 reset（无执行状态）时运行。
        Some(unsafe { &mut *ptr })
    }
}

/// 异步生成器状态内所有对象引用的扁平列表（GC mark 边）。
pub(crate) fn async_generator_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = async_gen_state_mut(obj) else {
        return Vec::new();
    };
    let mut edges = Vec::new();
    let push = |v: JsValue, edges: &mut Vec<JsValue>| {
        if v.is_object() {
            edges.push(v);
        }
    };
    push(state.callee, &mut edges);
    edges.extend(state.args.iter().copied().filter(|v| v.is_object()));
    push(state.result, &mut edges);
    for req in state.queue.iter() {
        push(req.promise, &mut edges);
        push(req.resolve, &mut edges);
        push(req.reject, &mut edges);
    }
    if let Some(req) = &state.current {
        push(req.promise, &mut edges);
        push(req.resolve, &mut edges);
        push(req.reject, &mut edges);
    }
    edges.extend(state.regs.iter().copied().filter(|v| v.is_object()));
    if let Some(frame) = &state.frame {
        push(frame.saved_this, &mut edges);
        push(frame.saved_new_target, &mut edges);
        push(frame.callee, &mut edges);
        if let Some(ct) = frame.constructed_this {
            push(ct, &mut edges);
        }
    }
    edges.extend(state.spill_stack.iter().copied().filter(|v| v.is_object()));
    edges.extend(state.save_stack.iter().copied().filter(|v| v.is_object()));
    for cells in &state.cell_stack {
        for &cell_ptr in cells {
            if cell_ptr.is_null() {
                continue;
            }
            // SAFETY: cell 由 session_epoch 分配，本 session 内指针有效。
            let cell = unsafe { &*cell_ptr };
            push(cell.value, &mut edges);
        }
    }
    edges.extend(state.for_of_iters.iter().copied().filter(|v| v.is_object()));
    push(state.last_for_of_result, &mut edges);
    if let Some(iter) = state.delegated_iterator {
        push(iter, &mut edges);
    }
    edges
}

/// 用转发函数重写状态快照中的所有 JsValue（session GC 移动式清扫 / promote 用）。
pub(crate) fn rewrite_async_generator_native(obj: &JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue) {
    let Some(state) = async_gen_state_mut(obj) else {
        return;
    };
    state.callee = rewrite(state.callee);
    for v in &mut state.args {
        *v = rewrite(*v);
    }
    state.result = rewrite(state.result);
    for req in state.queue.iter_mut() {
        req.promise = rewrite(req.promise);
        req.resolve = rewrite(req.resolve);
        req.reject = rewrite(req.reject);
    }
    if let Some(req) = &mut state.current {
        req.promise = rewrite(req.promise);
        req.resolve = rewrite(req.resolve);
        req.reject = rewrite(req.reject);
    }
    for v in state.regs.iter_mut() {
        *v = rewrite(*v);
    }
    if let Some(frame) = &mut state.frame {
        frame.saved_this = rewrite(frame.saved_this);
        frame.saved_new_target = rewrite(frame.saved_new_target);
        frame.callee = rewrite(frame.callee);
        frame.constructed_this = frame.constructed_this.map(&mut rewrite);
    }
    for v in &mut state.spill_stack {
        *v = rewrite(*v);
    }
    for v in &mut state.save_stack {
        *v = rewrite(*v);
    }
    for cells in &mut state.cell_stack {
        for &mut cell_ptr in cells.iter_mut() {
            if cell_ptr.is_null() {
                continue;
            }
            // SAFETY: cell 由 session_epoch 分配，本 session 内指针有效。
            let cell = unsafe { &mut *cell_ptr };
            cell.value = rewrite(cell.value);
        }
    }
    for v in &mut state.for_of_iters {
        *v = rewrite(*v);
    }
    state.last_for_of_result = rewrite(state.last_for_of_result);
    state.delegated_iterator = state.delegated_iterator.map(&mut rewrite);
}

/// 深拷贝状态盒到新对象（promote / sweep 用）：新对象持独立 Box，源盒可安全释放。
pub(crate) fn clone_async_generator_native_with_rewrite(
    old: &JsObject, new: &mut JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue,
) {
    let Some(state) = async_gen_state_mut(old) else {
        return;
    };
    let cloned = AsyncGeneratorState {
        phase: state.phase,
        callee: rewrite(state.callee),
        args: state.args.iter().copied().map(&mut rewrite).collect(),
        this_value: state.this_value,
        result: rewrite(state.result),
        queue: state
            .queue
            .iter()
            .map(|req| AsyncGenRequest {
                mode: req.mode,
                promise: rewrite(req.promise),
                resolve: rewrite(req.resolve),
                reject: rewrite(req.reject),
            })
            .collect(),
        current: state.current.as_ref().map(|req| AsyncGenRequest {
            mode: req.mode,
            promise: rewrite(req.promise),
            resolve: rewrite(req.resolve),
            reject: rewrite(req.reject),
        }),
        regs: Box::new({
            let mut regs = [JsValue::undefined(); 256];
            for (i, v) in state.regs.iter().enumerate() {
                regs[i] = rewrite(*v);
            }
            regs
        }),
        pc: state.pc,
        bytecode: state.bytecode.clone(),
        sub_idx: state.sub_idx,
        active_reg_limit: state.active_reg_limit,
        root_reg_limit: state.root_reg_limit,
        frame: state.frame.as_ref().map(|f| CallFrame {
            return_addr: f.return_addr,
            function_name: f.function_name,
            caller_reg_limit: f.caller_reg_limit,
            saved_reg_offset: f.saved_reg_offset,
            spill_offset: f.spill_offset,
            arguments_base: f.arguments_base,
            arguments_count: f.arguments_count,
            saved_this: rewrite(f.saved_this),
            saved_new_target: rewrite(f.saved_new_target),
            callee: rewrite(f.callee),
            construct_result_reg: f.construct_result_reg,
            constructed_this: f.constructed_this.map(&mut rewrite),
            is_derived_constructor: f.is_derived_constructor,
            continuation: f.continuation,
        }),
        spill_stack: state.spill_stack.iter().copied().map(&mut rewrite).collect(),
        save_stack: state.save_stack.iter().copied().map(&mut rewrite).collect(),
        cell_stack: state.cell_stack.clone(),
        try_stack: state.try_stack.clone(),
        for_in_iters: state.for_in_iters.clone(),
        for_of_iters: state.for_of_iters.iter().copied().map(&mut rewrite).collect(),
        last_for_of_result: rewrite(state.last_for_of_result),
        delegated_iterator: state.delegated_iterator.map(&mut rewrite),
        saved_bytecode_stack: state.saved_bytecode_stack.clone(),
        saved_immutables_stack: state.saved_immutables_stack.clone(),
        exception_value: state.exception_value.map(&mut rewrite),
        pending_exception: state.pending_exception.map(&mut rewrite),
        pending_error_kind: state.pending_error_kind,
        pending_completion: state.pending_completion,
    };
    new.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 释放异步生成器状态盒（对象被 GC 回收时），返回释放字节数。
pub(crate) fn drop_async_generator_native(obj: &JsObject) -> u64 {
    if !obj.is_async_generator_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut AsyncGeneratorState;
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: 指针来自 create_async_generator_object 的 Box::into_raw，只在 GC 回收时释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    std::mem::size_of::<AsyncGeneratorState>() as u64
        + state.bytecode.len() as u64 * std::mem::size_of::<u32>() as u64
        + state.spill_stack.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
        + state.save_stack.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
        + state.args.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
}
