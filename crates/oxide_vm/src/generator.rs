//! 生成器运行时：帧快照挂起/恢复 + next/return/throw 方法。
//!
//! 策略：`function*` 函数调用返回生成器迭代器对象（状态存 `native_data`），
//! body 不立即执行。`next()` 时把生成器执行状态（寄存器窗口、pc、bytecode、
//! spill/save/try/cell 栈段、for-in/for-of 迭代器）灌回 VM，在内嵌 dispatch
//! 循环中继续执行到下一个 `YIELD` 或 `RETURN`。`YIELD` 通过
//! `Vm::generator_suspended` 信号让内嵌 dispatch 返回，恢复方据此快照挂起状态。

use std::sync::Arc;

use oxide_builtins::iterator::{is_callable, make_iter_result};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_boolean, NativeResult};
use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use crate::vm::Vm;

/// 生成器执行阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeneratorPhase {
    /// 尚未开始执行（首次 next 前）。
    New,
    /// 正在执行（body 运行在嵌套 dispatch 循环中）。
    Running,
    /// 已在 `yield` 挂起。
    Suspended,
    /// 已正常返回 / 被 `return()` 提前结束。
    Completed,
}

/// 生成器挂起时的完整执行上下文快照。
///
/// 存在生成器对象 `native_data`（`Box`），跨 `next()` 调用存活；其中所有
/// `JsValue` 由 session GC 经生成器对象边追踪（见 `session_gc`）。
pub(crate) struct GeneratorState {
    pub phase: GeneratorPhase,
    /// 生成器函数对象（首次执行时压帧用）。
    pub callee: JsValue,
    /// 首次调用实参。
    pub args: Vec<JsValue>,
    /// 首次调用时的方法接收者（`obj.g()` 的 `obj`），压帧时作 `this` 绑定。
    pub this_value: JsValue,
    /// 完成后返回值。
    pub result: JsValue,
    /// 挂起时的执行上下文（regs/pc/bytecode/各栈段/迭代器/在途异常）。
    pub suspended: crate::suspended::SuspendedFrame,
}

/// `next()/return()/throw()` 的注入模式。
#[derive(Debug, Clone, Copy)]
pub(crate) enum GeneratorResumeMode {
    /// 普通恢复：`next(v)` 的 v 作为 `yield` 表达式结果（写 reg 0）。
    Next(JsValue),
    /// 提前结束：等价于在挂起点执行 `return v`（触发 finally）。
    Return(JsValue),
    /// 注入异常：等价于在挂起点抛错（可被 catch/finally 捕获）。
    Throw(JsValue),
}

/// 一次恢复的结局。
pub(crate) enum GeneratorStep {
    /// 让出：`value` 为 `yield` 的值，生成器仍可继续。
    Suspended { value: JsValue },
    /// `yield*` 委托让出：`value` 为内层迭代器的原始结果对象，外层 next() 原样返回
    /// （透传内层 done/value 字段，spec GeneratorYield 语义）。
    SuspendedRaw { value: JsValue },
    /// 完成：`value` 为返回值。
    Completed { value: JsValue },
    /// 未捕获异常逃逸：`value` 为原异常值，调用方须重新抛出。
    Thrown { value: JsValue },
}

/// `yield*` 首次推进（YIELD_STAR dispatch）的结局。
pub(crate) enum YieldStarOutcome {
    /// 内层未 done：挂起外层，让出内层原始结果对象（原样透传）。
    Suspend(JsValue),
    /// 内层 done：委托完成，value 为委托值（写 reg 0 继续外层）。
    Continue(JsValue),
    /// 异常已展开到外层 catch/finally：继续 dispatch。
    Unwind,
}

/// `yield*` 委托转发（生成器恢复时）的结局。
pub(crate) enum DelegateOutcome {
    /// 内层未 done：保持委托挂起，让出 value。
    Suspend { value: JsValue },
    /// 内层消化 next/throw 后 done：委托结束，外层继续，value 为委托值。
    Continue { value: JsValue },
    /// 内层消化 return 后 done / 无 return 方法：委托结束，外层完成，value 为返回值。
    Complete { value: JsValue },
    /// 内层调用抛错且已展开到外层 catch/finally：委托终止，继续 dispatch。
    Unwind,
}

impl Vm {
    /// 调用生成器函数返回的迭代器对象：创建生成器实例、挂状态并执行调用时参数初始化。
    ///
    /// 实例 [[Prototype]] = 调用方 `g.prototype`（为对象时），否则回退 `%GeneratorPrototype%`
    /// （GetPrototypeFromConstructor 语义，`default-proto`/`prototype-value` 测试）。
    pub(crate) fn create_generator_object(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        let gen_proto_val = JsValue::from_js_object(self.generator_proto.as_ptr() as *mut JsObject);
        let obj = self.epoch.alloc(JsObject::new_empty(EMPTY_SHAPE_ID, gen_proto_val));
        let obj_ref = unsafe { &mut *obj };
        obj_ref.type_tag = JsObject::OBJ_TYPE_GENERATOR;
        let state = Box::new(GeneratorState {
            phase: GeneratorPhase::New,
            callee,
            args: args.to_vec(),
            this_value,
            result: JsValue::undefined(),
            suspended: crate::suspended::SuspendedFrame::new_empty(),
        });
        obj_ref.set_native_data(Box::into_raw(state) as *mut u8);
        self.gc_state.track_epoch_object(obj);
        let gen_val = JsValue::from_js_object(obj);
        // 调用时参数初始化（含默认值/解构/arguments 创建）：副作用与异常在 `g()` 时刻生效。
        self.initialize_generator(gen_val)?;
        // 实例 [[Prototype]] 在参数初始化之后读取：参数默认值可能改写 `g.prototype`
        // （GetPrototypeFromConstructor 语义），为对象则用，否则回退 %GeneratorPrototype%。
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

    /// 生成器调用时的参数初始化步：压入生成器帧，执行到 body 起点标记（SUSPEND_BODY）。
    ///
    /// 参数初始化抛错时恢复调用方状态并在调用方上下文重新抛（可被外围 try/catch 捕获）；
    /// 成功则把挂起在 body 起点的状态快照存入生成器对象。
    ///
    /// # 副作用
    /// - 生成器对象状态从 `New` 转为 `Suspended`（body 起点）。
    /// - 参数默认值的副作用、`arguments` 创建、解构的迭代副作用在调用时刻完成。
    fn initialize_generator(&mut self, gen_val: JsValue) -> Result<(), String> {
        let state_ptr = self.generator_state_ptr(gen_val);
        let saved = self.save_inline_state(256);
        self.generator_suspended = None;
        // 嵌套生成器创建（参数默认值里调用其它 generator）会改写 init 标志，须保存恢复。
        let prev_init_step = self.generator_init_step;
        let prev_body_started = self.generator_body_started;
        self.generator_body_started = false;
        self.generator_init_step = true;
        self.native_call_depth += 1;

        // 压入生成器帧（New）。
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
        unsafe { (*state_ptr).phase = GeneratorPhase::Running };

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
            let state = unsafe { &mut *state_ptr };
            self.snapshot_generator(state)?;
            self.restore_inline_state(saved);
            return Ok(());
        }

        // 参数初始化抛错/异常结束：恢复调用方上下文后重新抛出。
        let exc = self.last_uncaught_value.take().unwrap_or_else(|| match result {
            Ok(v) => oxide_builtins::error::create_error(self, &format!("generator initialization failed: {v:?}")),
            Err(e) => oxide_builtins::error::create_from_text(self, &e),
        });
        let kind = self.thrown_error_kind(exc);
        unsafe { (*state_ptr).phase = GeneratorPhase::Completed };
        self.restore_inline_state(saved);
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(kind);
        self.unwind().map(|_| ())
    }

    /// 取出生成器对象的状态指针（调用方须先校验 `is_generator_obj`）。
    fn generator_state_ptr(&self, gen_val: JsValue) -> *mut GeneratorState {
        unsafe { (*gen_val.as_js_object_ptr()).native_data() as *mut GeneratorState }
    }

    /// 恢复生成器一步执行：`next(v)` / `return(v)` / `throw(e)`。
    ///
    /// # 步骤
    /// 1. 校验生成器对象与阶段（已完成/执行中直接给出对应结局）。
    /// 2. 用 `InlineSyncState` 保存调用方 VM 状态，把生成器挂起快照灌回 VM。
    /// 3. 在嵌套 `dispatch()` 中运行生成器 body 到下一个 `YIELD`/`RETURN`/异常。
    /// 4. 让出则快照新状态；完成/异常则标记阶段；最后恢复调用方状态。
    ///
    /// # 副作用
    /// - 修改生成器对象 `native_data` 中的快照。
    /// - 嵌套 dispatch 期间 VM 完全被生成器状态占据。
    pub(crate) fn resume_generator(
        &mut self, gen_val: JsValue, mode: GeneratorResumeMode,
    ) -> Result<GeneratorStep, String> {
        if !gen_val.is_object() {
            let err = oxide_builtins::error::create_type_error(self, "Generator methods called on non-object");
            return Ok(GeneratorStep::Thrown { value: err });
        }
        let obj = unsafe { &*gen_val.as_js_object_ptr() };
        if !obj.is_generator_obj() {
            let err =
                oxide_builtins::error::create_type_error(self, "Generator methods called on incompatible receiver");
            return Ok(GeneratorStep::Thrown { value: err });
        }
        let state_ptr = self.generator_state_ptr(gen_val);

        // 已完成：next 返回 {value: undefined, done: true}（完成值仅在完成步交付）；
        // return(v) 返回 {value: v, done: true}；throw 直接传播异常。
        if unsafe { (*state_ptr).phase } == GeneratorPhase::Completed {
            return match mode {
                GeneratorResumeMode::Next(_) => Ok(GeneratorStep::Completed { value: JsValue::undefined() }),
                GeneratorResumeMode::Return(v) => Ok(GeneratorStep::Completed { value: v }),
                GeneratorResumeMode::Throw(e) => Ok(GeneratorStep::Thrown { value: e }),
            };
        }
        if unsafe { (*state_ptr).phase } == GeneratorPhase::Running {
            let err = oxide_builtins::error::create_type_error(self, "Generator is already running");
            return Ok(GeneratorStep::Thrown { value: err });
        }

        let was_new = unsafe { (*state_ptr).phase } == GeneratorPhase::New;
        // return() 在未开始的生成器上直接完成，不执行 body。
        if was_new {
            if let GeneratorResumeMode::Return(v) = mode {
                let state = unsafe { &mut *state_ptr };
                state.phase = GeneratorPhase::Completed;
                state.result = v;
                return Ok(GeneratorStep::Completed { value: v });
            }
            if let GeneratorResumeMode::Throw(e) = mode {
                unsafe { (*state_ptr).phase = GeneratorPhase::Completed };
                return Ok(GeneratorStep::Thrown { value: e });
            }
        }

        // 保存调用方完整 VM 状态。
        let saved = self.save_inline_state(256);
        self.generator_suspended = None;
        self.native_call_depth += 1;

        // 首次执行：压入生成器帧；挂起：恢复快照。
        if was_new {
            self.delegated_iterator = None;
            let callee = unsafe { (*state_ptr).callee };
            let args = unsafe { std::mem::take(&mut (*state_ptr).args) };
            let push_res = self.prepare_execution_initial(callee, JsValue::undefined(), &args);
            unsafe { (*state_ptr).args = args };
            if let Err(e) = push_res {
                self.restore_inline_state(saved);
                self.native_call_depth -= 1;
                return Err(e);
            }
            unsafe { (*state_ptr).phase = GeneratorPhase::Running };
        } else {
            let state = unsafe { &mut *state_ptr };
            let subs = Arc::clone(&self.sub_modules);
            let restore_res = state.suspended.restore_into(self, &subs);
            if restore_res.is_err() {
                // 挂起状态跨 run：sub_modules 已重建，无法恢复（与动态函数同限制）。
                self.restore_inline_state(saved);
                self.native_call_depth -= 1;
                return Err("generator suspended across runs is no longer valid".into());
            }
            state.phase = GeneratorPhase::Running;

            // `yield*` 委托恢复：把 next/return/throw 请求转发给内层迭代器，按内层
            // 结局决定挂起透传 / 外层继续 / 外层完成 / 异常传播。
            // 委托分支处置完毕后直接进入 dispatch，不再执行注入模式（reg 0 已就位）。
            if self.delegated_iterator.is_some() {
                let forwarded = self.delegate_forward(mode);
                match forwarded {
                    Err(e) => {
                        // 委托期间异常逃逸（unwind 失败，已弹出全部帧）：标记完成并交付 Thrown。
                        let exc = self
                            .last_uncaught_value
                            .take()
                            .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                        self.restore_inline_state(saved);
                        self.native_call_depth -= 1;
                        let state = unsafe { &mut *state_ptr };
                        state.phase = GeneratorPhase::Completed;
                        state.result = JsValue::undefined();
                        return Ok(GeneratorStep::Thrown { value: exc });
                    }
                    Ok(DelegateOutcome::Suspend { value }) => {
                        let state = unsafe { &mut *state_ptr };
                        state.phase = GeneratorPhase::Suspended;
                        self.snapshot_generator(state)?;
                        self.restore_inline_state(saved);
                        self.native_call_depth -= 1;
                        // 委托让出原样透传内层结果对象，不二次包装。
                        return Ok(GeneratorStep::SuspendedRaw { value });
                    }
                    Ok(DelegateOutcome::Continue { value }) => {
                        // 委托结束（内层消化 next/throw 后 done）：yield* 表达式值为
                        // value，外层继续执行。
                        self.regs[0] = value;
                    }
                    Ok(DelegateOutcome::Complete { value }) => {
                        // 委托结束（内层消化 return 后 done 或无 return 方法）：外层完成，
                        // 交付值为内层结果，穿越自身 finally。
                        let prev_dispatch = self.generator_dispatch;
                        self.generator_dispatch = true;
                        let completed = self.complete_generator_return(value);
                        self.generator_dispatch = prev_dispatch;
                        if let Some(completed) = completed? {
                            self.restore_inline_state(saved);
                            self.native_call_depth -= 1;
                            let state = unsafe { &mut *state_ptr };
                            state.phase = GeneratorPhase::Completed;
                            state.result = completed;
                            return Ok(GeneratorStep::Completed { value: completed });
                        }
                        // 进入 finally 穿越：继续 dispatch。
                    }
                    Ok(DelegateOutcome::Unwind) => {
                        // 内层调用抛错且已展开到外层 catch/finally：委托终止，继续 dispatch。
                        self.delegated_iterator = None;
                    }
                }
            } else {
                // 注入模式：next(v) 写 reg 0；throw(e) 恢复异常上下文；return(v) 完成穿越。
                match mode {
                    GeneratorResumeMode::Next(arg) => {
                        self.regs[0] = arg;
                    }
                    GeneratorResumeMode::Throw(exc) => {
                        self.exception_value = Some(exc);
                        self.pending_error_kind = Some(self.thrown_error_kind(exc));
                        if self.unwind().is_err() {
                            self.restore_inline_state(saved);
                            self.native_call_depth -= 1;
                            let thrown = self.last_uncaught_value.take().unwrap_or(exc);
                            unsafe { (*state_ptr).phase = GeneratorPhase::Completed };
                            return Ok(GeneratorStep::Thrown { value: thrown });
                        }
                    }
                    GeneratorResumeMode::Return(value) => {
                        // 等价于在挂起点执行 return：穿越 finally 或直接交付返回值。
                        // 直接交付路径走 do_return（帧弹出后需按内嵌 dispatch 语义返回）。
                        let prev_dispatch = self.generator_dispatch;
                        self.generator_dispatch = true;
                        let completed = self.complete_generator_return(value);
                        self.generator_dispatch = prev_dispatch;
                        if let Some(completed) = completed? {
                            self.restore_inline_state(saved);
                            self.native_call_depth -= 1;
                            let state = unsafe { &mut *state_ptr };
                            state.phase = GeneratorPhase::Completed;
                            state.result = completed;
                            return Ok(GeneratorStep::Completed { value: completed });
                        }
                    }
                }
            }
        }

        let prev_dispatch = self.generator_dispatch;
        self.generator_dispatch = true;
        let result = self.dispatch();
        self.generator_dispatch = prev_dispatch;
        self.native_call_depth -= 1;

        // YIELD 让出：快照挂起状态。
        if let Some(value) = self.generator_suspended.take() {
            let state = unsafe { &mut *state_ptr };
            // 委托挂起时 value 是内层原始结果对象，原样透传；普通 yield 才二次包装。
            let delegating = self.delegated_iterator.is_some();
            self.snapshot_generator(state)?;
            self.restore_inline_state(saved);
            if delegating {
                return Ok(GeneratorStep::SuspendedRaw { value });
            }
            return Ok(GeneratorStep::Suspended { value });
        }

        // 完成或异常逃逸。
        match result {
            Ok(value) => {
                let state = unsafe { &mut *state_ptr };
                state.phase = GeneratorPhase::Completed;
                state.result = value;
                self.restore_inline_state(saved);
                Ok(GeneratorStep::Completed { value })
            }
            Err(e) => {
                let exc = self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                let state = unsafe { &mut *state_ptr };
                state.phase = GeneratorPhase::Completed;
                state.result = JsValue::undefined();
                self.restore_inline_state(saved);
                Ok(GeneratorStep::Thrown { value: exc })
            }
        }
    }

    /// `yield*` 委托首次进入：GetIterator 取内层迭代器并推进一步。
    ///
    /// # 步骤
    /// 1. 经迭代协议把内层值包装为迭代器（不可迭代抛 TypeError）。
    /// 2. 调内层 next(undefined) 得 {value, done}。
    /// 3. done → 委托完成值交付（外层继续）；未 done → 挂起，委托迭代器存入
    ///    `self.delegated_iterator`。
    ///
    /// # 副作用
    /// - 内层 next 同步执行，可能压入/弹出调用帧。
    /// - 挂起时设置 `delegated_iterator`，由 snapshot_generator 存入生成器状态。
    pub(crate) fn dispatch_yield_star(&mut self, rd: usize) -> Result<YieldStarOutcome, String> {
        let inner = self.regs[rd];
        let iterator = match oxide_builtins::iterator::make_iterator_for_value_without_return(self, inner) {
            Ok(it) => it,
            Err(exc) => {
                self.last_uncaught_value = Some(exc);
                return self.yield_star_raise(String::new());
            }
        };
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let next_si = self.kernel_core.perm_interner().intern("next").0;
        let next_fn = match self.ordinary_get(iter_obj, next_si, iterator) {
            Ok(f) => f,
            Err(e) => return self.yield_star_raise(e),
        };
        let result = if is_callable(next_fn) {
            match self.call_function_sync(next_fn, iterator, &[JsValue::undefined()]) {
                Ok(r) => r,
                Err(e) => return self.yield_star_raise(e),
            }
        } else {
            return self.yield_star_raise(self.error_message_text("TypeError", "iterator.next is not callable"));
        };
        if !result.is_object() {
            return self.yield_star_raise(self.error_message_text("TypeError", "iterator result is not an object"));
        }
        let result_obj = unsafe { &*result.as_js_object_ptr() };
        let done_si = self.kernel_core.perm_interner().intern("done").0;
        let value_si = self.kernel_core.perm_interner().intern("value").0;
        let done = to_boolean(match self.ordinary_get(result_obj, done_si, result) {
            Ok(v) => v,
            Err(e) => return self.yield_star_raise(e),
        });
        // done 时才读取 value（委托完成值）；done=false 直接让出原始结果对象。
        if done {
            let value = match self.ordinary_get(result_obj, value_si, result) {
                Ok(v) => v,
                Err(e) => return self.yield_star_raise(e),
            };
            Ok(YieldStarOutcome::Continue(value))
        } else {
            self.delegated_iterator = Some(iterator);
            // 挂起让出内层原始结果对象：外层 next() 原样透传（done 字段保持内层值）。
            Ok(YieldStarOutcome::Suspend(result))
        }
    }

    /// `yield*` 委托单步转发：把外层恢复请求转发给内层迭代器。
    ///
    /// # 步骤
    /// 1. next 走统一包装器（数组/字符串按索引、对象委托自身 next，方法内置不提前绑定）；
    ///    return/throw 对内层迭代器延迟 GetMethod（wrapper 不应在创建时访问内层方法）。
    /// 2. 方法缺失时按语义兜底：next → TypeError；return → 外层直接完成（值为请求值）；
    ///    throw → 先 IteratorClose（调内层 return，其抛错以该错为准）再补抛原异常。
    /// 3. 调内层方法得结果对象，读 done/value 判定委托结局。
    ///
    /// # 副作用
    /// - 内层调用抛错时经 unwind 展开（可被外层 catch/finally 捕获，返回 Unwind）。
    /// - 委托结束时清空 `self.delegated_iterator`，挂起透传时保留。
    pub(crate) fn delegate_forward(&mut self, mode: GeneratorResumeMode) -> Result<DelegateOutcome, String> {
        let iterator = match self.delegated_iterator {
            Some(it) => it,
            None => return Ok(DelegateOutcome::Unwind),
        };
        let (method, arg) = match mode {
            GeneratorResumeMode::Next(v) => ("next", v),
            GeneratorResumeMode::Return(v) => ("return", v),
            GeneratorResumeMode::Throw(e) => ("throw", e),
        };
        // return/throw 的 receiver 与 GetMethod 目标都是内层迭代器（包装器统一 next）。
        let inner = if matches!(mode, GeneratorResumeMode::Next(_)) {
            iterator
        } else {
            self.iterator_inner(iterator)
        };
        let iter_obj = unsafe { &*inner.as_js_object_ptr() };
        let method_si = self.kernel_core.perm_interner().intern(method).0;
        let method_fn = match self.ordinary_get(iter_obj, method_si, inner) {
            Ok(f) => f,
            Err(e) => return self.delegate_raise(e),
        };
        let inner_result = if is_callable(method_fn) {
            match self.call_function_sync(method_fn, inner, &[arg]) {
                Ok(r) => r,
                Err(e) => return self.delegate_raise(e),
            }
        } else {
            match mode {
                GeneratorResumeMode::Next(_) => {
                    return self.delegate_raise(self.error_message_text("TypeError", "iterator.next is not callable"))
                }
                GeneratorResumeMode::Return(v) => {
                    // 内层无 return 方法：外层直接完成，值为请求值。
                    self.delegated_iterator = None;
                    return Ok(DelegateOutcome::Complete { value: v });
                }
                GeneratorResumeMode::Throw(_e) => {
                    // 内层无 throw 方法：先 IteratorClose（GetMethod 内层 return，getter 抛错
                    // 传播、调用抛错传播、结果非对象抛 TypeError），再抛 TypeError（协议违规）。
                    let ret_si = self.kernel_core.perm_interner().intern("return").0;
                    let ret_fn = match self.ordinary_get(iter_obj, ret_si, inner) {
                        Ok(f) => f,
                        Err(e) => return self.delegate_raise(e),
                    };
                    if is_callable(ret_fn) {
                        let close_result = match self.call_function_sync(ret_fn, inner, &[]) {
                            Ok(r) => r,
                            Err(e) => return self.delegate_raise(e),
                        };
                        if !close_result.is_object() {
                            return self.delegate_raise(
                                self.error_message_text("TypeError", "IteratorResult is not an object"),
                            );
                        }
                    }
                    self.delegated_iterator = None;
                    return self.delegate_raise(self.error_message_text(
                        "TypeError",
                        "yield* protocol violation: iterator does not have a throw method",
                    ));
                }
            }
        };
        if !inner_result.is_object() {
            return self.delegate_raise(self.error_message_text("TypeError", "iterator result is not an object"));
        }
        let result_obj = unsafe { &*inner_result.as_js_object_ptr() };
        let done_si = self.kernel_core.perm_interner().intern("done").0;
        let value_si = self.kernel_core.perm_interner().intern("value").0;
        let done = to_boolean(match self.ordinary_get(result_obj, done_si, inner_result) {
            Ok(v) => v,
            Err(e) => return self.delegate_raise(e),
        });
        // done 时才读取 value（委托完成值）；done=false 直接让出原始结果对象。
        if done {
            let value = match self.ordinary_get(result_obj, value_si, inner_result) {
                Ok(v) => v,
                Err(e) => return self.delegate_raise(e),
            };
            // 委托结束：next/throw 被内层消化后外层继续，return 被内层消化后外层完成。
            self.delegated_iterator = None;
            match mode {
                GeneratorResumeMode::Next(_) | GeneratorResumeMode::Throw(_) => Ok(DelegateOutcome::Continue { value }),
                GeneratorResumeMode::Return(_) => Ok(DelegateOutcome::Complete { value }),
            }
        } else {
            // 转发期间内层生成器的恢复会覆盖 `self.delegated_iterator`，须重新存回
            // 委托迭代器，使外层挂起快照保留委托状态。让出内层原始结果对象。
            self.delegated_iterator = Some(iterator);
            Ok(DelegateOutcome::Suspend { value: inner_result })
        }
    }

    /// 从统一包装器中取内层迭代器（`__inner__` 槽），兜底返回包装器自身。
    fn iterator_inner(&mut self, wrapper: JsValue) -> JsValue {
        if !wrapper.is_object() {
            return wrapper;
        }
        let obj = unsafe { &*wrapper.as_js_object_ptr() };
        let inner_si = self.kernel_core.perm_interner().intern("__inner__").0;
        match self.ordinary_get(obj, inner_si, wrapper) {
            Ok(v) if !v.is_undefined() => v,
            _ => wrapper,
        }
    }

    /// 委托期间的异常注入：恢复原始异常值并经 unwind 展开。
    ///
    /// # 返回值
    /// - `Ok(Unwind)`：异常被外层 catch/finally 捕获，可继续 dispatch；
    /// - `Err`：异常逃逸出外层（unwind 失败），调用方按 Thrown 交付。
    fn delegate_raise(&mut self, msg: String) -> Result<DelegateOutcome, String> {
        let exc = self
            .last_uncaught_value
            .take()
            .unwrap_or_else(|| oxide_builtins::error::create_type_error(self, &msg));
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(self.thrown_error_kind(exc));
        match self.unwind() {
            Ok(()) => Ok(DelegateOutcome::Unwind),
            Err(e) => Err(e),
        }
    }

    /// `yield*` 首次推进的异常注入：与 [`delegate_raise`] 同路径，结局为
    /// [`YieldStarOutcome`] 的 Unwind。
    fn yield_star_raise(&mut self, msg: String) -> Result<YieldStarOutcome, String> {
        let exc = self
            .last_uncaught_value
            .take()
            .unwrap_or_else(|| oxide_builtins::error::create_type_error(self, &msg));
        self.exception_value = Some(exc);
        self.pending_error_kind = Some(self.thrown_error_kind(exc));
        match self.unwind() {
            Ok(()) => Ok(YieldStarOutcome::Unwind),
            Err(e) => Err(e),
        }
    }

    /// 把当前 VM 执行状态（生成器 body 刚在嵌套 dispatch 中让出）快照进状态盒。
    ///
    /// 生成器帧弹出存 `suspended.frame`，其余栈段整段搬入（嵌套循环期间这些栈
    /// 只含生成器数据）。
    fn snapshot_generator(&mut self, state: &mut GeneratorState) -> Result<(), String> {
        state.suspended.save_from(self, state.callee)?;
        state.phase = GeneratorPhase::Suspended;
        Ok(())
    }

    /// 在生成器挂起点交付 `return v` 完成：穿越 finally 或直接返回。
    ///
    /// # 返回值
    /// - `Ok(Some(value))`：直接完成，无需再 dispatch（`value` 为返回值）；
    /// - `Ok(None)`：进入 finally 穿越，调用方须继续 `dispatch()`。
    pub(crate) fn complete_generator_return(&mut self, value: JsValue) -> Result<Option<JsValue>, String> {
        // 纯 catch handler 的清理延后到完成消费处：record_completion 在 finally
        // 穿越路径弹出逃出的 catch-only handler；无 finally 时 do_return 弹帧前
        // 兜底清理。若提前弹出，return() 抛错经 unwind 展开时将找不到外围 catch。
        let crossed = self
            .try_stack
            .iter()
            .filter(|h| h.frame_depth == self.frames.len() && h.finally_pc.is_some())
            .count();
        // .return()/.throw() 注入时生成器挂起快照中打开的全部迭代器一并逃出
        // （栈上迭代器属于本生成器），完成恢复处统一关闭。
        let for_of_count = self.iters.for_of_iters.len();
        let for_in_count = self.iters.for_in_iters.len();
        if let Some(finally_pc) = self.record_completion(crate::vm::Completion::Return {
            value,
            remaining_finally: crossed,
            for_of_count,
            for_in_count,
        }) {
            self.pc = finally_pc;
            return Ok(None);
        }
        // 无 finally：关闭逃出迭代器后直接交付返回（弹出生成器帧后 frames 为空 → Some(value)）。
        match self.close_escaped_iters(for_of_count, for_in_count) {
            Ok(true) => self.do_return(value),
            Ok(false) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// `%GeneratorPrototype%.next`：恢复生成器执行到下一个 yield/return。
pub(crate) fn generator_next(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let step = vm.resume_generator(this_val, GeneratorResumeMode::Next(arg));
    generator_step_result(vm, step)
}

/// `%GeneratorPrototype%.return`：提前结束生成器（穿越挂起中的 finally）。
pub(crate) fn generator_return(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let step = vm.resume_generator(this_val, GeneratorResumeMode::Return(arg));
    generator_step_result(vm, step)
}

/// `%GeneratorPrototype%.throw`：向生成器注入异常。
pub(crate) fn generator_throw(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let step = vm.resume_generator(this_val, GeneratorResumeMode::Throw(arg));
    generator_step_result(vm, step)
}

/// `%GeneratorPrototype%[@@iterator]`：生成器自身即可迭代，返回 `this`。
pub(crate) fn generator_symbol_iterator(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    NativeResult::Ok(this_val)
}

/// 把恢复结局折叠为迭代器结果 `{value, done}` 或传播异常。
/// 委托让出（SuspendedRaw）原样透传内层结果对象，不做二次包装。
fn generator_step_result(vm: &mut Vm, step: Result<GeneratorStep, String>) -> NativeResult {
    match step {
        Ok(GeneratorStep::SuspendedRaw { value }) => NativeResult::Ok(value),
        Ok(GeneratorStep::Suspended { value }) | Ok(GeneratorStep::Completed { value }) => {
            let done = matches!(step, Ok(GeneratorStep::Completed { .. }));
            NativeResult::Ok(make_iter_result(vm, value, done))
        }
        Ok(GeneratorStep::Thrown { value }) => NativeResult::Err(value),
        Err(e) => NativeResult::Err(oxide_builtins::error::create_type_error(vm, &e)),
    }
}

/// 初始化生成器内建对象：`%GeneratorPrototype%` 与 `%GeneratorFunction.prototype%`。
///
/// 两个原型对象存于 VM 字段（session 生命周期），`full_reset` 后重建。
pub(crate) fn init_generator_intrinsics(vm: &mut Vm) {
    let sf = vm.kernel_core.perm_interner().as_ref();
    let sh = vm.kernel_core.shape_forge().as_ref();
    let fn_proto_val = vm.session.builtin_world().fn_proto_val();
    // %GeneratorPrototype%：proto = %IteratorPrototype%（生成器是迭代器，继承
    // @@iterator 与 Iterator helper 方法），方法 next/return/throw。
    let iterator_proto_val =
        JsValue::from_js_object(vm.session.builtin_world().iterator_proto.as_ptr() as *mut JsObject);
    let mut gen_proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, iterator_proto_val));
    oxide_kernel::bind_methods_static!(
        &mut gen_proto,
        sf,
        sh,
        fn_proto_val,
        ("next", generator_next as *const (), 1),
        ("return", generator_return as *const (), 1),
        ("throw", generator_throw as *const (), 1),
    );
    // @@toStringTag（Object.prototype.toString → "[object Generator]"）。
    let tag_si = sf.intern("@@toStringTag").0;
    let tag_shape = sh.make_shape(gen_proto.shape_id(), tag_si);
    gen_proto.set_shape_id(tag_shape);
    let tag_pos = gen_proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("Generator").0)));
    gen_proto.set_data_meta(tag_pos, oxide_types::object::PropAttributes::new(false, false, true));
    // @@iterator：返回自身（生成器是可迭代对象）。
    let iter_key = oxide_types::private_key::make_well_known_symbol_key(0);
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method_key_static(
        &mut gen_proto,
        sh,
        sf,
        iter_key,
        "@@iterator",
        unsafe { oxide_types::object::NativeFnPtr::from_raw(generator_symbol_iterator as *const ()) },
        0,
        fn_proto_val,
    );
    vm.generator_proto = P::new(*gen_proto);

    // %GeneratorFunction.prototype%：proto = Function.prototype，constructor = %GeneratorFunction%。
    let mut gf_ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    gf_ctor.set_function(true);
    gf_ctor.set_native_arg_count(1);
    let mut gf_proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    // %GeneratorFunction% 构造器：动态生成器函数创建未实现，调用抛 TypeError。
    gf_ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(generator_function_stub as *const ()) }));
    // prototype/name 属性（构造器形状 [prototype, name]）。
    let proto_si = sf.intern("prototype").0;
    let ctor_shape = sh.make_shape(gf_ctor.shape_id(), proto_si);
    gf_ctor.set_shape_id(ctor_shape);
    let ppos = gf_ctor.push_prop(JsValue::from_js_object(gf_proto.as_mut() as *mut JsObject));
    gf_ctor.set_data_meta(ppos, oxide_types::object::PropAttributes::new(false, false, false));
    let name_si = sf.intern("name").0;
    let name_shape = sh.make_shape(gf_ctor.shape_id(), name_si);
    gf_ctor.set_shape_id(name_shape);
    let npos = gf_ctor.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("GeneratorFunction").0)));
    gf_ctor.set_data_meta(npos, oxide_types::object::PropAttributes::new(false, false, true));
    // gf_proto.constructor = gf_ctor（构造器对象泄漏持有，与 builtin 方法 wrapper 同生命周期）。
    let ctor_si = sf.intern("constructor").0;
    let ctor2_shape = sh.make_shape(gf_proto.shape_id(), ctor_si);
    gf_proto.set_shape_id(ctor2_shape);
    let cpos = gf_proto.push_prop(JsValue::from_js_object(Box::into_raw(gf_ctor)));
    gf_proto.set_data_meta(cpos, oxide_types::object::PropAttributes::new(false, false, true));
    // gf_proto.prototype = %GeneratorPrototype%（默认原型，default-proto 测试读取）。
    let proto2_si = sf.intern("prototype").0;
    let proto2_shape = sh.make_shape(gf_proto.shape_id(), proto2_si);
    gf_proto.set_shape_id(proto2_shape);
    let ppos2 = gf_proto.push_prop(JsValue::from_js_object(vm.generator_proto.as_ptr() as *mut JsObject));
    gf_proto.set_data_meta(ppos2, oxide_types::object::PropAttributes::new(false, false, false));
    // gf_proto[Symbol.toStringTag] = "GeneratorFunction"（数据属性，w/e/c = false/false/true）。
    let tag2_key = oxide_types::private_key::make_well_known_symbol_key(0);
    let tag2_shape = sh.make_shape(gf_proto.shape_id(), tag2_key);
    gf_proto.set_shape_id(tag2_shape);
    let tag2_pos = gf_proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("GeneratorFunction").0)));
    gf_proto.set_data_meta(tag2_pos, oxide_types::object::PropAttributes::new(false, false, true));

    vm.generator_function_proto = P::new(*gf_proto);
}

/// `%GeneratorFunction%` 占位：动态生成器函数创建未实现，调用抛错。
fn generator_function_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    NativeResult::Err(oxide_builtins::error::create_type_error(
        vm,
        "GeneratorFunction constructor is not supported",
    ))
}

// ── session GC 支撑：状态快照中的 JsValues 作为生成器对象边追踪 ──

#[expect(clippy::mut_from_ref)]
fn generator_state_mut(obj: &JsObject) -> Option<&mut GeneratorState> {
    let ptr = obj.native_data() as *mut GeneratorState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 在 create_generator_object 中由 Box::into_raw 分配，
        // 生命周期与生成器对象一致；GC 只在 reset（无执行状态）时运行。
        Some(unsafe { &mut *ptr })
    }
}

/// 生成器状态内所有对象引用的扁平列表（GC mark 边）。
pub(crate) fn generator_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = generator_state_mut(obj) else {
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
    state.suspended.for_each_value(|v| {
        if v.is_object() {
            edges.push(v);
        }
    });
    edges
}

/// 生成器状态内所有 session 字符串的扁平列表（GC mark 字符串边）。
/// 与 `generator_native_edges` 的对象收集互补，经同一字段清单（自身字段 +
/// `suspended.for_each_value`）产出字符串。
pub(crate) fn generator_native_string_edges(obj: &JsObject) -> Vec<*mut oxide_types::object::JsString> {
    let Some(state) = generator_state_mut(obj) else {
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
    state.suspended.for_each_value(|v| {
        if v.is_string() {
            out.push(v.as_string_ptr_mut());
        }
    });
    out
}

/// 用转发函数重写状态快照中的所有 JsValue（session GC 移动式清扫 / promote 用）。
pub(crate) fn rewrite_generator_native(obj: &JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue) {
    let Some(state) = generator_state_mut(obj) else {
        return;
    };
    state.callee = rewrite(state.callee);
    for v in &mut state.args {
        *v = rewrite(*v);
    }
    state.result = rewrite(state.result);
    state.suspended.rewrite_values(rewrite);
}

/// 深拷贝状态盒到新对象（promote / sweep 用）：新对象持独立 Box，源盒可安全释放。
pub(crate) fn clone_generator_native_with_rewrite(
    old: &JsObject, new: &mut JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue,
) {
    let Some(state) = generator_state_mut(old) else {
        return;
    };
    let cloned = GeneratorState {
        phase: state.phase,
        callee: rewrite(state.callee),
        args: state.args.iter().copied().map(&mut rewrite).collect(),
        this_value: state.this_value,
        result: rewrite(state.result),
        suspended: state.suspended.clone_with_rewrite(rewrite),
    };
    new.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 释放生成器状态盒（对象被 GC 回收时），返回释放字节数。
pub(crate) fn drop_generator_native(obj: &JsObject) -> u64 {
    if !obj.is_generator_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut GeneratorState;
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: 指针来自 create_generator_object 的 Box::into_raw，只在 GC 回收时释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    std::mem::size_of::<GeneratorState>() as u64
        + state.suspended.heap_bytes()
        + state.args.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
}
