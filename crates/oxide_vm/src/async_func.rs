//! 异步函数运行时：异步帧快照挂起/恢复 + await 微任务驱动。
//!
//! 策略：`async function` 调用返回 capability promise（状态存 `AsyncState`），
//! body 立即同步执行到第一个 `AWAIT`（与 generator 的惰性不同）。`await` 求值
//! 被等待值、PromiseResolve 包装后登记恢复反应；promise settle 后微任务经
//! `Vm::async_context` 驱动的嵌套 dispatch 恢复异步帧，执行到下一个
//! `AWAIT`/`RETURN`/异常。挂起快照/恢复机械复用 generator 的 regs/pc/bytecode/
//! spill/save/try/cell 栈段搬运。`AWAIT` 经 `Vm::async_suspended` 信号让内嵌
//! dispatch 返回，恢复方据此快照挂起状态。

use std::sync::Arc;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_boolean, NativeResult};
use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::vm::Vm;

/// 异步函数执行阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AsyncPhase {
    /// 尚未开始执行（调用时压帧前）。
    New,
    /// 正在执行（body 运行在嵌套 dispatch 循环中）。
    Running,
    /// 已在 `await` 挂起。
    Suspended,
    /// 已正常返回 / 异常结束。
    Completed,
}

/// 异步函数挂起时的完整执行上下文快照 + capability。
///
/// 存在异步上下文对象 `native_data`（`Box`），跨 await 微任务存活；其中所有
/// `JsValue` 由 session GC 经上下文对象边追踪（见 `session_gc`）。
pub(crate) struct AsyncState {
    pub phase: AsyncPhase,
    /// 异步函数对象（首次执行时压帧用）。
    pub callee: JsValue,
    /// 首次调用实参。
    pub args: Vec<JsValue>,
    /// 首次调用时的方法接收者（`obj.g()` 的 `obj`），压帧时作 `this` 绑定。
    pub this_value: JsValue,
    /// 完成后返回值。
    pub result: JsValue,
    /// 调用返回给调用方的 capability promise。
    pub promise: JsValue,
    /// capability 上的 resolve/reject 闭包（完成时结算用）。
    pub resolve: JsValue,
    pub reject: JsValue,
    /// 挂起时的执行上下文（regs/pc/bytecode/各栈段/迭代器/在途异常）。
    pub suspended: crate::suspended::SuspendedFrame,
}

/// 一次 await 恢复的注入模式。
#[derive(Debug, Clone, Copy)]
pub(crate) enum AsyncResumeMode {
    /// 普通恢复：fulfilled 值作为 `await` 表达式结果（写 reg 0）。
    Next(JsValue),
    /// 拒绝恢复：在挂起点抛错（可被 catch/finally 捕获）。
    Throw(JsValue),
}

/// 异步上下文对象上存 capability 闭包的属性名（await 恢复闭包定位上下文用）。
pub(crate) const ASYNC_CTX_PROP: &str = "__oxide_async_ctx__";
/// 恢复闭包上区分 reject 角色的属性名。
const ASYNC_REJECT_PROP: &str = "__oxide_async_reject__";

impl Vm {
    /// 调用异步函数返回的 capability promise：创建上下文对象 + 状态盒，立即压帧
    /// 同步执行 body 到第一个 AWAIT/RETURN/异常，返回 pending promise。
    ///
    /// # 副作用
    /// - 上下文对象状态从 `New` 转为 `Running`；AWAIT 挂起后转为 `Suspended`。
    /// - 参数默认值副作用、body 到首个 await 的同步执行在调用时刻完成。
    pub(crate) fn create_async_object(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue],
    ) -> Result<JsValue, String> {
        let (promise, resolve, reject) = self.new_promise_capability();
        let ctx_val = self.create_async_context(callee, this_value, args, promise, resolve, reject);
        self.start_async(ctx_val)?;
        Ok(promise)
    }

    /// 分配异步上下文对象（`OBJ_TYPE_ASYNC`），AsyncState 状态盒挂 `native_data`。
    fn create_async_context(
        &mut self, callee: JsValue, this_value: JsValue, args: &[JsValue], promise: JsValue, resolve: JsValue,
        reject: JsValue,
    ) -> JsValue {
        let proto_val = JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        let obj = unsafe { &mut *ptr };
        obj.type_tag = JsObject::OBJ_TYPE_ASYNC;
        let state = Box::new(AsyncState {
            phase: AsyncPhase::New,
            callee,
            args: args.to_vec(),
            this_value,
            result: JsValue::undefined(),
            promise,
            resolve,
            reject,
            suspended: crate::suspended::SuspendedFrame::new_empty(),
        });
        obj.set_native_data(Box::into_raw(state) as *mut u8);
        JsValue::from_js_object(ptr)
    }

    /// 取出异步上下文对象的状态指针（调用方须先校验 `is_async_obj`）。
    fn async_state_ptr(&self, ctx_val: JsValue) -> *mut AsyncState {
        unsafe { (*ctx_val.as_js_object_ptr()).native_data() as *mut AsyncState }
    }

    /// 首次启动异步体：压入异步帧并在嵌套 dispatch 中执行到第一个 AWAIT/RETURN/异常。
    ///
    /// # 步骤
    /// 1. 用 `InlineSyncState` 保存调用方 VM 状态，清空 VM 后压入异步帧。
    /// 2. 嵌套 `dispatch()` 运行 body；AWAIT 置 `async_suspended` 使 dispatch 返回。
    /// 3. 挂起则快照；完成则 resolve capability；异常则 reject。恢复调用方状态。
    fn start_async(&mut self, ctx_val: JsValue) -> Result<(), String> {
        let state_ptr = self.async_state_ptr(ctx_val);
        let saved = self.save_inline_state();
        self.async_suspended = false;
        let prev_ctx = self.async_context.take();
        let prev_dispatch = self.async_dispatch;
        let prev_gen_ctx = self.async_gen_context.take();
        let prev_gen_dispatch = self.async_gen_dispatch;
        self.async_dispatch = true;
        // 普通异步函数执行期间 AWAIT 走普通异步恢复路径：屏蔽外层异步生成器上下文。
        self.async_gen_dispatch = false;
        self.async_context = Some(ctx_val);
        self.native_call_depth += 1;

        let callee = unsafe { (*state_ptr).callee };
        let this_value = unsafe { (*state_ptr).this_value };
        let args = unsafe { std::mem::take(&mut (*state_ptr).args) };
        let push_res = self.prepare_execution_initial(callee, this_value, &args);
        unsafe { (*state_ptr).args = args };
        if let Err(e) = push_res {
            self.restore_async_flags(prev_ctx, prev_dispatch, prev_gen_ctx, prev_gen_dispatch);
            self.native_call_depth -= 1;
            self.restore_inline_state(saved);
            return Err(e);
        }
        unsafe { (*state_ptr).phase = AsyncPhase::Running };

        let result = self.dispatch();
        self.restore_async_flags(prev_ctx, prev_dispatch, prev_gen_ctx, prev_gen_dispatch);
        self.native_call_depth -= 1;

        if std::mem::take(&mut self.async_suspended) {
            let state = unsafe { &mut *state_ptr };
            self.snapshot_async(state)?;
            self.restore_inline_state(saved);
            return Ok(());
        }

        match result {
            Ok(value) => {
                self.finish_async(state_ptr, Ok(value))?;
                self.restore_inline_state(saved);
                Ok(())
            }
            Err(e) => {
                let exc = self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
                self.finish_async(state_ptr, Err(exc))?;
                self.restore_inline_state(saved);
                Ok(())
            }
        }
    }

    /// 恢复异步体一步执行：`await` 的 promise settle 后由恢复闭包调用。
    ///
    /// # 步骤
    /// 1. 用 `InlineSyncState` 保存调用方 VM 状态，把挂起快照灌回 VM。
    /// 2. 注入 await 结果（fulfilled 写 reg 0 / rejected 注入异常并展开）。
    /// 3. 嵌套 `dispatch()` 运行 body 到下一个 AWAIT/RETURN/异常。
    /// 4. 挂起则快照新状态；完成/异常则结算 capability。恢复调用方状态。
    pub(crate) fn resume_async(&mut self, ctx_val: JsValue, mode: AsyncResumeMode) -> Result<(), String> {
        let state_ptr = self.async_state_ptr(ctx_val);
        let saved = self.save_inline_state();
        self.async_suspended = false;
        let prev_ctx = self.async_context.take();
        let prev_dispatch = self.async_dispatch;
        let prev_gen_ctx = self.async_gen_context.take();
        let prev_gen_dispatch = self.async_gen_dispatch;
        self.async_dispatch = true;
        // 普通异步函数执行期间 AWAIT 走普通异步恢复路径：屏蔽外层异步生成器上下文。
        self.async_gen_dispatch = false;
        self.async_context = Some(ctx_val);
        self.native_call_depth += 1;

        {
            let state = unsafe { &mut *state_ptr };
            let subs = Arc::clone(&self.sub_modules);
            let restore_res = state.suspended.restore_into(self, &subs);
            if restore_res.is_err() {
                // 挂起状态跨 run：sub_modules 已重建，无法恢复（与生成器同限制）。
                self.restore_async_flags(prev_ctx, prev_dispatch, prev_gen_ctx, prev_gen_dispatch);
                self.native_call_depth -= 1;
                self.restore_inline_state(saved);
                return Err("async function suspended across runs is no longer valid".into());
            }
            state.phase = AsyncPhase::Running;
        }

        // 注入 await 结果：fulfilled 写 reg 0；rejected 恢复异常上下文并展开。
        if let AsyncResumeMode::Throw(exc) = mode {
            self.exception_value = Some(exc);
            self.pending_error_kind = Some(self.thrown_error_kind(exc));
            if self.unwind().is_err() {
                // 异常逃逸出异步帧（无 catch/finally）：结算为 reject。
                self.restore_async_flags(prev_ctx, prev_dispatch, prev_gen_ctx, prev_gen_dispatch);
                self.native_call_depth -= 1;
                let thrown = self.last_uncaught_value.take().unwrap_or(exc);
                self.finish_async(state_ptr, Err(thrown))?;
                self.restore_inline_state(saved);
                return Ok(());
            }
        } else if let AsyncResumeMode::Next(value) = mode {
            self.regs[0] = value;
        }

        let result = self.dispatch();
        self.restore_async_flags(prev_ctx, prev_dispatch, prev_gen_ctx, prev_gen_dispatch);
        self.native_call_depth -= 1;

        if std::mem::take(&mut self.async_suspended) {
            let state = unsafe { &mut *state_ptr };
            self.snapshot_async(state)?;
            self.restore_inline_state(saved);
            return Ok(());
        }

        match result {
            Ok(value) => {
                self.finish_async(state_ptr, Ok(value))?;
                self.restore_inline_state(saved);
                Ok(())
            }
            Err(e) => {
                let exc = self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
                self.finish_async(state_ptr, Err(exc))?;
                self.restore_inline_state(saved);
                Ok(())
            }
        }
    }

    /// 恢复嵌套 dispatch 覆盖的 async 上下文标志（含外层异步生成器上下文）。
    fn restore_async_flags(
        &mut self, prev_ctx: Option<JsValue>, prev_dispatch: bool, prev_gen_ctx: Option<JsValue>,
        prev_gen_dispatch: bool,
    ) {
        self.async_dispatch = prev_dispatch;
        self.async_context = prev_ctx;
        self.async_gen_dispatch = prev_gen_dispatch;
        self.async_gen_context = prev_gen_ctx;
    }

    /// 结算异步函数：正常结束 resolve capability，异常结束 reject capability。
    fn finish_async(&mut self, state_ptr: *mut AsyncState, outcome: Result<JsValue, JsValue>) -> Result<(), String> {
        let (promise, value) = {
            let state = unsafe { &mut *state_ptr };
            state.phase = AsyncPhase::Completed;
            match &outcome {
                Ok(v) => {
                    state.result = *v;
                    (state.promise, *v)
                }
                Err(e) => {
                    state.result = JsValue::undefined();
                    (state.promise, *e)
                }
            }
        };
        match outcome {
            Ok(_) => {
                let _ = self.resolve_promise(promise, value);
            }
            Err(_) => {
                let _ = self.reject_promise(promise, value);
            }
        }
        Ok(())
    }

    /// `await` dispatch：PromiseResolve 包装被等待值、登记恢复反应，置挂起信号。
    ///
    /// # 副作用
    /// - 被等待值为原生 Promise 时直接复用，否则建新 promise 并 PromiseResolve。
    /// - 经 `perform_promise_then` 登记 fulfill/reject 恢复反应（微任务入队）。
    /// - 异步生成器内（`async_gen_dispatch`）登记异步生成器恢复闭包并置
    ///   `async_gen_suspended`；普通异步函数走 `async_suspended`。
    pub(crate) fn dispatch_await(&mut self, rd: usize) -> Result<(), String> {
        let value = self.regs[rd];
        self.perform_await(value)
    }

    /// await 核心：PromiseResolve 包装被等待值、登记恢复反应，置挂起信号。
    ///
    /// # 副作用
    /// - 被等待值为原生 Promise 时直接复用，否则建新 promise 并 PromiseResolve。
    /// - 经 `perform_promise_then` 登记 fulfill/reject 恢复反应（微任务入队）。
    /// - 异步生成器内（`async_gen_dispatch`）登记异步生成器恢复闭包并置
    ///   `async_gen_suspended`；普通异步函数走 `async_suspended`。
    pub(crate) fn perform_await(&mut self, value: JsValue) -> Result<(), String> {
        let ctx = match self.async_context {
            Some(c) => c,
            None => return Err("AWAIT executed outside async function".into()),
        };
        let promise = if self.is_promise_value(value) {
            value
        } else {
            let (p, _, _) = self.new_promise_capability();
            let _ = self.resolve_promise(p, value);
            p
        };
        if self.async_gen_dispatch {
            let fulfill_fn = self.make_async_gen_await_resume_fn(ctx, false);
            let reject_fn = self.make_async_gen_await_resume_fn(ctx, true);
            let _ = self.perform_promise_then(promise, fulfill_fn, reject_fn);
            self.async_gen_suspended = true;
            return Ok(());
        }
        let fulfill_fn = self.make_async_await_resume_fn(ctx, false);
        let reject_fn = self.make_async_await_resume_fn(ctx, true);
        let _ = self.perform_promise_then(promise, fulfill_fn, reject_fn);
        self.async_suspended = true;
        Ok(())
    }

    /// 构造 await 恢复闭包：携带目标异步上下文对象，区分 fulfill/reject 角色。
    fn make_async_await_resume_fn(&mut self, ctx: JsValue, reject_role: bool) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: async_await_resume_closure 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_await_resume_closure as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        let ctx_si = self.kernel_core.perm_interner().intern(ASYNC_CTX_PROP).0;
        self.set_or_create_prop_value(obj, ctx_si, ctx);
        let role_si = self.kernel_core.perm_interner().intern(ASYNC_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, role_si, JsValue::bool(reject_role));
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }

    /// 把当前 VM 执行状态（异步 body 刚在嵌套 dispatch 中让出）快照进状态盒。
    ///
    /// 异步帧弹出存 `suspended.frame`，其余栈段整段搬入（嵌套循环期间这些栈
    /// 只含异步数据）。
    fn snapshot_async(&mut self, state: &mut AsyncState) -> Result<(), String> {
        state.suspended.save_from(self, state.callee)?;
        state.phase = AsyncPhase::Suspended;
        Ok(())
    }
}

/// await 恢复闭包：读自身 prop 的异步上下文与角色，恢复异步帧继续执行。
fn async_await_resume_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "await resume handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let ctx_si = vm.kernel_core.perm_interner().intern(ASYNC_CTX_PROP).0;
    let ctx = vm.resolve_property(callee_obj, ctx_si).unwrap_or(JsValue::undefined());
    let role_si = vm.kernel_core.perm_interner().intern(ASYNC_REJECT_PROP).0;
    let is_reject = vm.resolve_property(callee_obj, role_si).map_or(false, to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let mode = if is_reject {
        AsyncResumeMode::Throw(value)
    } else {
        AsyncResumeMode::Next(value)
    };
    match vm.resume_async(ctx, mode) {
        Ok(()) => NativeResult::Ok(JsValue::undefined()),
        Err(e) => NativeResult::Err(oxide_builtins::error::create_error(vm, &e)),
    }
}

/// 初始化/重建异步函数内建对象：`%AsyncFunction.prototype%`（+ 占位 `%AsyncFunction%`）。
///
/// 异步函数对象的 [[Prototype]] 指向此原型（`f.constructor.name` → "AsyncFunction"）。
/// 动态 `AsyncFunction` 构造器创建未实现，调用抛 TypeError。
pub(crate) fn init_async_intrinsics(vm: &mut Vm) {
    let sf = vm.kernel_core.perm_interner().as_ref();
    let sh = vm.kernel_core.shape_forge().as_ref();
    let fn_proto_val = vm.session.builtin_world().fn_proto_val();

    // %AsyncFunction% 构造器：proto = Function.prototype，调用抛错（动态创建未实现）。
    let mut af_ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    af_ctor.set_function(true);
    af_ctor.set_native_arg_count(1);
    // SAFETY: async_function_stub 是 NativeFn 函数项。
    af_ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_function_stub as *const ()) }));

    // %AsyncFunction.prototype%：proto = Function.prototype，constructor = %AsyncFunction%。
    let mut af_proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
    let name_si = sf.intern("name").0;
    let ctor_shape = sh.make_shape(af_ctor.shape_id(), name_si);
    af_ctor.set_shape_id(ctor_shape);
    let npos = af_ctor.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AsyncFunction").0)));
    af_ctor.set_data_meta(npos, PropAttributes::new(false, false, true));
    let proto_si = sf.intern("prototype").0;
    let ctor_shape2 = sh.make_shape(af_ctor.shape_id(), proto_si);
    af_ctor.set_shape_id(ctor_shape2);
    let ppos = af_ctor.push_prop(JsValue::from_js_object(af_proto.as_mut() as *mut JsObject));
    af_ctor.set_data_meta(ppos, PropAttributes::new(false, false, false));
    // af_proto.constructor = %AsyncFunction%（构造器对象泄漏持有，与 builtin 方法 wrapper 同生命周期）。
    let ctor_si = sf.intern("constructor").0;
    let pshape = sh.make_shape(af_proto.shape_id(), ctor_si);
    af_proto.set_shape_id(pshape);
    let cpos = af_proto.push_prop(JsValue::from_js_object(Box::into_raw(af_ctor)));
    af_proto.set_data_meta(cpos, PropAttributes::new(false, false, true));
    // af_proto[Symbol.toStringTag] = "AsyncFunction"（数据属性，w/e/c = false/false/true）。
    let tag_si = sf.intern("@@toStringTag").0;
    let tag_shape = sh.make_shape(af_proto.shape_id(), tag_si);
    af_proto.set_shape_id(tag_shape);
    let tag_pos = af_proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AsyncFunction").0)));
    af_proto.set_data_meta(tag_pos, PropAttributes::new(false, false, true));

    vm.async_function_proto = P::new(*af_proto);
}

/// `%AsyncFunction%` 占位：动态异步函数创建未实现，调用抛错。
fn async_function_stub(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    NativeResult::Err(oxide_builtins::error::create_type_error(vm, "AsyncFunction constructor is not supported"))
}

// ── session GC 支撑：状态快照中的 JsValues 作为异步上下文对象边追踪 ──

fn async_state_mut(obj: &JsObject) -> Option<&mut AsyncState> {
    let ptr = obj.native_data() as *mut AsyncState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 在 create_async_context 中由 Box::into_raw 分配，
        // 生命周期与异步上下文对象一致；GC 只在 reset（无执行状态）时运行。
        Some(unsafe { &mut *ptr })
    }
}

/// 异步状态内所有对象引用的扁平列表（GC mark 边）。
pub(crate) fn async_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = async_state_mut(obj) else {
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
    push(state.promise, &mut edges);
    push(state.resolve, &mut edges);
    push(state.reject, &mut edges);
    state.suspended.for_each_value(|v| {
        if v.is_object() {
            edges.push(v);
        }
    });
    edges
}

/// 异步状态内所有 session 字符串的扁平列表（GC mark 字符串边）。
pub(crate) fn async_native_string_edges(obj: &JsObject) -> Vec<*mut oxide_types::object::JsString> {
    let Some(state) = async_state_mut(obj) else {
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
    push_str(state.promise, &mut out);
    push_str(state.resolve, &mut out);
    push_str(state.reject, &mut out);
    state.suspended.for_each_value(|v| {
        if v.is_string() {
            out.push(v.as_string_ptr_mut());
        }
    });
    out
}

/// 用转发函数重写状态快照中的所有 JsValue（session GC 移动式清扫 / promote 用）。
pub(crate) fn rewrite_async_native(obj: &JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue) {
    let Some(state) = async_state_mut(obj) else {
        return;
    };
    state.callee = rewrite(state.callee);
    for v in &mut state.args {
        *v = rewrite(*v);
    }
    state.result = rewrite(state.result);
    state.promise = rewrite(state.promise);
    state.resolve = rewrite(state.resolve);
    state.reject = rewrite(state.reject);
    state.suspended.rewrite_values(rewrite);
}

/// 深拷贝状态盒到新对象（promote / sweep 用）：新对象持独立 Box，源盒可安全释放。
pub(crate) fn clone_async_native_with_rewrite(
    old: &JsObject, new: &mut JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue,
) {
    let Some(state) = async_state_mut(old) else {
        return;
    };
    let cloned = AsyncState {
        phase: state.phase,
        callee: rewrite(state.callee),
        args: state.args.iter().copied().map(&mut rewrite).collect(),
        this_value: state.this_value,
        result: rewrite(state.result),
        promise: rewrite(state.promise),
        resolve: rewrite(state.resolve),
        reject: rewrite(state.reject),
        suspended: state.suspended.clone_with_rewrite(rewrite),
    };
    new.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 释放异步状态盒（对象被 GC 回收时），返回释放字节数。
pub(crate) fn drop_async_native(obj: &JsObject) -> u64 {
    if !obj.is_async_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut AsyncState;
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: 指针来自 create_async_context 的 Box::into_raw，只在 GC 回收时释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    std::mem::size_of::<AsyncState>() as u64
        + state.suspended.heap_bytes()
        + state.args.capacity() as u64 * std::mem::size_of::<JsValue>() as u64
}
