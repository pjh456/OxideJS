//! 生成器运行时：帧快照挂起/恢复 + next/return/throw 方法。
//!
//! 策略：`function*` 函数调用返回生成器迭代器对象（状态存 `native_data`），
//! body 不立即执行。`next()` 时把生成器执行状态（寄存器窗口、pc、bytecode、
//! spill/save/try/cell 栈段、for-in/for-of 迭代器）灌回 VM，在内嵌 dispatch
//! 循环中继续执行到下一个 `YIELD` 或 `RETURN`。`YIELD` 通过
//! `Vm::generator_suspended` 信号让内嵌 dispatch 返回，恢复方据此快照挂起状态。

use std::sync::Arc;

use oxide_builtins::iterator::make_iter_result;
use oxide_bytecode::opcode;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::mem::P;
use oxide_types::object::{Cell, JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use crate::vm::{CallFrame, ForInIter, FrameContinuation, TryHandler, Vm};

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
    // ── 挂起时的执行上下文 ──
    pub regs: Box<[JsValue; 256]>,
    pub pc: usize,
    pub bytecode: Vec<opcode::Instr>,
    /// 生成器模块 flat_id（恢复时重激活 immutables）。
    pub sub_idx: u32,
    pub active_reg_limit: u8,
    pub root_reg_limit: u8,
    /// 生成器帧（压入时由 push_bytecode_frame 构造，挂起时弹出存此）。
    pub frame: Option<CallFrame>,
    pub spill_stack: Vec<JsValue>,
    pub save_stack: Vec<JsValue>,
    pub cell_stack: Vec<Vec<*mut Cell>>,
    pub try_stack: Vec<TryHandler>,
    pub for_in_iters: Vec<*mut ForInIter<'static>>,
    pub for_of_iters: Vec<JsValue>,
    pub last_for_of_result: JsValue,
    pub saved_bytecode_stack: Vec<Vec<opcode::Instr>>,
    pub saved_immutables_stack: Vec<*const [JsValue]>,
    /// 在途异常/完成（throw 穿越 finally 挂起时保留，恢复后继续展开）。
    pub exception_value: Option<JsValue>,
    pub pending_exception: Option<JsValue>,
    pub pending_error_kind: Option<&'static str>,
    pub pending_completion: Option<crate::vm::Completion>,
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
    /// 完成：`value` 为返回值。
    Completed { value: JsValue },
    /// 未捕获异常逃逸：`value` 为原异常值，调用方须重新抛出。
    Thrown { value: JsValue },
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
        let saved = self.save_inline_state();
        self.generator_suspended = None;
        // 嵌套生成器创建（参数默认值里调用其它 generator）会改写 init 标志，须保存恢复。
        let prev_init_step = self.generator_init_step;
        let prev_body_started = self.generator_body_started;
        self.generator_body_started = false;
        self.generator_init_step = true;
        self.native_call_depth += 1;

        // 压入生成器帧（New）。
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
            Err(e) => oxide_builtins::error::create_error(self, &e),
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
        let saved = self.save_inline_state();
        self.generator_suspended = None;
        self.native_call_depth += 1;

        // 首次执行：压入生成器帧；挂起：恢复快照。
        if was_new {
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
            let args = unsafe { std::mem::take(&mut (*state_ptr).args) };
            let push_res = self.push_bytecode_frame(
                callee,
                JsValue::undefined(),
                &args,
                None,
                None,
                JsValue::undefined(),
                FrameContinuation::None,
            );
            unsafe { (*state_ptr).args = args };
            if let Err(e) = push_res {
                self.restore_inline_state(saved);
                return Err(e);
            }
            unsafe { (*state_ptr).phase = GeneratorPhase::Running };
        } else {
            let state = unsafe { &mut *state_ptr };
            self.regs = *state.regs;
            self.pc = state.pc;
            self.bytecode = std::mem::take(&mut state.bytecode);
            let subs = Arc::clone(&self.sub_modules);
            if (state.sub_idx as usize) < subs.len() {
                self.activate_immutables(state.sub_idx as usize, &subs[state.sub_idx as usize].constants);
            } else {
                // 挂起状态跨 run：sub_modules 已重建，无法恢复（与动态函数同限制）。
                self.restore_inline_state(saved);
                return Err("generator suspended across runs is no longer valid".into());
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
            self.saved_bytecode_stack = std::mem::take(&mut state.saved_bytecode_stack);
            self.saved_immutables_stack = std::mem::take(&mut state.saved_immutables_stack);
            self.exception_value = state.exception_value.take();
            self.pending_exception = state.pending_exception.take();
            self.pending_error_kind = state.pending_error_kind.take();
            self.pending_completion = state.pending_completion.take();
            self.inline_callee = None;
            state.phase = GeneratorPhase::Running;

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

        let prev_dispatch = self.generator_dispatch;
        self.generator_dispatch = true;
        let result = self.dispatch();
        self.generator_dispatch = prev_dispatch;
        self.native_call_depth -= 1;

        // YIELD 让出：快照挂起状态。
        if let Some(value) = self.generator_suspended.take() {
            let state = unsafe { &mut *state_ptr };
            self.snapshot_generator(state)?;
            self.restore_inline_state(saved);
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
                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
                let state = unsafe { &mut *state_ptr };
                state.phase = GeneratorPhase::Completed;
                state.result = JsValue::undefined();
                self.restore_inline_state(saved);
                Ok(GeneratorStep::Thrown { value: exc })
            }
        }
    }

    /// 把当前 VM 执行状态（生成器 body 刚在嵌套 dispatch 中让出）快照进状态盒。
    ///
    /// 生成器帧弹出存 `frame`，其余栈段整段搬入（嵌套循环期间这些栈只含生成器数据）。
    fn snapshot_generator(&mut self, state: &mut GeneratorState) -> Result<(), String> {
        let frame = self
            .frames
            .pop()
            .ok_or_else(|| "generator frame missing on yield".to_string())?;
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
        state.saved_bytecode_stack = std::mem::take(&mut self.saved_bytecode_stack);
        state.saved_immutables_stack = std::mem::take(&mut self.saved_immutables_stack);
        state.exception_value = self.exception_value.take();
        state.pending_exception = self.pending_exception.take();
        state.pending_error_kind = self.pending_error_kind.take();
        state.pending_completion = self.pending_completion.take();
        state.phase = GeneratorPhase::Suspended;
        Ok(())
    }

    /// 在生成器挂起点交付 `return v` 完成：穿越 finally 或直接返回。
    ///
    /// # 返回值
    /// - `Ok(Some(value))`：直接完成，无需再 dispatch（`value` 为返回值）；
    /// - `Ok(None)`：进入 finally 穿越，调用方须继续 `dispatch()`。
    fn complete_generator_return(&mut self, value: JsValue) -> Result<Option<JsValue>, String> {
        // return 逃出当前帧：弹出纯 catch handler，穿越 finally。
        self.pop_frame_catch_handlers();
        let crossed = self
            .try_stack
            .iter()
            .filter(|h| h.frame_depth == self.frames.len() && h.finally_pc.is_some())
            .count();
        if let Some(finally_pc) = self.record_completion(crate::vm::Completion::Return {
            value,
            remaining_finally: crossed,
        }) {
            self.pc = finally_pc;
            return Ok(None);
        }
        // 无 finally：直接交付返回（弹出生成器帧后 frames 为空 → Some(value)）。
        self.do_return(value)
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
fn generator_step_result(vm: &mut Vm, step: Result<GeneratorStep, String>) -> NativeResult {
    match step {
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
    let object_proto_val = JsValue::from_js_object(vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject);

    // %GeneratorPrototype%：proto = Object.prototype，方法 next/return/throw/@@iterator。
    let mut gen_proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, object_proto_val));
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
    edges
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
}

/// 释放生成器状态盒（对象被 GC 回收时），返回释放字节数。
pub(crate) fn drop_generator_native(obj: &JsObject) -> u64 {
    let ptr = obj.native_data() as *mut GeneratorState;
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: 指针来自 create_generator_object 的 Box::into_raw，只在 GC 回收时释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    std::mem::size_of::<GeneratorState>() as u64
        + state.bytecode.len() as u64 * std::mem::size_of::<u32>() as u64
        + state.spill_stack.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
        + state.save_stack.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
        + state.for_of_iters.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
        + state.args.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
}
