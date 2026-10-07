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

use oxide_builtins::iterator::{is_callable, make_iter_result};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::object::{Cell, JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::generator::{DelegateOutcome, GeneratorResumeMode};
use crate::vm::{Completion, Vm};

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

/// `yield*` 委托协议步（异步生成器）：内层 next/return/throw 的 promise 结算后，
/// 步进取续闭包按步推进协议状态机。纯枚举（零 JsValue 字段，零 GC 面）：
/// 结算值经反应实参传递，跨微任务存活由 job 队列 GC 根保证。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DelegateStep {
    /// 等内层 next(v) 或 throw(e) 的 promise 结算（两者结算逻辑同构）。
    Inner,
    /// 等内层 return(v) 的 promise 结算（外层 return 请求）。
    Return,
    /// 等内层 return()（close）的 promise 结算（外层 throw 请求，内层无 throw 方法）。
    Close,
    /// 等请求值 promise 结算（外层 return 请求，内层无 return 方法，规范 Await 步）。
    ReturnAwait,
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
    /// 委托协议步：步进取续闭包消费后恢复；恢复入口恒为 None（纯枚举，零 GC 面）。
    pub delegate_pending: Option<DelegateStep>,
    /// 挂起时的执行上下文（regs/pc/bytecode/各栈段/迭代器/在途异常）。
    pub suspended: crate::suspended::SuspendedFrame,
}

/// 恢复闭包上区分 reject 角色的属性名（await 恢复闭包定位用）。
const AG_REJECT_PROP: &str = "__oxide_async_gen_reject__";
/// yield 值 unwrap 恢复闭包上存目标请求 promise 的属性名。
const AG_YIELD_PROMISE_PROP: &str = "__oxide_async_gen_yield_promise__";
/// yield 值 unwrap 恢复闭包上区分委托透传（raw）的属性名。
const AG_YIELD_RAW_PROP: &str = "__oxide_async_gen_yield_raw__";
/// 委托步进取续闭包上存协议步的属性名。
const AG_STEP_PROP: &str = "__oxide_async_gen_delegate_step__";

impl Vm {
    /// 调用异步生成器函数返回的迭代器对象：创建异步生成器实例、挂状态并执行
    /// 调用时参数初始化。
    ///
    /// 实例 [[Prototype]] = 调用方 `g.prototype`（为对象时），否则回退
    /// `%AsyncGeneratorPrototype%`（GetPrototypeFromConstructor 语义）。
    pub(crate) fn create_async_generator_object(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        let gen_proto_val = JsValue::from_js_object(self.realm.async_generator_proto.as_ptr() as *mut JsObject);
        let obj = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, gen_proto_val));
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
            delegate_pending: None,
            suspended: crate::suspended::SuspendedFrame::new_empty(),
        });
        obj_ref.set_native_data(Box::into_raw(state) as *mut u8);
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
        let saved = self.save_inline_state(256);
        self.generator_suspended = None;
        // 嵌套生成器创建（参数默认值里调用其它 generator）会改写 init 标志，须保存恢复。
        let prev_init_step = self.generator_init_step;
        let prev_body_started = self.generator_body_started;
        self.generator_body_started = false;
        self.generator_init_step = true;
        self.native_call_depth += 1;

        // 压入异步生成器帧（New）。
        let callee = unsafe { (*state_ptr).callee };
        let this_value = unsafe { (*state_ptr).this_value };
        let args = unsafe { std::mem::take(&mut (*state_ptr).args) };
        let push_res = self.prepare_execution_initial(callee, this_value, &args);
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
                oxide_builtins::error::create_from_text(self, &format!("async generator initialization failed: {v:?}"))
            }
            Err(e) => oxide_builtins::error::create_from_text(self, &e),
        });
        let kind = self.thrown_error_kind(exc);
        unsafe { (*state_ptr).phase = AsyncGenPhase::Completed };
        self.restore_inline_state(saved);
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(kind);
        self.unwind().map(|_| ())
    }

    /// 取出异步生成器对象的状态指针（调用方须先校验 `is_async_generator_obj`）。
    pub(crate) fn async_gen_state_ptr(&self, gen_val: JsValue) -> *mut AsyncGeneratorState {
        unsafe { (*gen_val.as_js_object_ptr()).native_data() as *mut AsyncGeneratorState }
    }

    /// 入队一个请求；生成器处于可恢复状态（New/YieldSuspended/Completed）时立即
    /// 启动处理，正在执行 / await 挂起时留待当前请求结算后再处理。
    ///
    /// 坏 receiver（非对象或非异步生成器对象）按规范不同步抛错，而是返回
    /// 携带 TypeError 的 rejected Promise。
    pub(crate) fn async_generator_enqueue(
        &mut self, gen_val: JsValue, mode: GeneratorResumeMode,
    ) -> Result<JsValue, JsValue> {
        let is_gen = if gen_val.is_object() {
            unsafe { &*gen_val.as_js_object_ptr() }.is_async_generator_obj()
        } else {
            false
        };
        if !is_gen {
            let (promise, _, _) = self.new_promise_capability();
            let exc = oxide_builtins::error::create_type_error(
                self,
                if gen_val.is_object() {
                    "AsyncGenerator methods called on incompatible receiver"
                } else {
                    "AsyncGenerator methods called on non-object"
                },
            );
            let _ = self.reject_promise(promise, exc);
            return Ok(promise);
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
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
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
        let saved = self.save_inline_state(256);
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
            let restore_res = state.suspended.restore_into(self, state.callee);
            if restore_res.is_err() {
                // callee 记录的表代际已被回收（跨 run 且无存活函数引用），无法恢复。
                self.async_gen_dispatch = prev_agd;
                self.async_gen_context = prev_gen_ctx;
                self.generator_dispatch = prev_gd;
                self.async_dispatch = prev_ad;
                self.async_context = prev_ctx;
                self.native_call_depth -= 1;
                self.restore_inline_state(saved);
                return Err("async generator suspended across runs is no longer valid".into());
            }
            state.phase = AsyncGenPhase::Running;
            // 入口不变量：步进取续闭包先消费协议步再调本函数。
            debug_assert!(state.delegate_pending.is_none());
        }

        // 委托恢复：把 next/return/throw 请求转发给内层异步迭代器，登记步进取续
        // 闭包等 promise 结算后经快照分支挂起（AwaitSuspended）。
        if self.delegated_iterator.is_some() {
            let mode = {
                let state = unsafe { &mut *state_ptr };
                state
                    .current
                    .as_ref()
                    .map(|c| c.mode)
                    .unwrap_or(GeneratorResumeMode::Next(JsValue::undefined()))
            };
            let forwarded = self.delegate_forward_async(mode, gen_val);
            match forwarded {
                Err(e) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                    self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                    return self.finish_async_gen_exc(state_ptr, saved, gen_val, exc);
                }
                Ok(DelegateOutcome::Suspend { .. }) => {
                    // 协议步等待（delegate_pending 已置）：内层 promise 结算前 body 不得
                    // 继续，快照挂起状态后返回，等步进取续闭包（微任务）恢复。本分支在
                    // dispatch 之前返回，调度标志须在此恢复（dispatch 后的恢复不会执行）。
                    unsafe { (*state_ptr).phase = AsyncGenPhase::AwaitSuspended };
                    self.snapshot_async_generator(state_ptr)?;
                    self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                    self.restore_inline_state(saved);
                    return Ok(());
                }
                Ok(DelegateOutcome::Continue { value }) => {
                    self.regs[0] = value;
                }
                Ok(DelegateOutcome::Complete { value }) => {
                    // 共享枚举保留同步路径的 Complete 结局；委托转发现已不产生
                    // （return 请求值经 Await 步挂起结算），此臂仅作穷尽防御。
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
        self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
        self.post_dispatch_async_gen(state_ptr, saved, gen_val, result)
    }

    /// 异步生成器恢复的 dispatch 后处理：YIELD 让出（快照 + 让出值 unwrap 结算）、
    /// AWAIT 挂起（快照）、完成/异常（结算当前请求）。
    ///
    /// # 前提
    /// 调度标志已恢复（调用方先恢复再调本函数）；本函数内部按结果快照新状态并
    /// 恢复调用方内联状态。
    fn post_dispatch_async_gen(
        &mut self, state_ptr: *mut AsyncGeneratorState, saved: Box<crate::vm::InlineSyncState>, gen_val: JsValue,
        result: Result<JsValue, String>,
    ) -> Result<(), String> {
        // YIELD 让出：快照挂起状态，让出值经 promise unwrap 后结算当前请求。
        if let Some(value) = self.generator_suspended.take() {
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
            // yield 值 unwrap 前视为 await 挂起（不可由新 next 恢复）：AsyncGeneratorYield
            // 先 Await(value)，unwrap 完成前排队请求不得恢复生成器——否则被拒 yield 值
            // 尚未结算，排队 next 会错误地继续执行。
            unsafe { (*state_ptr).phase = AsyncGenPhase::AwaitSuspended };
            self.snapshot_async_generator(state_ptr)?;
            self.restore_inline_state(saved);
            if let Some(req) = request {
                let fulfill_fn = self.make_async_gen_yield_unwrap_fn(gen_val, req.promise, false, false);
                let reject_fn = self.make_async_gen_yield_unwrap_fn(gen_val, req.promise, false, true);
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
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                self.finish_async_gen_exc(state_ptr, saved, gen_val, exc)
            }
        }
    }

    /// `yield*` 委托单步转发（异步生成器）：把外层请求转发给内层异步迭代器，
    /// 登记步进取续闭包等 promise 结算。
    ///
    /// # 步骤
    /// 1. Next/Throw：取内层 next/throw 方法并以请求值调用；无 throw 方法时回退
    ///    内层 return()（无参 close），再无 return 方法时抛 TypeError 入 body。
    /// 2. Return：取内层 return 方法；缺失时执行规范 Await 步（请求值经
    ///    PromiseResolve 包装后登记恢复闭包挂起等结算，结算后以展开值完成）；
    ///    可调用时调用。
    /// 3. 调用结果 promise 直通或 PromiseResolve 包装，登记步进取续闭包
    ///    （Inner/Return/Close），置 `delegate_pending`。
    ///
    /// # 返回值
    /// - `Ok(Suspend)`：协议步等待（`delegate_pending` 已置，调用方经快照分支挂起）；
    /// - `Ok(Unwind)`：TypeError 已抛入 body（内层无 throw/return 方法），继续 dispatch；
    /// - `Err`：内层调用同步抛错，调用方走既有 Err 通道。
    fn delegate_forward_async(
        &mut self, mode: GeneratorResumeMode, gen_val: JsValue,
    ) -> Result<DelegateOutcome, String> {
        let iterator = match self.delegated_iterator {
            Some(it) => it,
            None => return Ok(DelegateOutcome::Unwind),
        };
        let (method, arg, mut step) = match mode {
            GeneratorResumeMode::Next(v) => ("next", v, DelegateStep::Inner),
            GeneratorResumeMode::Throw(e) => ("throw", e, DelegateStep::Inner),
            GeneratorResumeMode::Return(v) => ("return", v, DelegateStep::Return),
        };
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let method_si = self.kernel_core.perm_interner().intern(method).0;
        let method_fn = self.ordinary_get(iter_obj, method_si, iterator)?;
        let inner_result = if is_callable(method_fn) {
            self.call_function_sync(method_fn, iterator, &[arg])?
        } else {
            match mode {
                GeneratorResumeMode::Next(_) => {
                    return Err(self.error_message_text("TypeError", "iterator.next is not callable"));
                }
                GeneratorResumeMode::Return(v) => {
                    // 内层无 return 方法：规范 return 分支的 Await 步——完成值是
                    // 请求值经 Await 展开后的值（原生 promise 直通，其余经
                    // PromiseResolve 包装），登记恢复闭包挂起等结算。
                    let value_promise = if self.is_promise_value(v) {
                        v
                    } else {
                        match self.promise_resolve(v) {
                            Ok(p) => p,
                            Err(exc) => {
                                self.last_uncaught_value = Some(exc);
                                return Err(String::new());
                            }
                        }
                    };
                    let fulfill_fn = self.make_delegate_return_await_closure(gen_val, false);
                    let reject_fn = self.make_delegate_return_await_closure(gen_val, true);
                    let _ = self.perform_promise_then(value_promise, fulfill_fn, reject_fn);
                    self.delegated_iterator = Some(iterator);
                    let state = unsafe { &mut *self.async_gen_state_ptr(gen_val) };
                    state.delegate_pending = Some(DelegateStep::ReturnAwait);
                    return Ok(DelegateOutcome::Suspend { value: JsValue::undefined() });
                }
                GeneratorResumeMode::Throw(_) => {
                    // 内层无 throw 方法：回退内层 return()（无参 close），步为 Close。
                    let ret_si = self.kernel_core.perm_interner().intern("return").0;
                    let ret_fn = self.ordinary_get(iter_obj, ret_si, iterator)?;
                    if !is_callable(ret_fn) {
                        // 内层亦无 return 方法：close 为 no-op，抛 TypeError 入 body（协议违规）。
                        self.delegated_iterator = None;
                        let exc = oxide_builtins::error::create_type_error(
                            self,
                            "yield* protocol violation: iterator does not have a throw method",
                        );
                        self.exception_value = Some(exc);
                        self.pending_error_kind = Some(self.thrown_error_kind(exc));
                        return match self.unwind() {
                            Ok(()) => Ok(DelegateOutcome::Unwind),
                            Err(e) => Err(e),
                        };
                    }
                    // 步为 Close：close 成功后抛 TypeError 入 body，拒绝则抛拒绝原因入 body。
                    step = DelegateStep::Close;
                    self.call_function_sync(ret_fn, iterator, &[])?
                }
            }
        };
        // Await：结果 promise 直通，非 promise 经 PromiseResolve 包装。
        let result_promise = if self.is_promise_value(inner_result) {
            inner_result
        } else {
            match self.promise_resolve(inner_result) {
                Ok(p) => p,
                Err(exc) => {
                    self.last_uncaught_value = Some(exc);
                    return Err(String::new());
                }
            }
        };
        // 登记步进取续闭包；先 Call 再置 pending（同步抛错不悬挂）。
        let fulfill_fn = self.make_delegate_step_closure(gen_val, step, false);
        let reject_fn = self.make_delegate_step_closure(gen_val, step, true);
        let _ = self.perform_promise_then(result_promise, fulfill_fn, reject_fn);
        self.delegated_iterator = Some(iterator);
        let state = unsafe { &mut *self.async_gen_state_ptr(gen_val) };
        state.delegate_pending = Some(step);
        Ok(DelegateOutcome::Suspend { value: JsValue::undefined() })
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
    /// 异步生成器帧弹出存 `suspended.frame`，其余栈段整段搬入（嵌套循环期间这些栈
    /// 只含异步生成器数据）。请求队列与当前请求保留在状态盒中不动。
    fn snapshot_async_generator(&mut self, state_ptr: *mut AsyncGeneratorState) -> Result<(), String> {
        let state = unsafe { &mut *state_ptr };
        state.suspended.save_from(self, state.callee)?;
        Ok(())
    }

    /// 构造 await 恢复闭包：携带目标异步生成器上下文对象，区分 fulfill/reject 角色。
    pub(crate) fn make_async_gen_await_resume_fn(&mut self, ctx: JsValue, reject_role: bool) -> JsValue {
        let fn_proto = self.realm.session.builtin_world().fn_proto_val();
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
        let fn_proto = self.realm.session.builtin_world().fn_proto_val();
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

    /// 构造委托步进取续闭包：携带目标异步生成器上下文、协议步与 fulfill/reject 角色。
    pub(crate) fn make_delegate_step_closure(
        &mut self, ctx: JsValue, step: DelegateStep, reject_role: bool,
    ) -> JsValue {
        let fn_proto = self.realm.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: async_gen_delegate_step_closure 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_gen_delegate_step_closure as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        let ctx_si = self.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
        self.set_or_create_prop_value(obj, ctx_si, ctx);
        let step_si = self.kernel_core.perm_interner().intern(AG_STEP_PROP).0;
        self.set_or_create_prop_value(obj, step_si, JsValue::int(step as i32));
        let role_si = self.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, role_si, JsValue::bool(reject_role));
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }

    /// 构造委托 return 请求值 Await 恢复闭包：携带目标异步生成器上下文与
    /// fulfill/reject 角色，结算后以展开值完成生成器（拒绝时以拒绝原因终止）。
    fn make_delegate_return_await_closure(&mut self, ctx: JsValue, reject_role: bool) -> JsValue {
        let fn_proto = self.realm.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: async_gen_delegate_return_await_closure 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe {
            NativeFnPtr::from_raw(async_gen_delegate_return_await_closure as *const ())
        }));
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

    /// 构造逃出关闭结算闭包（异步生成器）：携带目标异步生成器上下文对象与
    /// return() promise，区分 fulfill/reject 角色。
    pub(crate) fn make_async_gen_escape_close_fn(
        &mut self, ctx: JsValue, promise: JsValue, reject_role: bool,
    ) -> JsValue {
        let fn_proto = self.realm.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: async_gen_escape_closure 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_gen_escape_closure as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        let ctx_si = self.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
        self.set_or_create_prop_value(obj, ctx_si, ctx);
        let role_si = self.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, role_si, JsValue::bool(reject_role));
        let promise_si = self
            .kernel_core
            .perm_interner()
            .intern(crate::async_func::ASYNC_ESCAPE_PROMISE_PROP)
            .0;
        self.set_or_create_prop_value(obj, promise_si, promise);
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }

    /// 恢复逃出 for-await-of 的异步生成器：return() 的 promise 结算后继续关闭剩余
    /// 条目并执行完成。
    ///
    /// # 步骤
    /// 1. 陈旧防御：状态盒 pending_async_escape.close_promise 须与本闭包 promise
    ///    匹配，不匹配则不处理（防同一 promise 被双结算）。
    /// 2. 把挂起快照灌回 VM，取出完成与剩余待关条目并清除 pending_async_escape。
    /// 3. 多层逃出：remaining 非空时逐层关闭（每层一轮结算），空了才执行完成。
    /// 4. 执行完成：Return 走 do_return 完成生成器，Break/Continue 跳转 target_pc
    ///    续 dispatch。
    /// 5. 再挂起则快照新状态；完成/异常则结算当前请求。恢复调用方状态。
    ///
    /// # 边界与前提
    /// - 关闭 promise 拒绝时（reject 角色），拒绝值替代在途完成：抛拒绝原因，
    ///   外围 catch/finally 接管；无处理器时当前请求以拒绝原因拒绝。
    pub(crate) fn resume_async_gen_escape(
        &mut self, ctx: JsValue, promise: JsValue, is_reject: bool, value: JsValue,
    ) -> Result<(), String> {
        let state_ptr = self.async_gen_state_ptr(ctx);
        let saved = self.save_inline_state(256);
        // 清上一轮多层关闭残留的挂起信号（多层逐层结算每轮清零，防陈旧信号误消费）。
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
        self.async_context = Some(ctx);
        self.async_gen_context = Some(ctx);
        self.native_call_depth += 1;

        {
            let state = unsafe { &mut *state_ptr };
            // 陈旧防御：close_promise 须匹配，陈旧闭包不处理。
            let is_stale = !matches!(
                state.suspended.pending_async_escape.as_ref(),
                Some(p) if p.close_promise == promise
            );
            if is_stale {
                self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                self.restore_inline_state(saved);
                return Ok(());
            }
            let restore_res = state.suspended.restore_into(self, state.callee);
            if restore_res.is_err() {
                self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                self.restore_inline_state(saved);
                return Err("async generator suspended across runs is no longer valid".into());
            }
            state.phase = AsyncGenPhase::Running;
        }

        // 取出完成与剩余待关条目并清除 pending_async_escape。
        let (completion, mut remaining) = match self.pending_async_escape.take() {
            Some(p) => (p.completion, p.remaining),
            // 陈旧防御已早退，正常路径此处必为 Some；防御性兜底。
            None => {
                self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                self.restore_inline_state(saved);
                return Ok(());
            }
        };

        // 多层逃出：remaining 非空时逐层关闭（每层一轮结算），空了才执行完成
        // （或抛拒绝原因）。挂起信号由 register 置位，快照后返回续下一轮结算；
        // 该信号由下一轮结算闭包消费、不经派发循环，须先清防陈旧信号泄漏进
        // 另一上下文的 dispatch。
        while let Some(next_iter) = remaining.first().cloned() {
            remaining.remove(0);
            match self.register_async_escape_close(next_iter, completion, remaining.clone()) {
                Ok(true) => {
                    self.async_gen_suspended = false;
                    unsafe { (*state_ptr).phase = AsyncGenPhase::AwaitSuspended };
                    self.snapshot_async_generator(state_ptr)?;
                    self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                    self.restore_inline_state(saved);
                    return Ok(());
                }
                Ok(false) => continue,
                Err(e) => return Err(e),
            }
        }

        // 关闭拒绝：拒绝值替代在途完成。抛拒绝原因，外围 catch/finally 接管；
        // 无处理器时当前请求以拒绝原因拒绝。
        if is_reject {
            self.exception_value = Some(value);
            self.pending_error_kind = Some(self.thrown_error_kind(value));
            if self.unwind().is_err() {
                let exc = self.last_uncaught_value.take().unwrap_or(value);
                self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                return self.finish_async_gen_exc(state_ptr, saved, ctx, exc);
            }
            let result = self.dispatch();
            self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
            return self.post_dispatch_async_gen(state_ptr, saved, ctx, result);
        }

        // remaining 已空，执行完成。
        match completion {
            Completion::Return { value: ret_val, .. } => {
                let result = self.do_return(ret_val)?;
                if let Some(result) = result {
                    self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                    return self.finish_async_gen_ok(state_ptr, saved, ctx, result);
                }
                // 嵌套帧未弹空：续 dispatch。
                let result = self.dispatch();
                self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                self.post_dispatch_async_gen(state_ptr, saved, ctx, result)
            }
            Completion::Break { target_pc, .. } | Completion::Continue { target_pc, .. } => {
                self.pc = target_pc;
                let result = self.dispatch();
                self.restore_async_gen_flags(prev_ctx, prev_gen_ctx, prev_gd, prev_ad, prev_agd);
                self.post_dispatch_async_gen(state_ptr, saved, ctx, result)
            }
        }
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
        .is_some_and(oxide_runtime_api::to_boolean);
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
        Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
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
        .is_some_and(oxide_runtime_api::to_boolean);
    let role_si = vm.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, role_si)
        .is_some_and(oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if is_reject {
        // yield 值被拒：next() 的 promise reject，且生成器关闭（后续 next 直接 done）。
        let _ = vm.reject_promise(promise, value);
        if ctx.is_object() {
            let obj = unsafe { &*ctx.as_js_object_ptr() };
            if obj.is_async_generator_obj() {
                let state = vm.async_gen_state_ptr(ctx);
                // 仅在仍挂起在 yield 点（unwrap 前 AwaitSuspended，未被新 next 恢复）时关闭。
                if unsafe { (*state).phase } == AsyncGenPhase::AwaitSuspended {
                    unsafe { (*state).phase = AsyncGenPhase::Completed };
                    unsafe { (*state).result = JsValue::undefined() };
                }
            }
        }
    } else {
        // yield 值 unwrap 完成：恢复可让出状态（async_generator_start 在 AwaitSuspended
        // 下不启动，须先转回 YieldSuspended 才能恢复执行继续 yield/完成）。
        if ctx.is_object() {
            let obj = unsafe { &*ctx.as_js_object_ptr() };
            if obj.is_async_generator_obj() {
                let state = vm.async_gen_state_ptr(ctx);
                if unsafe { (*state).phase } == AsyncGenPhase::AwaitSuspended {
                    unsafe { (*state).phase = AsyncGenPhase::YieldSuspended };
                }
            }
        }
        let result = if raw { value } else { make_iter_result(vm, value, false) };
        let _ = vm.resolve_promise(promise, result);
    }
    if ctx.is_object() {
        let _ = vm.async_generator_start(ctx);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// 委托步进取续闭包：内层 next/return/throw 的 promise 结算时推进委托协议一步。
///
/// 读自身 prop 的协议步与角色，校验状态盒 `delegate_pending` 匹配（防陈旧闭包），
/// 再按步分支（规范：`yield*` 交付值不做二次 Await，失败一律抛入 body 可 catch）：
/// - Inner：结算值判 done——done 时委托值注入 body 恢复；未 done 时委托值原样
///   让出（`{value, done:false}` 结算当前请求）；非对象或拒绝原因抛入 body。
/// - Return：done 时以委托值完成生成器（return completion）；未 done 时委托值
///   原样让出；拒绝原因 / 非对象抛入 body。
/// - Close：close 结算后抛 TypeError 入 body（协议违规）；close 拒绝则抛拒绝原因。
fn async_gen_delegate_step_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "delegate step handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let ctx_si = vm.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
    let ctx = vm.resolve_property(callee_obj, ctx_si).unwrap_or(JsValue::undefined());
    let step_si = vm.kernel_core.perm_interner().intern(AG_STEP_PROP).0;
    let step = match vm.resolve_property(callee_obj, step_si) {
        Some(v) if v.is_int() => v.as_int(),
        _ => return NativeResult::Ok(JsValue::undefined()),
    };
    let step = match step {
        0 => DelegateStep::Inner,
        1 => DelegateStep::Return,
        2 => DelegateStep::Close,
        _ => return NativeResult::Ok(JsValue::undefined()),
    };
    let role_si = vm.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, role_si)
        .is_some_and(oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !ctx.is_object() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let obj = unsafe { &*ctx.as_js_object_ptr() };
    if !obj.is_async_generator_obj() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let state_ptr = vm.async_gen_state_ptr(ctx);
    // 陈旧防御：状态盒的在途协议步必须与本闭包一致。
    if unsafe { (*state_ptr).delegate_pending } != Some(step) {
        return NativeResult::Ok(JsValue::undefined());
    }
    match step {
        DelegateStep::Inner => {
            // 失败子路径：拒绝原因 / 非对象结算值 / done 或 value getter 抛错，
            // 均抛入 body（body 的 try/catch 可捕获）。
            let failure = if is_reject {
                Some(value)
            } else if !value.is_object() {
                Some(oxide_builtins::error::create_type_error(vm, "iterator result is not an object"))
            } else {
                None
            };
            if let Some(exc) = failure {
                throw_delegate_into_body(vm, ctx, state_ptr, exc);
                return NativeResult::Ok(JsValue::undefined());
            }
            let result_obj = unsafe { &*value.as_js_object_ptr() };
            let done_si = vm.kernel_core.perm_interner().intern("done").0;
            let done = match vm.ordinary_get(result_obj, done_si, value) {
                Ok(d) => oxide_runtime_api::to_boolean(d),
                Err(e) => {
                    let exc = vm
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
                    throw_delegate_into_body(vm, ctx, state_ptr, exc);
                    return NativeResult::Ok(JsValue::undefined());
                }
            };
            let value_si = vm.kernel_core.perm_interner().intern("value").0;
            let inner_value = match vm.ordinary_get(result_obj, value_si, value) {
                Ok(v) => v,
                Err(e) => {
                    let exc = vm
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
                    throw_delegate_into_body(vm, ctx, state_ptr, exc);
                    return NativeResult::Ok(JsValue::undefined());
                }
            };
            if done {
                // 委托 done：委托值注入 body，body 从 yield* 续点继续。清挂起帧的
                // 委托迭代器（restore_into 会把它写回 VM，不清则恢复后再走 S0 转发）。
                let state = unsafe { &mut *state_ptr };
                if let Some(req) = state.current.as_mut() {
                    req.mode = GeneratorResumeMode::Next(inner_value);
                }
                state.delegate_pending = None;
                state.suspended.delegated_iterator = None;
                match vm.resume_async_generator(ctx) {
                    Ok(()) => NativeResult::Ok(JsValue::undefined()),
                    Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
                }
            } else {
                // 委托未 done：交付值原样让出（规范 yield* 不对值 Await），委托持续。
                delegate_yield_raw(vm, ctx, state_ptr, inner_value);
                NativeResult::Ok(JsValue::undefined())
            }
        }
        DelegateStep::Return => {
            // return 分支：拒绝原因 / 非对象结算值 / getter 抛错均抛入 body；
            // done:true 以委托值完成生成器；done:false 原样让出委托值。
            let (done, inner_value) = match read_delegate_result(vm, value) {
                DelegateResult::Failure(exc) => {
                    throw_delegate_into_body(vm, ctx, state_ptr, exc);
                    return NativeResult::Ok(JsValue::undefined());
                }
                DelegateResult::Ok(done, inner_value) => (done, inner_value),
            };
            if done {
                // 以委托值完成（return completion，finally 穿越）：改写请求模式为
                // Return(委托值) 后恢复，注入路径走 complete_generator_return。
                let state = unsafe { &mut *state_ptr };
                if let Some(req) = state.current.as_mut() {
                    req.mode = GeneratorResumeMode::Return(inner_value);
                }
                state.delegate_pending = None;
                state.suspended.delegated_iterator = None;
                match vm.resume_async_generator(ctx) {
                    Ok(()) => NativeResult::Ok(JsValue::undefined()),
                    Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
                }
            } else {
                delegate_yield_raw(vm, ctx, state_ptr, inner_value);
                NativeResult::Ok(JsValue::undefined())
            }
        }
        DelegateStep::Close => {
            // close 分支（内层无 throw 方法）：close 结算后抛 TypeError 入 body
            // （协议违规）；close 拒绝则抛拒绝原因入 body；close 结果非对象则抛
            // IteratorResult TypeError（与同步路径一致）。
            let exc = if is_reject {
                value
            } else if !value.is_object() {
                oxide_builtins::error::create_type_error(vm, "IteratorResult is not an object")
            } else {
                oxide_builtins::error::create_type_error(
                    vm,
                    "yield* protocol violation: iterator does not have a throw method",
                )
            };
            throw_delegate_into_body(vm, ctx, state_ptr, exc);
            NativeResult::Ok(JsValue::undefined())
        }
        DelegateStep::ReturnAwait => {
            // 该步由专用恢复闭包消费，步进取续闭包不处理（陈旧防御）。
            NativeResult::Ok(JsValue::undefined())
        }
    }
}

/// 委托 return 请求值 Await 恢复闭包：内层无 return 方法时，请求值 promise 结算后
/// 以展开值完成生成器（规范 yield* 评估 return 分支的 Await 步）；拒绝时以拒绝
/// 原因终止生成器。
fn async_gen_delegate_return_await_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(
            vm,
            "delegate return await handler is invalid",
        ));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let ctx_si = vm.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
    let ctx = vm.resolve_property(callee_obj, ctx_si).unwrap_or(JsValue::undefined());
    let role_si = vm.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, role_si)
        .is_some_and(oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !ctx.is_object() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let obj = unsafe { &*ctx.as_js_object_ptr() };
    if !obj.is_async_generator_obj() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let state_ptr = vm.async_gen_state_ptr(ctx);
    // 陈旧防御：状态盒的在途协议步必须与本闭包一致。
    if unsafe { (*state_ptr).delegate_pending } != Some(DelegateStep::ReturnAwait) {
        return NativeResult::Ok(JsValue::undefined());
    }
    let state = unsafe { &mut *state_ptr };
    if let Some(req) = state.current.as_mut() {
        req.mode = if is_reject {
            GeneratorResumeMode::Throw(value)
        } else {
            GeneratorResumeMode::Return(value)
        };
    }
    state.delegate_pending = None;
    // 清挂起帧的委托迭代器（restore_into 会把它写回 VM，不清则恢复后再走 S0 转发）。
    state.suspended.delegated_iterator = None;
    match vm.resume_async_generator(ctx) {
        Ok(()) => NativeResult::Ok(JsValue::undefined()),
        Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
    }
}

/// 逃出关闭结算闭包（异步生成器）：读自身 prop 的异步生成器上下文、return()
/// promise 与角色，恢复挂起帧并继续关闭剩余条目、执行完成。
fn async_gen_escape_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "escape close handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let ctx_si = vm.kernel_core.perm_interner().intern(crate::async_func::ASYNC_CTX_PROP).0;
    let ctx = vm.resolve_property(callee_obj, ctx_si).unwrap_or(JsValue::undefined());
    let role_si = vm.kernel_core.perm_interner().intern(AG_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, role_si)
        .is_some_and(oxide_runtime_api::to_boolean);
    let promise_si = vm
        .kernel_core
        .perm_interner()
        .intern(crate::async_func::ASYNC_ESCAPE_PROMISE_PROP)
        .0;
    let promise = vm.resolve_property(callee_obj, promise_si).unwrap_or(JsValue::undefined());
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match vm.resume_async_gen_escape(ctx, promise, is_reject, value) {
        Ok(()) => NativeResult::Ok(JsValue::undefined()),
        Err(e) => NativeResult::Err(oxide_builtins::error::create_from_text(vm, &e)),
    }
}

/// 委托结算值读取：done 与 value 的 getter 抛错经 `last_uncaught_value` 取原值。
enum DelegateResult {
    /// 结算值读取成功（done 标志与委托值）。
    Ok(bool, JsValue),
    /// 结算值读取失败（须抛入 body 的异常值）。
    Failure(JsValue),
}

fn read_delegate_result(vm: &mut Vm, value: JsValue) -> DelegateResult {
    if !value.is_object() {
        return DelegateResult::Failure(oxide_builtins::error::create_type_error(
            vm,
            "iterator result is not an object",
        ));
    }
    let result_obj = unsafe { &*value.as_js_object_ptr() };
    let done_si = vm.kernel_core.perm_interner().intern("done").0;
    let done = match vm.ordinary_get(result_obj, done_si, value) {
        Ok(d) => oxide_runtime_api::to_boolean(d),
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            return DelegateResult::Failure(exc);
        }
    };
    let value_si = vm.kernel_core.perm_interner().intern("value").0;
    let inner_value = match vm.ordinary_get(result_obj, value_si, value) {
        Ok(v) => v,
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            return DelegateResult::Failure(exc);
        }
    };
    DelegateResult::Ok(done, inner_value)
}

/// 委托让出：内层值原样交付（规范 `yield*` 不对交付值 Await），以 `{value, done:false}`
/// 结算当前请求、恢复可让出状态、驱动队列。
///
/// 微任务上下文专用：不恢复生成器执行（生成器已让出，等待下一个 next/return/throw）。
fn delegate_yield_raw(vm: &mut Vm, ctx: JsValue, state_ptr: *mut AsyncGeneratorState, value: JsValue) {
    {
        let state = unsafe { &mut *state_ptr };
        state.delegate_pending = None;
        if unsafe { (*state_ptr).phase } == AsyncGenPhase::AwaitSuspended {
            unsafe { (*state_ptr).phase = AsyncGenPhase::YieldSuspended };
        }
    }
    let request = unsafe { (*state_ptr).current.take() };
    if let Some(req) = request {
        let result = make_iter_result(vm, value, false);
        let _ = vm.resolve_promise(req.promise, result);
    }
    if !unsafe { (*state_ptr).queue.is_empty() } {
        let _ = vm.async_generator_start(ctx);
    }
}

/// 把委托协议异常抛入 body：当前请求改写为 Throw(exc)、清协议状态后恢复
/// （body 的 catch 可捕获；无 catch 时既有 finish 通道 reject 请求并完成）。
fn throw_delegate_into_body(vm: &mut Vm, ctx: JsValue, state_ptr: *mut AsyncGeneratorState, exc: JsValue) {
    {
        let state = unsafe { &mut *state_ptr };
        if let Some(req) = state.current.as_mut() {
            req.mode = GeneratorResumeMode::Throw(exc);
        }
        state.delegate_pending = None;
        // 清挂起帧的委托迭代器（restore_into 会把它写回 VM，不清则恢复后再走 S0 转发）。
        state.suspended.delegated_iterator = None;
    }
    let _ = vm.resume_async_generator(ctx);
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

/// `@@asyncDispose` 结算闭包上存目标能力 promise 的属性名。
const AD_SETTLE_PROMISE_PROP: &str = "__oxide_async_dispose_promise__";
/// `@@asyncDispose` 结算闭包上区分 fulfill/reject 角色的属性名。
const AD_SETTLE_ROLE_PROP: &str = "__oxide_async_dispose_role__";

/// `%AsyncIteratorPrototype%[@@asyncDispose]`：新建能力，取 `this` 的 `return`
/// 方法调用后，把结果经 PromiseResolve 展开、以 undefined 结算能力。
///
/// # 步骤
/// 1. GetMethod(this, "return")：非对象接收者或方法缺失/非对象时按无方法处理。
/// 2. 无方法：能力以 undefined resolve 后立即返回。
/// 3. Call(return, this, « undefined »)：调用失败以原异常 reject 能力。
/// 4. 结果为 promise 时注册 unwrap 反应（最终值恒 undefined）；非 promise 直接
///    以 undefined resolve 能力。
///
/// # 边界与前提
/// - 本方法返回的 promise 永不同步抛错：所有异常路径都转为能力 reject。
///
/// # 副作用
/// - 新建能力 promise 与两个结算闭包（登记进 session GC 根）。
/// - 调用用户 `return` 方法（可能执行任意用户代码）。
pub(crate) fn async_iterator_async_dispose(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let o = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let (promise, _, _) = vm.new_promise_capability();

    // GetMethod(O, "return")：完整 [[Get]]（触发 getter，getter 抛错经
    // IfAbruptRejectPromise 转能力 reject）。
    let ret = if o.is_object() {
        let si = vm.kernel_core.perm_interner().intern("return").0;
        let obj = unsafe { &*o.as_js_object_ptr() };
        match vm.ordinary_get(obj, si, o) {
            Ok(r) => Some(r),
            Err(_) => {
                let exc = vm.take_uncaught_value().unwrap_or_else(|| {
                    oxide_builtins::error::create_from_text(vm, "AsyncIterator return getter failed")
                });
                let _ = vm.reject_promise(promise, exc);
                return NativeResult::Ok(promise);
            }
        }
    } else {
        None
    };
    let ret = match ret {
        Some(r) if r.is_object() => r,
        _ => {
            let _ = vm.resolve_promise(promise, JsValue::undefined());
            return NativeResult::Ok(promise);
        }
    };

    // Call(return, O, « undefined »)：调用失败以原异常 reject 能力。
    let result = match vm.call_function_sync(ret, o, &[JsValue::undefined()]) {
        Ok(r) => r,
        Err(_) => {
            let exc = vm.take_uncaught_value().unwrap_or_else(|| {
                oxide_builtins::error::create_from_text(vm, "AsyncIterator return method call failed")
            });
            let _ = vm.reject_promise(promise, exc);
            return NativeResult::Ok(promise);
        }
    };

    // PromiseResolve + PerformPromiseThen(resultWrapper, unwrap, undefined, capability)：
    // unwrap 闭包恒返回 undefined，故能力最终值恒为 undefined。
    if vm.is_promise_value(result) {
        let unwrap = vm.make_async_dispose_unwrap_fn();
        let derived = match vm.perform_promise_then(result, unwrap, JsValue::undefined()) {
            Ok(d) => d,
            Err(exc) => {
                let _ = vm.reject_promise(promise, exc);
                return NativeResult::Ok(promise);
            }
        };
        let on_fulfilled = vm.make_async_dispose_settle_fn(promise, false);
        let on_rejected = vm.make_async_dispose_settle_fn(promise, true);
        let _ = vm.perform_promise_then(derived, on_fulfilled, on_rejected);
    } else {
        let _ = vm.resolve_promise(promise, JsValue::undefined());
    }
    NativeResult::Ok(promise)
}

/// unwrap 闭包：忽略反应值，恒返回 undefined（规范 CreateBuiltinFunction(unwrap, 1, "", « »)）。
fn async_dispose_unwrap_closure(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
    NativeResult::Ok(JsValue::undefined())
}

/// 结算闭包：以反应值结算自身 prop 携带的能力 promise（角色 prop 区分 fulfill/reject）。
fn async_dispose_settle_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "settle handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let prom_si = vm.kernel_core.perm_interner().intern(AD_SETTLE_PROMISE_PROP).0;
    let promise = vm.resolve_property(callee_obj, prom_si).unwrap_or(JsValue::undefined());
    let role_si = vm.kernel_core.perm_interner().intern(AD_SETTLE_ROLE_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, role_si)
        .is_some_and(oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if is_reject {
        let _ = vm.reject_promise(promise, value);
    } else {
        let _ = vm.resolve_promise(promise, value);
    }
    NativeResult::Ok(JsValue::undefined())
}

impl Vm {
    /// 构造 `@@asyncDispose` 的 unwrap 闭包（无捕获状态，恒返回 undefined）。
    fn make_async_dispose_unwrap_fn(&mut self) -> JsValue {
        let fn_proto = self.realm.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: async_dispose_unwrap_closure 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_dispose_unwrap_closure as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        self.add_fn_name_length(unsafe { &mut *ptr }, "", 1);
        JsValue::from_js_object(ptr)
    }

    /// 构造 `@@asyncDispose` 结算闭包：捕获能力 promise 与结算角色，
    /// 反应触发时以反应值结算能力 promise。
    fn make_async_dispose_settle_fn(&mut self, promise: JsValue, reject_role: bool) -> JsValue {
        let fn_proto = self.realm.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: async_dispose_settle_closure 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_dispose_settle_closure as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        let prom_si = self.kernel_core.perm_interner().intern(AD_SETTLE_PROMISE_PROP).0;
        self.set_or_create_prop_value(obj, prom_si, promise);
        let role_si = self.kernel_core.perm_interner().intern(AD_SETTLE_ROLE_PROP).0;
        self.set_or_create_prop_value(obj, role_si, JsValue::bool(reject_role));
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }
}

/// 初始化/重建异步生成器内建对象：`%AsyncGeneratorPrototype%`、
/// `%AsyncGeneratorFunction.prototype%` 与占位 `%AsyncGeneratorFunction%`。
///
/// 两个原型对象存于 VM 字段（session 生命周期），`full_reset` 后重建。
pub(crate) fn init_async_generator_intrinsics(vm: &mut Vm) {
    let sf = vm.kernel_core.perm_interner().as_ref();
    let sh = vm.kernel_core.shape_forge().as_ref();
    let fn_proto_val = vm.realm.session.builtin_world().fn_proto_val();
    let world = vm.realm.session.builtin_world();
    // %AsyncGeneratorPrototype% 链到 %AsyncIteratorPrototype%（规范原型链），
    // 方法 next/return/throw 挂其自身。
    let async_iterator_proto_val = JsValue::from_js_object(world.async_iterator_proto.as_ptr() as *mut JsObject);

    // %AsyncGeneratorPrototype%：proto = %AsyncIteratorPrototype%。
    let mut ag_proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, async_iterator_proto_val));
    // 站点标签：与 Generator 原型的同名方法槽（next/return/throw）区分复用键。
    let ag_label = sf.intern("AsyncGeneratorPrototype").0;
    oxide_kernel::bind_methods_static!(
        &mut ag_proto,
        sf,
        sh,
        world,
        ag_label,
        ("next", async_generator_next as *const (), 1),
        ("return", async_generator_return as *const (), 1),
        ("throw", async_generator_throw as *const (), 1),
    );
    // Symbol.toStringTag（Object.prototype.toString → "[object AsyncGenerator]"）。
    let tag_key =
        oxide_types::private_key::make_well_known_symbol_key(oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG);
    let tag_shape = sh.make_shape(ag_proto.shape_id(), tag_key);
    ag_proto.set_shape_id(tag_shape);
    let tag_pos = ag_proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AsyncGenerator").0)));
    ag_proto.set_data_meta(tag_pos, PropAttributes::new(false, false, true));
    // @@asyncIterator：返回自身（异步生成器是异步可迭代对象，供 for-await-of 消费）。
    let aiter_key = oxide_types::private_key::make_well_known_symbol_key(8);
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method_key_labeled_static(
        &mut ag_proto,
        sh,
        sf,
        aiter_key,
        "@@asyncIterator",
        unsafe { oxide_types::object::NativeFnPtr::from_raw(async_generator_symbol_async_iterator as *const ()) },
        0,
        world,
        ag_label,
    );
    Vm::swap_intrinsic_proto(&mut vm.realm.async_generator_proto, *ag_proto);

    // %AsyncGeneratorFunction.prototype%：proto = Function.prototype，
    // constructor = %AsyncGeneratorFunction%。占位构造器动态创建未实现，
    // 调用抛 TypeError。选择性重建复用：前轮占位构造器按键迁移（prototype
    // 槽在下方 P 槽换入后重指新原型），不再每轮新建对象。
    let agf_ctor_label = sf.intern("AsyncGeneratorFunctionCtor").0;
    let agf_reuse_key = oxide_kernel::builtin::FnWrapperKey::new(0, agf_ctor_label, 0, 0);
    // SAFETY: async_generator_function_constructor 是 NativeFn 函数项。
    let agf_ctor_fn_ptr = unsafe {
        NativeFnPtr::from_raw(oxide_builtins::function::async_generator_function_constructor::<Vm> as *const ())
    };
    let (agf_ctor_ptr, agf_ctor_is_new) = match world.find_fn_wrapper(agf_reuse_key, agf_ctor_fn_ptr, 1) {
        Some(ptr) => (ptr, false),
        None => {
            let mut agf_ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
            agf_ctor.set_function(true);
            agf_ctor.set_native_arg_count(1);
            agf_ctor.set_native_fn(Some(agf_ctor_fn_ptr));
            // 构造器 tag：IsConstructor 判定与 new 表达式派发据此放行。
            agf_ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
            // 自身属性序：length、name、prototype、@@toStringTag（CreateBuiltinFunction 序）。
            let length_si = sf.intern("length").0;
            let length_shape = sh.make_shape(EMPTY_SHAPE_ID, length_si);
            agf_ctor.set_shape_id(length_shape);
            let name_si = sf.intern("name").0;
            let name_shape = sh.make_shape(agf_ctor.shape_id(), name_si);
            agf_ctor.set_shape_id(name_shape);
            let proto_si = sf.intern("prototype").0;
            let proto_shape = sh.make_shape(agf_ctor.shape_id(), proto_si);
            agf_ctor.set_shape_id(proto_shape);
            let tag_key = oxide_types::private_key::make_well_known_symbol_key(
                oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG,
            );
            let tag_shape = sh.make_shape(agf_ctor.shape_id(), tag_key);
            agf_ctor.set_shape_id(tag_shape);
            let agf_ctor_ptr = Box::into_raw(agf_ctor);
            // 登记进 world 释放表（带复用键）：session 收尾统一释放构造器本体与属性区。
            world.track_fn_wrapper(agf_ctor_ptr, agf_reuse_key);
            (agf_ctor_ptr, true)
        }
    };
    let mut agf_proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    // agf_proto.constructor = %AsyncGeneratorFunction%（构造器登记进 world 释放表，与 builtin 方法 wrapper 同生命周期）。
    let ctor_si = sf.intern("constructor").0;
    let ctor2_shape = sh.make_shape(agf_proto.shape_id(), ctor_si);
    agf_proto.set_shape_id(ctor2_shape);
    let cpos = agf_proto.push_prop(JsValue::from_js_object(agf_ctor_ptr));
    agf_proto.set_data_meta(cpos, PropAttributes::new(false, false, true));
    // agf_proto.prototype = %AsyncGeneratorPrototype%。
    let proto2_si = sf.intern("prototype").0;
    let proto2_shape = sh.make_shape(agf_proto.shape_id(), proto2_si);
    agf_proto.set_shape_id(proto2_shape);
    let ppos2 = agf_proto.push_prop(JsValue::from_js_object(vm.realm.async_generator_proto.as_ptr() as *mut JsObject));
    agf_proto.set_data_meta(ppos2, PropAttributes::new(false, false, true));
    // agf_proto[Symbol.toStringTag] = "AsyncGeneratorFunction"（数据属性，w/e/c = false/false/true）。
    let tag2_key =
        oxide_types::private_key::make_well_known_symbol_key(oxide_types::private_key::WELL_KNOWN_SYMBOL_TO_STRING_TAG);
    let tag2_shape = sh.make_shape(agf_proto.shape_id(), tag2_key);
    agf_proto.set_shape_id(tag2_shape);
    let tag2_pos = agf_proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AsyncGeneratorFunction").0)));
    agf_proto.set_data_meta(tag2_pos, PropAttributes::new(false, false, true));

    // proto 本体只存在于 P 槽（Arc 副本，原 Box 随函数结束释放）：装入 P 槽后再写
    // 构造器 length/name/prototype/@@toStringTag 属性（按形状序），prototype 指向 P 槽实例，
    // 使其与动态异步生成器函数使用的 [[Prototype]] 同一对象。复用构造器槽位已填充，
    // prototype 原位改指新 P 原型（旧原型已随 P 换出释放）。
    Vm::swap_intrinsic_proto(&mut vm.realm.async_generator_function_proto, *agf_proto);
    // SAFETY: agf_ctor_ptr 为 Box 原分配（已登记释放表、生命周期覆盖 session），本 Vm 独占。
    unsafe {
        let ctor_mut = &mut *agf_ctor_ptr;
        let proto_val = JsValue::from_js_object(vm.realm.async_generator_function_proto.as_ptr() as *mut JsObject);
        let name_val = JsValue::perm_string(sf.string_ptr(sf.intern("AsyncGeneratorFunction").0));
        if agf_ctor_is_new {
            let lpos = ctor_mut.push_prop(JsValue::int(1));
            ctor_mut.set_data_meta(lpos, PropAttributes::new(false, false, true));
            let npos = ctor_mut.push_prop(name_val);
            ctor_mut.set_data_meta(npos, PropAttributes::new(false, false, true));
            let ppos = ctor_mut.push_prop(proto_val);
            ctor_mut.set_data_meta(ppos, PropAttributes::new(false, false, false));
            let tag_val = JsValue::perm_string(sf.string_ptr(sf.intern("Function").0));
            let tpos = ctor_mut.push_prop(tag_val);
            ctor_mut.set_data_meta(tpos, PropAttributes::new(false, false, true));
        } else {
            let si_prototype = sf.intern("prototype").0;
            if let Some(pos) = sh.lookup_position(ctor_mut.shape_id(), si_prototype) {
                ctor_mut.set_prop_at(pos, proto_val);
            }
            let si_name = sf.intern("name").0;
            if let Some(pos) = sh.lookup_position(ctor_mut.shape_id(), si_name) {
                ctor_mut.set_prop_at(pos, name_val);
            }
        }
    }

    // 绑定 global：AsyncGeneratorFunction 槽已存在则原位更新（full_reset 未重建
    // global 时旧槽指向已弃 ctor），不存在则开新槽。
    let global_ptr = vm.realm.session.global_object().as_ptr() as *mut JsObject;
    // SAFETY: global 由 session 持有存活整个 session；本函数内只改其 shape/属性区，
    // 期间无 reset 或对象搬移。
    let global = unsafe { &mut *global_ptr };
    let si = sf.intern("AsyncGeneratorFunction").0;
    let ctor_val = JsValue::from_js_object(agf_ctor_ptr);
    if let Some(pos) = sh.lookup_position(global.shape_id(), si) {
        global.set_prop_at(pos, ctor_val);
    } else {
        let shape = sh.make_shape(global.shape_id(), si);
        global.set_shape_id(shape);
        let pos = global.push_prop(ctor_val);
        // global 数据属性：writable:true、enumerable:false、configurable:true。
        global.set_data_meta(pos, PropAttributes::new(true, false, true));
        global.bump_generation();
    }
}

// ── session GC 支撑：状态快照中的 JsValues 作为异步生成器对象边追踪 ──

#[expect(clippy::mut_from_ref)]
fn async_gen_state_mut(obj: &JsObject) -> Option<&mut AsyncGeneratorState> {
    let ptr = obj.native_data() as *mut AsyncGeneratorState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 在 create_async_generator_object 中由 Box::into_raw 分配，
        // 生命周期与异步生成器对象一致；GC 路径（mark 边收集）只读状态盒。
        Some(unsafe { &mut *ptr })
    }
}

/// 异步生成器状态内全部引用边的扁平列表（GC mark 边）：对象/字符串/BigInt
/// 均产出，消费侧按值类型分发到对象栈与存活集。
pub(crate) fn async_generator_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = async_gen_state_mut(obj) else {
        return Vec::new();
    };
    let mut edges = Vec::new();
    edges.push(state.callee);
    edges.extend(state.args.iter().copied());
    edges.push(state.result);
    for req in state.queue.iter() {
        edges.push(req.promise);
        edges.push(req.resolve);
        edges.push(req.reject);
    }
    if let Some(req) = &state.current {
        edges.push(req.promise);
        edges.push(req.resolve);
        edges.push(req.reject);
    }
    state.suspended.for_each_value(|v| edges.push(v));
    edges
}

/// 异步生成器状态内所有 session 字符串的扁平列表（GC mark 字符串边）。
pub(crate) fn async_generator_native_string_edges(obj: &JsObject) -> Vec<*mut oxide_types::object::JsString> {
    let Some(state) = async_gen_state_mut(obj) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let push_str = |v: JsValue, out: &mut Vec<*mut oxide_types::object::JsString>| {
        if v.is_string() {
            out.push(v.as_string_ptr_mut());
        }
    };
    push_str(state.callee, &mut out);
    for &v in &state.args {
        push_str(v, &mut out);
    }
    push_str(state.result, &mut out);
    for req in state.queue.iter() {
        push_str(req.promise, &mut out);
        push_str(req.resolve, &mut out);
        push_str(req.reject, &mut out);
    }
    if let Some(req) = &state.current {
        push_str(req.promise, &mut out);
        push_str(req.resolve, &mut out);
        push_str(req.reject, &mut out);
    }
    state.suspended.for_each_value(|v| {
        if v.is_string() {
            out.push(v.as_string_ptr_mut());
        }
    });
    out
}

/// 异步生成器挂起帧 cell_stack 的 cell 指针扁平列表（GC mark cell 边）。
/// 与 `async_generator_native_string_edges` 经同一字段清单（`suspended.cell_stack`）
/// 产出，cell 指针不经对象图、须独立入存活集。
pub(crate) fn async_generator_native_cell_edges(obj: &JsObject) -> Vec<*mut Cell> {
    let Some(state) = async_gen_state_mut(obj) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for cells in &state.suspended.cell_stack {
        for &cell_ptr in cells {
            if !cell_ptr.is_null() {
                out.push(cell_ptr);
            }
        }
    }
    out
}

/// 只读核算异步生成器状态盒字节（不释放），供 GC 账目核算。
pub(crate) fn async_generator_native_size(obj: &JsObject) -> u64 {
    if !obj.is_async_generator_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut AsyncGeneratorState;
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        std::mem::size_of::<AsyncGeneratorState>() as u64
            + (*ptr).suspended.heap_bytes()
            + (*ptr).args.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
    }
}

/// 释放异步生成器状态盒（对象被 GC 回收时），返回释放字节数。
pub(crate) fn drop_async_generator_native(obj: &JsObject) -> u64 {
    let bytes = async_generator_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let ptr = obj.native_data() as *mut AsyncGeneratorState;
    // SAFETY: ptr 非空（async_generator_native_size 已验证），Box::from_raw 恰好释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    drop(state);
    bytes
}
