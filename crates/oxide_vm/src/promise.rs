//! Promise 运行时：Promise 状态盒 + resolve/reject 闭包 + 微任务队列。
//!
//! Promise 对象把 `PromiseState` 状态盒（Box）挂在 `native_data`；resolve/reject
//! 是携带目标 promise 的 native 闭包函数（函数对象 prop 存 promise 引用）。
//! 反应（reaction）与 thenable 委托以 `Microtask` 入队 `Vm::job_queue`，
//! 由 `run()` 末尾的 drain 循环 FIFO 执行。所有盒内 JsValue 由 session GC
//! 经 Promise 对象边追踪（见 `promise_native_edges` / `rewrite_promise_native` /
//! `clone_promise_native_with_rewrite` / `drop_promise_native`）。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::mem::P;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::native::NativeFn;
use crate::vm::Vm;
use crate::vm_warn;

/// Promise 的 settled 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromiseStateKind {
    Pending,
    Fulfilled,
    Rejected,
}

/// `then` 注册的一条反应：派生 promise 的能力（promise + resolve/reject）+ 处理器。
#[derive(Clone)]
pub(crate) struct PromiseReaction {
    pub promise: JsValue,
    pub resolve: JsValue,
    pub reject: JsValue,
    pub handler: JsValue,
    pub is_fulfill: bool,
}

/// Promise 状态盒：存于 Promise 对象 `native_data`，所有 JsValue 是 GC 边。
pub(crate) struct PromiseState {
    pub state: PromiseStateKind,
    pub result: JsValue,
    pub reactions: Vec<PromiseReaction>,
    /// 本 promise 能力上的 resolve/reject 闭包（thenable 委托入队时取用）。
    pub resolve_fn: JsValue,
    pub reject_fn: JsValue,
    /// resolve/reject 是否已被调用过（Resolve Promise Functions 的 alreadyResolved）。
    /// 首次调用后置位，后续任何 resolve/reject（含 thenable 委托期间）均 no-op。
    pub already_resolved: bool,
}

/// 一条微任务（job）。
pub(crate) enum Microtask {
    /// 反应任务：调 `handler(argument)`，结果经能力 resolve/reject 交付；空处理器直通。
    Reaction {
        is_fulfill: bool,
        handler: JsValue,
        argument: JsValue,
        resolve: JsValue,
        reject: JsValue,
    },
    /// thenable 委托任务：调 `then(thenable, resolve, reject)`。
    Thenable {
        thenable: JsValue,
        then: JsValue,
        resolve: JsValue,
        reject: JsValue,
    },
}

/// 单次 drain 的微任务处理上限，防止无终止的自我产生链导致活锁。
const MAX_DRAIN_JOBS: usize = 1_000_000;

/// 闭包函数对象上存目标 promise 的属性名。
const PROMISE_PROP: &str = "__oxide_promise__";
/// thenable 委托结算代理标志：绕过 alreadyResolved（委托是真正结算路径）。
const DELEGATED_PROP: &str = "__oxide_delegated__";
/// finally 处理器上存 onFinally 回调的属性名。
const ON_FINALLY_PROP: &str = "__oxide_on_finally__";
/// finally 处理器上区分 reject 角色的属性名。
const FINALLY_REJECT_PROP: &str = "__oxide_finally_reject__";
/// 能力 executor 上捕获 resolve/reject 的属性名。
const CAP_RESOLVE_PROP: &str = "__oxide_cap_resolve__";
const CAP_REJECT_PROP: &str = "__oxide_cap_reject__";

// ── 聚合静态方法（all/race/allSettled/any）内部状态属性 ──
/// 元素处理器函数上存共享记录对象的属性名。
const AGG_RECORD_PROP: &str = "__oxide_agg_record__";
/// 元素处理器函数上存元素下标的属性名。
const AGG_INDEX_PROP: &str = "__oxide_agg_index__";
/// 元素处理器函数上存"已调用一次"标志的属性名。
const AGG_ALREADY_PROP: &str = "__oxide_agg_already__";
/// 记录对象上存剩余未结算元素计数（含尾部哨兵 1）。
const AGG_REMAINING_PROP: &str = "__oxide_agg_remaining__";
/// 记录对象上存结果数组（all/allSettled 的 values，any 的 errors）。
const AGG_VALUES_PROP: &str = "__oxide_agg_values__";
/// 记录对象上存能力 resolve 闭包。
const AGG_RESOLVE_PROP: &str = "__oxide_agg_resolve__";
/// 记录对象上存能力 reject 闭包。
const AGG_REJECT_PROP: &str = "__oxide_agg_reject__";

/// 聚合静态方法的语义模式（决定元素处理器与结算方式）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum AggregateKind {
    All,
    Race,
    AllSettled,
    Any,
}

impl Vm {
    /// 创建空 Promise 对象（proto = `%Promise.prototype%`），状态盒为 Pending。
    fn create_promise_object(&mut self) -> JsValue {
        let proto_val = JsValue::from_js_object(self.promise_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        let obj = unsafe { &mut *ptr };
        obj.type_tag = JsObject::OBJ_TYPE_PROMISE;
        let state = Box::new(PromiseState {
            state: PromiseStateKind::Pending,
            result: JsValue::undefined(),
            reactions: Vec::new(),
            resolve_fn: JsValue::undefined(),
            reject_fn: JsValue::undefined(),
            already_resolved: false,
        });
        obj.set_native_data(Box::into_raw(state) as *mut u8);
        JsValue::from_js_object(ptr)
    }

    /// 新建 Promise 能力 `(promise, resolve, reject)`：resolve/reject 为携带
    /// 目标 promise 的 native 闭包，同时写入状态盒供 thenable 委托取用。
    pub(crate) fn new_promise_capability(&mut self) -> (JsValue, JsValue, JsValue) {
        let promise = self.create_promise_object();
        let resolve = self.make_resolve_reject_fn(promise, false, false);
        let reject = self.make_resolve_reject_fn(promise, true, false);
        let state = self.promise_state_ptr(promise);
        let state = unsafe { &mut *state };
        state.resolve_fn = resolve;
        state.reject_fn = reject;
        (promise, resolve, reject)
    }

    /// NewPromiseCapability(C)：按构造器 C 建能力。C 为内置 Promise 时走直接路径，
    /// 否则经 GetCapabilitiesExecutor 间接构造 `new C(executor)`。
    fn new_promise_capability_with_ctor(&mut self, ctor: JsValue) -> Result<(JsValue, JsValue, JsValue), JsValue> {
        let intrinsic = JsValue::from_js_object(self.promise_constructor.as_ptr() as *mut JsObject);
        if oxide_runtime_api::same_value(ctor, intrinsic) {
            return Ok(self.new_promise_capability());
        }
        // GetCapabilitiesExecutor：捕获 resolve/reject 到自身 prop。
        let executor = self.make_capability_executor();
        // 构造 C(executor)；结果须为对象。
        let promise = self.construct_ctor(ctor, &[executor])?;
        if !promise.is_object() {
            return Err(oxide_builtins::error::create_type_error(
                self,
                "Promise constructor returned a non-object",
            ));
        }
        let (resolve, reject) = {
            let exec_obj = unsafe { &*executor.as_js_object_ptr() };
            let res_si = self.kernel_core.perm_interner().intern(CAP_RESOLVE_PROP).0;
            let rej_si = self.kernel_core.perm_interner().intern(CAP_REJECT_PROP).0;
            let r = self.resolve_property(exec_obj, res_si).unwrap_or(JsValue::undefined());
            let j = self.resolve_property(exec_obj, rej_si).unwrap_or(JsValue::undefined());
            (r, j)
        };
        if !oxide_builtins::iterator::is_callable(resolve) || !oxide_builtins::iterator::is_callable(reject) {
            return Err(oxide_builtins::error::create_type_error(
                self,
                "Promise resolve and reject functions must be callable",
            ));
        }
        Ok((promise, resolve, reject))
    }

    /// 构造 GetCapabilitiesExecutor：调用时把 resolve/reject 存入自身 prop。
    fn make_capability_executor(&mut self) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: capability_executor 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(capability_executor as *const ()) }));
        func.set_native_arg_count(2);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        self.add_fn_name_length(obj, "", 2);
        JsValue::from_js_object(ptr)
    }

    /// 构造调用：分配 proto = ctor.prototype（缺省 Object.prototype）的 this 后以
    /// 普通调用执行 ctor，返回值非对象时回退到 this（与 Reflect.construct 同路径）。
    fn construct_ctor(&mut self, ctor: JsValue, args: &[JsValue]) -> Result<JsValue, JsValue> {
        if !oxide_builtins::iterator::is_callable(ctor) {
            return Err(oxide_builtins::error::create_type_error(self, "constructor is not callable"));
        }
        let ctor_obj = unsafe { &*ctor.as_js_object_ptr() };
        if ctor_obj.is_arrow()
            || (ctor_obj.native_fn().is_some() && ctor_obj.type_tag != JsObject::OBJ_TYPE_CONSTRUCTOR)
        {
            return Err(oxide_builtins::error::create_type_error(self, "constructor is not a constructor"));
        }
        let proto_si = self.kernel_core.perm_interner().intern("prototype").0;
        let proto_val = match self.resolve_property(ctor_obj, proto_si) {
            Some(p) if p.is_object() => p,
            _ => JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject),
        };
        let this_ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        let this_val = JsValue::from_js_object(this_ptr);
        match self.call_function_sync(ctor, this_val, args) {
            Ok(ret) if ret.is_object() => Ok(ret),
            Ok(_) => Ok(this_val),
            Err(e) => Err(self
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e))),
        }
    }

    /// 构造携带目标 promise 的 native 闭包（resolve 或 reject 角色）。
    /// `delegated` 为 true 表示 thenable 委托用的结算代理：绕过 alreadyResolved
    /// （委托是唯一真正的结算路径，由 then 的 resolve/reject 触发），但 state != Pending
    /// 仍阻止二次结算。
    fn make_resolve_reject_fn(&mut self, promise: JsValue, reject_role: bool, delegated: bool) -> JsValue {
        let native_fn: NativeFn = if reject_role { promise_reject_closure } else { promise_resolve_closure };
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: native_fn 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(native_fn as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        let si = self.kernel_core.perm_interner().intern(PROMISE_PROP).0;
        self.set_or_create_prop_value(obj, si, promise);
        let dsi = self.kernel_core.perm_interner().intern(DELEGATED_PROP).0;
        self.set_or_create_prop_value(obj, dsi, JsValue::bool(delegated));
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }

    /// 从 resolve/reject 闭包函数对象读取目标 promise。
    fn promise_from_callee(&self) -> Option<JsValue> {
        let callee = self.regs[254];
        if !callee.is_object() {
            return None;
        }
        let obj = unsafe { &*callee.as_js_object_ptr() };
        let si = self.kernel_core.perm_interner().intern(PROMISE_PROP).0;
        match self.resolve_property(obj, si) {
            Some(v) if v.is_object() => Some(v),
            _ => None,
        }
    }

    /// 当前 resolve/reject 闭包是否为 thenable 委托结算代理（绕过 alreadyResolved）。
    fn callee_delegated_flag(&self) -> bool {
        let callee = self.regs[254];
        if !callee.is_object() {
            return false;
        }
        let obj = unsafe { &*callee.as_js_object_ptr() };
        let si = self.kernel_core.perm_interner().intern(DELEGATED_PROP).0;
        self.resolve_property(obj, si).map(|v| v.is_bool() && v.as_bool()).unwrap_or(false)
    }

    /// 取出 Promise 状态盒指针（调用方须先校验 `is_promise_value`）。
    fn promise_state_ptr(&self, promise: JsValue) -> *mut PromiseState {
        unsafe { (*promise.as_js_object_ptr()).native_data() as *mut PromiseState }
    }

    /// 是否原生 Promise 对象。
    pub(crate) fn is_promise_value(&self, v: JsValue) -> bool {
        v.is_object() && {
            let ptr = v.as_js_object_ptr();
            !ptr.is_null() && unsafe { &*ptr }.is_promise_obj()
        }
    }

    /// 校验值的原型链含 `%Promise.prototype%`（`new Promise` 构造出的实例特征）。
    fn has_promise_proto(&self, v: JsValue) -> bool {
        if !v.is_object() {
            return false;
        }
        let target = self.promise_proto.as_ptr() as *mut JsObject;
        let mut cursor = v;
        for _ in 0..crate::vm::MAX_PROTO_CHAIN_DEPTH {
            if !cursor.is_object() {
                return false;
            }
            let ptr = cursor.as_js_object_ptr();
            if ptr.is_null() {
                return false;
            }
            if std::ptr::eq(ptr, target) {
                return true;
            }
            cursor = unsafe { &*ptr }.proto();
        }
        false
    }

    /// 入队一条微任务（追加到队尾，保证 FIFO）。
    fn enqueue(&mut self, job: Microtask) {
        self.job_queue.push_back(job);
    }

    /// `PromiseResolve` 核心：`x === promise` 抛 TypeError；thenable 委托入队；
    /// 其余直接以 `x` 完成 promise。
    ///
    /// # 副作用
    /// - 可能入队 Thenable 微任务，或直接 settle 目标 promise。
    /// - then getter 抛错时拒绝目标 promise（错误在内部消化，调用方无需处理）。
    pub(crate) fn resolve_promise(&mut self, promise: JsValue, x: JsValue) -> Result<(), String> {
        // alreadyResolved 守卫在 resolve 闭包层（首次调用置位，含 thenable 委托期间）。
        // 本函数自身不检查：委托闭包调用它时须放行（委托才是真正结算路径）；
        // 二次结算由 fulfill/reject 的 state != Pending 守卫兜底。
        if oxide_runtime_api::same_value(x, promise) {
            let err = oxide_builtins::error::create_type_error(self, "Chaining cycle detected for promise");
            return self.reject_promise(promise, err);
        }
        if x.is_object() {
            let x_obj = unsafe { &*x.as_js_object_ptr() };
            let then_si = self.kernel_core.perm_interner().intern("then").0;
            let then = match self.ordinary_get(x_obj, then_si, x) {
                Ok(v) => v,
                Err(e) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                    return self.reject_promise(promise, exc);
                }
            };
            if oxide_builtins::iterator::is_callable(then) {
                // thenable 委托期间本 promise 仍未 settle，但 already_resolved 已置位，
                // 后续 executor 的 resolve/reject 均 no-op。委托用独立结算代理闭包
                // （绕过 alreadyResolved——委托才是真正结算路径），仍受 state != Pending 守卫。
                let state = self.promise_state_ptr(promise);
                let state = unsafe { &mut *state };
                state.already_resolved = true;
                let resolve = self.make_resolve_reject_fn(promise, false, true);
                let reject = self.make_resolve_reject_fn(promise, true, true);
                self.enqueue(Microtask::Thenable {
                    thenable: x,
                    then,
                    resolve,
                    reject,
                });
                return Ok(());
            }
        }
        self.fulfill_promise(promise, x)
    }

    /// 以 `value` 完成 promise：只 settle 一次，完成后触发该态反应入队。
    pub(crate) fn fulfill_promise(&mut self, promise: JsValue, value: JsValue) -> Result<(), String> {
        let state_ptr = self.promise_state_ptr(promise);
        let settled = {
            let state = unsafe { &mut *state_ptr };
            if state.state != PromiseStateKind::Pending {
                return Ok(());
            }
            state.state = PromiseStateKind::Fulfilled;
            state.result = value;
            std::mem::take(&mut state.reactions)
        };
        // 只触发 fulfill 反应；reject 反应留待拒绝路径。
        for r in settled.into_iter().filter(|r| r.is_fulfill) {
            self.enqueue(Microtask::Reaction {
                is_fulfill: true,
                handler: r.handler,
                argument: value,
                resolve: r.resolve,
                reject: r.reject,
            });
        }
        Ok(())
    }

    /// 以 `reason` 拒绝 promise：只 settle 一次，完成后触发该态反应入队。
    pub(crate) fn reject_promise(&mut self, promise: JsValue, reason: JsValue) -> Result<(), String> {
        let state_ptr = self.promise_state_ptr(promise);
        let settled = {
            let state = unsafe { &mut *state_ptr };
            if state.state != PromiseStateKind::Pending {
                return Ok(());
            }
            state.state = PromiseStateKind::Rejected;
            state.result = reason;
            std::mem::take(&mut state.reactions)
        };
        // 只触发 reject 反应；fulfill 反应留待完成路径。
        for r in settled.into_iter().filter(|r| !r.is_fulfill) {
            self.enqueue(Microtask::Reaction {
                is_fulfill: false,
                handler: r.handler,
                argument: reason,
                resolve: r.resolve,
                reject: r.reject,
            });
        }
        Ok(())
    }

    /// PromiseResolve（%Promise%, value）核心：value 为原生 Promise 时读取其
    /// `constructor`，与 %Promise% 相同则原样返回；否则新建能力并经能力 resolve
    /// 结算（thenable 委托）。constructor getter / 结算抛错时透传原异常值。
    pub(crate) fn promise_resolve(&mut self, value: JsValue) -> Result<JsValue, JsValue> {
        if self.is_promise_value(value) {
            let obj = unsafe { &*value.as_js_object_ptr() };
            let ctor_si = self.kernel_core.perm_interner().intern("constructor").0;
            let ctor = match self.ordinary_get(obj, ctor_si, value) {
                Ok(c) => c,
                Err(e) => {
                    let exc = self
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                    return Err(exc);
                }
            };
            let intrinsic = JsValue::from_js_object(self.promise_constructor.as_ptr() as *mut JsObject);
            if oxide_runtime_api::same_value(ctor, intrinsic) {
                return Ok(value);
            }
        }
        let (promise, _resolve, _) = self.new_promise_capability();
        if let Err(e) = self.resolve_promise(promise, value) {
            let exc = self
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
            return Err(exc);
        }
        Ok(promise)
    }

    /// `PerformPromiseThen` 核心：注册 fulfill/reject 两条反应；已 settle 则直接入队。
    pub(crate) fn perform_promise_then(
        &mut self, this_val: JsValue, on_fulfilled: JsValue, on_rejected: JsValue,
    ) -> Result<JsValue, JsValue> {
        if !self.is_promise_value(this_val) {
            return Err(oxide_builtins::error::create_type_error(
                self,
                "Method Promise.prototype.then called on incompatible receiver",
            ));
        }
        let on_fulfilled = if oxide_builtins::iterator::is_callable(on_fulfilled) {
            on_fulfilled
        } else {
            JsValue::undefined()
        };
        let on_rejected = if oxide_builtins::iterator::is_callable(on_rejected) {
            on_rejected
        } else {
            JsValue::undefined()
        };
        let (new_promise, resolve, reject) = self.new_promise_capability();
        let state_ptr = self.promise_state_ptr(this_val);
        let (kind, result) = {
            let state = unsafe { &mut *state_ptr };
            (state.state, state.result)
        };
        match kind {
            PromiseStateKind::Pending => {
                let state = unsafe { &mut *state_ptr };
                state.reactions.push(PromiseReaction {
                    promise: new_promise,
                    resolve,
                    reject,
                    handler: on_fulfilled,
                    is_fulfill: true,
                });
                state.reactions.push(PromiseReaction {
                    promise: new_promise,
                    resolve,
                    reject,
                    handler: on_rejected,
                    is_fulfill: false,
                });
            }
            PromiseStateKind::Fulfilled => {
                self.enqueue(Microtask::Reaction {
                    is_fulfill: true,
                    handler: on_fulfilled,
                    argument: result,
                    resolve,
                    reject,
                });
            }
            PromiseStateKind::Rejected => {
                self.enqueue(Microtask::Reaction {
                    is_fulfill: false,
                    handler: on_rejected,
                    argument: result,
                    resolve,
                    reject,
                });
            }
        }
        Ok(new_promise)
    }

    /// drain 微任务队列：FIFO 逐条处理直到清空或达到上限。
    ///
    /// # 副作用
    /// - 任务内调用 JS 回调（`call_function_sync`），可能入队新任务。
    /// - 单任务抛错不中断 drain（未处理拒绝按规范不可见）。
    pub(crate) fn drain_job_queue(&mut self) {
        let mut count = 0usize;
        while let Some(job) = self.job_queue.pop_front() {
            count += 1;
            if count > MAX_DRAIN_JOBS {
                vm_warn!("drain_job_queue: exceeded {MAX_DRAIN_JOBS} microtasks, aborting");
                break;
            }
            self.run_microtask(job);
        }
    }

    /// 执行单条微任务。
    fn run_microtask(&mut self, job: Microtask) {
        match job {
            Microtask::Reaction {
                is_fulfill,
                handler,
                argument,
                resolve,
                reject,
            } => {
                // 空处理器（undefined/非可调用）→ 值直通：fulfill 调 resolve，reject 调 reject。
                let handler_result = if oxide_builtins::iterator::is_callable(handler) {
                    self.call_function_sync(handler, JsValue::undefined(), &[argument])
                } else if is_fulfill {
                    Ok(argument)
                } else {
                    let _ = self.call_function_sync(reject, JsValue::undefined(), &[argument]);
                    return;
                };
                match handler_result {
                    Ok(x) => {
                        let _ = self.call_function_sync(resolve, JsValue::undefined(), &[x]);
                    }
                    Err(e) => {
                        let exc = self
                            .last_uncaught_value
                            .take()
                            .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                        let _ = self.call_function_sync(reject, JsValue::undefined(), &[exc]);
                    }
                }
            }
            Microtask::Thenable {
                thenable,
                then,
                resolve,
                reject,
            } => {
                // 委托调用；抛错则拒绝目标 promise。
                match self.call_function_sync(then, thenable, &[resolve, reject]) {
                    Ok(_) => {}
                    Err(e) => {
                        let exc = self
                            .last_uncaught_value
                            .take()
                            .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                        let _ = self.call_function_sync(reject, JsValue::undefined(), &[exc]);
                    }
                }
            }
        }
    }

    /// 初始化/重建 Promise 内建对象：`%Promise%` 构造器与 `%Promise.prototype%`，
    /// 绑定到 global 的 `Promise` 槽。
    ///
    /// 在 VM 创建与 `full_reset` 后调用——原型继承自 session 的 Object/Function
    /// 原型，session 重建后须重挂并重绑 global 槽。
    pub(crate) fn init_promise_intrinsics(&mut self) {
        let sf = self.kernel_core.perm_interner().as_ref();
        let sh = self.kernel_core.shape_forge().as_ref();
        let fn_proto_val = self.session.builtin_world().fn_proto_val();
        let object_proto_val =
            JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject);

        // %Promise.prototype%：proto = Object.prototype，方法 then/catch/finally + @@toStringTag。
        let mut proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, object_proto_val));
        let ctor_si = sf.intern("constructor").0;
        let ctor_shape = sh.make_shape(proto.shape_id(), ctor_si);
        proto.set_shape_id(ctor_shape);
        proto.push_prop(JsValue::undefined());
        proto.set_data_meta(0u32, PropAttributes::new(true, false, true));
        oxide_kernel::bind_methods_static!(
            &mut proto,
            sf,
            sh,
            fn_proto_val,
            ("then", promise_then as *const (), 2),
            ("catch", promise_catch as *const (), 1),
            ("finally", promise_finally as *const (), 1),
        );
        let tag_si = sf.intern("@@toStringTag").0;
        let tag_shape = sh.make_shape(proto.shape_id(), tag_si);
        proto.set_shape_id(tag_shape);
        let tag_pos = proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("Promise").0)));
        proto.set_data_meta(tag_pos, PropAttributes::new(false, false, true));

        // %Promise% 构造器：proto = Function.prototype，静态方法 resolve/reject。
        // 自身属性顺序：length、name、prototype（CreateBuiltinFunction 顺序）。
        let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
        ctor.set_function(true);
        ctor.set_native_arg_count(1);
        // SAFETY: promise_constructor 是 NativeFn 函数项。
        ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(promise_constructor as *const ()) }));
        ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
        let length_si = sf.intern("length").0;
        let ctor_shape1 = sh.make_shape(ctor.shape_id(), length_si);
        ctor.set_shape_id(ctor_shape1);
        let lpos = ctor.push_prop(JsValue::int(1));
        ctor.set_data_meta(lpos, PropAttributes::new(false, false, true));
        let name_si = sf.intern("name").0;
        let ctor_shape2 = sh.make_shape(ctor.shape_id(), name_si);
        ctor.set_shape_id(ctor_shape2);
        let npos = ctor.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("Promise").0)));
        ctor.set_data_meta(npos, PropAttributes::new(false, false, true));
        let proto2_si = sf.intern("prototype").0;
        let ctor_shape3 = sh.make_shape(ctor.shape_id(), proto2_si);
        ctor.set_shape_id(ctor_shape3);
        ctor.push_prop(JsValue::undefined());
        ctor.set_data_meta(2u32, PropAttributes::new(false, false, false));
        oxide_kernel::bind_methods_static!(
            &mut ctor,
            sf,
            sh,
            fn_proto_val,
            ("resolve", promise_static_resolve as *const (), 1),
            ("reject", promise_static_reject as *const (), 1),
            ("all", promise_static_all as *const (), 1),
            ("race", promise_static_race as *const (), 1),
            ("allSettled", promise_static_all_settled as *const (), 1),
            ("any", promise_static_any as *const (), 1),
        );

        // 固定地址后互相接线：proto.constructor ↔ ctor.prototype。
        self.promise_proto = P::new(*proto);
        self.promise_constructor = P::new(*ctor);
        let proto_mut = unsafe { &mut *self.promise_proto.as_mut_ptr() };
        proto_mut.set_prop_at(0u32, JsValue::from_js_object(self.promise_constructor.as_ptr() as *mut JsObject));
        let ctor_mut = unsafe { &mut *self.promise_constructor.as_mut_ptr() };
        // prototype 槽位在 length/name 之后（下标 2）。
        ctor_mut.set_prop_at(2u32, JsValue::from_js_object(self.promise_proto.as_ptr() as *mut JsObject));

        // 绑定 global：槽已存在则更新（full_reset 未重建 global 时旧槽指向已弃 ctor）。
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        let global = unsafe { &mut *global_ptr };
        let si = self.kernel_core.perm_interner().intern("Promise").0;
        let ctor_val = JsValue::from_js_object(self.promise_constructor.as_ptr() as *mut JsObject);
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
            global.set_prop_at(pos, ctor_val);
        } else {
            let shape = self.kernel_core.shape_forge().make_shape(global.shape_id(), si);
            global.set_shape_id(shape);
            let pos = global.push_prop(ctor_val);
            // global 数据属性：writable:true、enumerable:false、configurable:true。
            global.set_data_meta(pos, PropAttributes::new(true, false, true));
            global.bump_generation();
        }

        // Promise.any 的拒绝路径依赖 AggregateError 内建。
        self.init_aggregate_error_intrinsics();
    }
}

/// 读取 Promise 的 settled 值（CLI 格式化用）：`Some((is_fulfilled, value))`，
/// Pending 返回 `None`。
pub fn promise_settled_value(obj: &JsObject) -> Option<(bool, JsValue)> {
    let state = promise_state_ref(obj)?;
    match state.state {
        PromiseStateKind::Pending => None,
        PromiseStateKind::Fulfilled => Some((true, state.result)),
        PromiseStateKind::Rejected => Some((false, state.result)),
    }
}

/// `Promise` 构造器：设置状态盒并同步调用 executor(resolve, reject)。
fn promise_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    // `new` 调用时 this 是 proto 链含 %Promise.prototype% 的新对象；裸调用拒绝。
    if !vm.has_promise_proto(this_val) {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "Promise must be called with new"));
    }
    let obj = unsafe { &mut *this_val.as_js_object_ptr() };
    obj.type_tag = JsObject::OBJ_TYPE_PROMISE;
    let resolve = vm.make_resolve_reject_fn(this_val, false, false);
    let reject = vm.make_resolve_reject_fn(this_val, true, false);
    let state = Box::new(PromiseState {
        state: PromiseStateKind::Pending,
        result: JsValue::undefined(),
        reactions: Vec::new(),
        resolve_fn: resolve,
        reject_fn: reject,
        already_resolved: false,
    });
    obj.set_native_data(Box::into_raw(state) as *mut u8);
    let executor = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !oxide_builtins::iterator::is_callable(executor) {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "Promise executor is not a function"));
    }
    match vm.call_function_sync(executor, JsValue::undefined(), &[resolve, reject]) {
        Ok(_) => NativeResult::Ok(this_val),
        Err(e) => {
            // executor 抛错：以抛出的值为拒绝原因（原值经 last_uncaught_value 保留）。
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            let _ = vm.reject_promise(this_val, exc);
            NativeResult::Ok(this_val)
        }
    }
}

/// resolve 闭包：`resolve(x)` 对目标 promise 执行 PromiseResolve。
/// 非委托闭包入口检查 alreadyResolved（首次调用置位，后续 no-op）；
/// 委托闭包（thenable 的 resolve）绕过该检查直接结算。
fn promise_resolve_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some(promise) = vm.promise_from_callee() else {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "resolve function is invalid"));
    };
    let delegated = vm.callee_delegated_flag();
    let x = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !delegated {
        // 非委托：alreadyResolved 守卫——首次调用置位，含 thenable 委托期间。
        let state = vm.promise_state_ptr(promise);
        let state = unsafe { &mut *state };
        if state.already_resolved {
            return NativeResult::Ok(JsValue::undefined());
        }
        state.already_resolved = true;
    }
    if let Err(e) = vm.resolve_promise(promise, x) {
        let exc = vm
            .last_uncaught_value
            .take()
            .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
        let _ = vm.reject_promise(promise, exc);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// reject 闭包：`reject(x)` 直接以 x 拒绝目标 promise。
fn promise_reject_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some(promise) = vm.promise_from_callee() else {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "reject function is invalid"));
    };
    let delegated = vm.callee_delegated_flag();
    let reason = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !delegated {
        let state = vm.promise_state_ptr(promise);
        let state = unsafe { &mut *state };
        if state.already_resolved {
            return NativeResult::Ok(JsValue::undefined());
        }
        state.already_resolved = true;
    }
    let _ = vm.reject_promise(promise, reason);
    NativeResult::Ok(JsValue::undefined())
}

/// `Promise.prototype.then(onFulfilled, onRejected)`：注册反应并返回派生 promise。
fn promise_then(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let on_fulfilled = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let on_rejected = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    match vm.perform_promise_then(this_val, on_fulfilled, on_rejected) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.prototype.catch(onRejected)`：等价 `then(undefined, onRejected)`，
/// 经 Invoke 语义调用 `this.then`（可被用户覆盖的 then）。
fn promise_catch(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let on_rejected = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match vm.invoke_then(this_val, &[JsValue::undefined(), on_rejected]) {
        Ok(result) => NativeResult::Ok(result),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.prototype.finally(onFinally)`：调 onFinally 后透传原值/原拒绝原因；
/// 经 Invoke 语义调用 `this.then`。
fn promise_finally(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let on_finally = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (then_finally, reject_finally) = if oxide_builtins::iterator::is_callable(on_finally) {
        (vm.make_finally_handler(on_finally, false), vm.make_finally_handler(on_finally, true))
    } else {
        (JsValue::undefined(), JsValue::undefined())
    };
    match vm.invoke_then(this_val, &[then_finally, reject_finally]) {
        Ok(result) => NativeResult::Ok(result),
        Err(err) => NativeResult::Err(err),
    }
}

impl Vm {
    /// Invoke 语义：`GetV(this, "then")` 后以原接收者调用，透传 getter/调用抛错。
    fn invoke_then(&mut self, this_val: JsValue, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let then_si = self.kernel_core.perm_interner().intern("then").0;
        // GetV：原始值先 ToObject 再读 then；getter 抛错透传原异常。
        let get_receiver = if this_val.is_object() {
            this_val
        } else {
            match oxide_runtime_api::to_object(this_val, self) {
                Ok(o) => o,
                Err(e) => return Err(oxide_builtins::error::create_type_error(self, &e)),
            }
        };
        let receiver_obj = unsafe { &*get_receiver.as_js_object_ptr() };
        let then = match self.ordinary_get(receiver_obj, then_si, this_val) {
            Ok(v) => v,
            Err(e) => {
                let exc = self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                return Err(exc);
            }
        };
        if !oxide_builtins::iterator::is_callable(then) {
            return Err(oxide_builtins::error::create_type_error(self, "then is not a function"));
        }
        match self.call_function_sync(then, this_val, args) {
            Ok(v) => Ok(v),
            Err(e) => Err(self
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e))),
        }
    }

    /// 构造 finally 处理器 native 闭包：调 onFinally 后，fulfill 角色返回原值，
    /// reject 角色重抛原拒绝原因。
    fn make_finally_handler(&mut self, on_finally: JsValue, reject_role: bool) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: promise_finally_handler 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(promise_finally_handler as *const ()) }));
        func.set_native_arg_count(0);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        let of_si = self.kernel_core.perm_interner().intern(ON_FINALLY_PROP).0;
        self.set_or_create_prop_value(obj, of_si, on_finally);
        let rj_si = self.kernel_core.perm_interner().intern(FINALLY_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, rj_si, JsValue::bool(reject_role));
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }

    /// 给 native 函数对象补 `length`/`name` 数据属性（length 先于 name，符合
    /// CreateBuiltinFunction 属性顺序）。
    pub(crate) fn add_fn_name_length(&mut self, obj: &mut JsObject, name: &str, length: u8) {
        let attrs = PropAttributes::new(false, false, true);
        let sh = self.kernel_core.shape_forge().as_ref();
        let sf = self.kernel_core.perm_interner().as_ref();
        let length_si = sf.intern("length").0;
        let length_shape = sh.make_shape(obj.shape_id(), length_si);
        obj.set_shape_id(length_shape);
        let lpos = obj.push_prop(JsValue::int(length as i32));
        obj.set_data_meta(lpos, attrs);
        let name_si = sf.intern("name").0;
        let name_shape = sh.make_shape(obj.shape_id(), name_si);
        obj.set_shape_id(name_shape);
        let npos = obj.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern(name).0)));
        obj.set_data_meta(npos, attrs);
    }

    /// 创建聚合静态方法（all/race/allSettled/any）的共享记录对象：剩余计数
    /// 初始 1，结果数组（race 无）与能力 resolve/reject 闭包全部存为自身属性。
    fn make_agg_record(&mut self, values: JsValue, resolve: JsValue, reject: JsValue) -> JsValue {
        let proto_val = JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        let obj = unsafe { &mut *ptr };
        let rem_si = self.kernel_core.perm_interner().intern(AGG_REMAINING_PROP).0;
        self.set_or_create_prop_value(obj, rem_si, JsValue::int(1));
        let val_si = self.kernel_core.perm_interner().intern(AGG_VALUES_PROP).0;
        self.set_or_create_prop_value(obj, val_si, values);
        let res_si = self.kernel_core.perm_interner().intern(AGG_RESOLVE_PROP).0;
        self.set_or_create_prop_value(obj, res_si, resolve);
        let rej_si = self.kernel_core.perm_interner().intern(AGG_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, rej_si, reject);
        JsValue::from_js_object(ptr)
    }

    /// 创建聚合静态方法的元素处理器闭包：携带共享记录与元素下标，`already` 标志
    /// 初始 false。内部状态属性非枚举且追加在 length/name 之后，保持内建函数
    /// "length 先于 name" 的属性序。
    fn make_agg_element_fn(&mut self, native_fn: NativeFn, record: JsValue, index: i32) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: native_fn 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(native_fn as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        let obj = unsafe { &mut *ptr };
        self.add_fn_name_length(obj, "", 1);
        let sh = self.kernel_core.shape_forge().as_ref();
        let attrs = PropAttributes::new(false, false, false);
        let rec_si = self.kernel_core.perm_interner().intern(AGG_RECORD_PROP).0;
        let rec_shape = sh.make_shape(obj.shape_id(), rec_si);
        obj.set_shape_id(rec_shape);
        let rec_pos = obj.push_prop(record);
        obj.set_data_meta(rec_pos, attrs);
        let idx_si = self.kernel_core.perm_interner().intern(AGG_INDEX_PROP).0;
        let idx_shape = sh.make_shape(obj.shape_id(), idx_si);
        obj.set_shape_id(idx_shape);
        let idx_pos = obj.push_prop(JsValue::int(index));
        obj.set_data_meta(idx_pos, attrs);
        let al_si = self.kernel_core.perm_interner().intern(AGG_ALREADY_PROP).0;
        let al_shape = sh.make_shape(obj.shape_id(), al_si);
        obj.set_shape_id(al_shape);
        let al_pos = obj.push_prop(JsValue::bool(false));
        obj.set_data_meta(al_pos, attrs);
        JsValue::from_js_object(ptr)
    }

    /// 构造 allSettled 的结算记录 `{status, <value_field>: value}` 普通对象。
    fn make_settled_record(&mut self, status: &str, value_field: &str, value: JsValue) -> JsValue {
        let proto_val = JsValue::from_js_object(self.session.builtin_world().object_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        let obj = unsafe { &mut *ptr };
        let status_si = self.kernel_core.perm_interner().intern("status").0;
        let status_val = self.new_string(status);
        self.set_or_create_prop_value(obj, status_si, status_val);
        let field_si = self.kernel_core.perm_interner().intern(value_field).0;
        self.set_or_create_prop_value(obj, field_si, value);
        JsValue::from_js_object(ptr)
    }

    /// 构造 AggregateError 实例（proto = %AggregateError.prototype%）：message 非
    /// undefined 时 ToString 建自身属性，errors 存为数据属性。
    fn make_aggregate_error(&mut self, errors: JsValue, message: JsValue) -> JsValue {
        let proto_val = JsValue::from_js_object(self.aggregate_error_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val));
        let obj = unsafe { &mut *ptr };
        // message/errors 为数据属性：writable、非枚举、configurable（CreateMethodProperty）。
        let attrs = PropAttributes::new(true, false, true);
        if !message.is_undefined() {
            let msg_str = oxide_runtime_api::to_string_full(message, self).unwrap_or_default();
            let msg_si = self.kernel_core.perm_interner().intern("message").0;
            let msg_val = self.new_string(&msg_str);
            let _ = self.define_data_property(obj, msg_si, msg_val, attrs);
        }
        let err_si = self.kernel_core.perm_interner().intern("errors").0;
        let _ = self.define_data_property(obj, err_si, errors, attrs);
        JsValue::from_js_object(ptr)
    }

    /// 把 errors 可迭代值收集为新数组（IterableToList）。不可迭代抛 TypeError。
    fn aggregate_errors_to_list(&mut self, errors: JsValue) -> Result<JsValue, JsValue> {
        let array_proto = JsValue::from_js_object(self.session.builtin_world().array_proto.as_ptr() as *mut JsObject);
        let ptr = self.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, array_proto, 0, self.epoch.bump()));
        let list = JsValue::from_js_object(ptr);
        let mut index = 0usize;
        oxide_builtins::iterator::iterate_elements(self, errors, |_vm, elem| {
            // SAFETY: list 是本函数新建的存活数组对象。
            let list_obj = unsafe { &mut *list.as_js_object_ptr() };
            list_obj.set_prop_at(index, elem);
            index += 1;
            Ok(())
        })?;
        Ok(list)
    }

    /// 初始化/重建 AggregateError 内建对象：`%AggregateError%` 构造器与
    /// `%AggregateError.prototype%`（proto = %Error.prototype%），绑定 global 槽。
    ///
    /// 与 `init_promise_intrinsics` 同生命周期（VM 创建 + full_reset），
    /// `Promise.any` 的拒绝路径依赖此内建。
    pub(crate) fn init_aggregate_error_intrinsics(&mut self) {
        let sf = self.kernel_core.perm_interner().as_ref();
        let sh = self.kernel_core.shape_forge().as_ref();
        let fn_proto_val = self.session.builtin_world().fn_proto_val();
        let error_proto_val = JsValue::from_js_object(self.session.builtin_world().error_proto.as_ptr() as *mut JsObject);

        // %AggregateError.prototype%：proto = %Error.prototype%，constructor/name/message 数据属性。
        let mut proto = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, error_proto_val));
        let ctor_si = sf.intern("constructor").0;
        let ctor_shape = sh.make_shape(proto.shape_id(), ctor_si);
        proto.set_shape_id(ctor_shape);
        proto.push_prop(JsValue::undefined());
        proto.set_data_meta(0u32, PropAttributes::new(true, false, true));
        let name_si = sf.intern("name").0;
        let name_shape = sh.make_shape(proto.shape_id(), name_si);
        proto.set_shape_id(name_shape);
        let name_pos = proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AggregateError").0)));
        proto.set_data_meta(name_pos, PropAttributes::new(true, false, true));
        let msg_si = sf.intern("message").0;
        let msg_shape = sh.make_shape(proto.shape_id(), msg_si);
        proto.set_shape_id(msg_shape);
        let msg_pos = proto.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("").0)));
        proto.set_data_meta(msg_pos, PropAttributes::new(true, false, true));

        // %AggregateError% 构造器：proto = %Function.prototype%，length 2。
        let mut ctor = Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto_val));
        ctor.set_function(true);
        ctor.type_tag = JsObject::OBJ_TYPE_CONSTRUCTOR;
        // SAFETY: aggregate_error_constructor 是 NativeFn 函数项。
        ctor.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(aggregate_error_constructor as *const ()) }));
        ctor.set_native_arg_count(2);
        let length_si = sf.intern("length").0;
        let ctor_shape1 = sh.make_shape(ctor.shape_id(), length_si);
        ctor.set_shape_id(ctor_shape1);
        let lpos = ctor.push_prop(JsValue::int(2));
        ctor.set_data_meta(lpos, PropAttributes::new(false, false, true));
        let name2_si = sf.intern("name").0;
        let ctor_shape2 = sh.make_shape(ctor.shape_id(), name2_si);
        ctor.set_shape_id(ctor_shape2);
        let npos = ctor.push_prop(JsValue::perm_string(sf.string_ptr(sf.intern("AggregateError").0)));
        ctor.set_data_meta(npos, PropAttributes::new(false, false, true));
        let proto2_si = sf.intern("prototype").0;
        let ctor_shape3 = sh.make_shape(ctor.shape_id(), proto2_si);
        ctor.set_shape_id(ctor_shape3);
        ctor.push_prop(JsValue::undefined());
        ctor.set_data_meta(2u32, PropAttributes::new(false, false, false));

        // 固定地址后互相接线：proto.constructor ↔ ctor.prototype。
        self.aggregate_error_proto = P::new(*proto);
        self.aggregate_error_constructor = P::new(*ctor);
        let proto_mut = unsafe { &mut *self.aggregate_error_proto.as_mut_ptr() };
        proto_mut.set_prop_at(
            0u32,
            JsValue::from_js_object(self.aggregate_error_constructor.as_ptr() as *mut JsObject),
        );
        let ctor_mut = unsafe { &mut *self.aggregate_error_constructor.as_mut_ptr() };
        ctor_mut.set_prop_at(2u32, JsValue::from_js_object(self.aggregate_error_proto.as_ptr() as *mut JsObject));

        // 绑定 global（槽已存在则更新）。
        let global_ptr = self.session.global_object().as_ptr() as *mut JsObject;
        let global = unsafe { &mut *global_ptr };
        let si = self.kernel_core.perm_interner().intern("AggregateError").0;
        let ctor_val = JsValue::from_js_object(self.aggregate_error_constructor.as_ptr() as *mut JsObject);
        if let Some(pos) = self.kernel_core.shape_forge().lookup_position(global.shape_id(), si) {
            global.set_prop_at(pos, ctor_val);
        } else {
            let shape = self.kernel_core.shape_forge().make_shape(global.shape_id(), si);
            global.set_shape_id(shape);
            let pos = global.push_prop(ctor_val);
            global.set_data_meta(pos, PropAttributes::new(true, false, true));
            global.bump_generation();
        }
    }
}

/// finally 处理器闭包：读自身 prop 的 onFinally 与角色，调用后透传/重抛。
fn promise_finally_handler(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "finally handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let of_si = vm.kernel_core.perm_interner().intern(ON_FINALLY_PROP).0;
    let on_finally = vm.resolve_property(callee_obj, of_si).unwrap_or(JsValue::undefined());
    let rj_si = vm.kernel_core.perm_interner().intern(FINALLY_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, rj_si)
        .map_or(false, oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match vm.call_function_sync(on_finally, JsValue::undefined(), &[]) {
        Ok(_) => {
            if is_reject {
                NativeResult::Err(value)
            } else {
                NativeResult::Ok(value)
            }
        }
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            NativeResult::Err(exc)
        }
    }
}

/// `Promise.resolve(x)`：`x` 为原生 Promise 且 `x.constructor === this` 时直接返回；
/// 否则按 `this`（构造器）建能力并 PromiseResolve。
fn promise_static_resolve(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let x = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if vm.is_promise_value(x) {
        let x_obj = unsafe { &*x.as_js_object_ptr() };
        let ctor_si = vm.kernel_core.perm_interner().intern("constructor").0;
        let x_ctor = match vm.ordinary_get(x_obj, ctor_si, x) {
            Ok(c) => c,
            Err(e) => {
                let exc = vm
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        if oxide_runtime_api::same_value(x_ctor, ctor) {
            return NativeResult::Ok(x);
        }
    }
    let (promise, resolve, _) = match vm.new_promise_capability_with_ctor(ctor) {
        Ok(t) => t,
        Err(err) => return NativeResult::Err(err),
    };
    // PromiseResolve：调用能力 resolve（自定义构造器可能产生非原生 promise）。
    match vm.call_function_sync(resolve, JsValue::undefined(), &[x]) {
        Ok(_) => NativeResult::Ok(promise),
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            NativeResult::Err(exc)
        }
    }
}

/// `Promise.reject(x)`：按 `this`（构造器）建能力并直接拒绝。
fn promise_static_reject(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let x = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let (promise, _, reject) = match vm.new_promise_capability_with_ctor(ctor) {
        Ok(t) => t,
        Err(err) => return NativeResult::Err(err),
    };
    match vm.call_function_sync(reject, JsValue::undefined(), &[x]) {
        Ok(_) => NativeResult::Ok(promise),
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            NativeResult::Err(exc)
        }
    }
}

/// `Promise.all(iterable)`：全部元素结算后以结果数组完成，任一拒绝则拒绝。
fn promise_static_all(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine(vm, ctor, iterable, AggregateKind::All) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.race(iterable)`：任一元素先结算即按该结果完成/拒绝。
fn promise_static_race(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine(vm, ctor, iterable, AggregateKind::Race) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.allSettled(iterable)`：全部元素结算后以 `{status, value|reason}` 数组完成。
fn promise_static_all_settled(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine(vm, ctor, iterable, AggregateKind::AllSettled) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.any(iterable)`：任一元素完成即完成；全部拒绝则以 AggregateError(errors) 拒绝。
fn promise_static_any(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let iterable = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match perform_promise_combine(vm, ctor, iterable, AggregateKind::Any) {
        Ok(promise) => NativeResult::Ok(promise),
        Err(err) => NativeResult::Err(err),
    }
}

/// 聚合静态方法的共享核心：迭代可迭代输入，逐元素调 `C.resolve` 建 promise 并注册反应，
/// 按模式在全部/任一结算后交付能力 promise。
///
/// # 步骤
/// 1. 建能力（NewPromiseCapability(C) 抛错同步上抛），取 C.resolve 一次（不可调用 → 拒绝）。
/// 2. 建共享记录（剩余计数 = 1 + 元素数，结果数组，能力 resolve/reject）。
/// 3. 迭代元素：逐元素 `Call(C.resolve, C, «elem»)` 后 `Invoke(promise.then, 元素处理器)`。
/// 4. 迭代完成时哨兵计数递减；为 0 时结算（all/allSettled 完成数组，any 拒绝 AggregateError）。
///
/// # 边界与前提
/// - 迭代异常按来源区分是否 IteratorClose：next 调用与 then 注册抛错关闭迭代器，
///   迭代结果对象的 done/value 读取抛错按规范直接拒绝（不关闭）。
/// - 元素处理器同步触发时（thenable 直接调 onFulfilled），剩余计数先 +1 再注册，
///   保证中途结算仍能等齐全部元素。
fn perform_promise_combine(
    vm: &mut Vm, ctor: JsValue, iterable: JsValue, kind: AggregateKind,
) -> Result<JsValue, JsValue> {
    let (promise, resolve, reject) = vm.new_promise_capability_with_ctor(ctor)?;

    // 取 C.resolve 一次（getter 抛错或不可调用 → 拒绝能力）。
    let resolve_si = vm.kernel_core.perm_interner().intern("resolve").0;
    let promise_resolve = if ctor.is_object() {
        // SAFETY: ctor 是存活对象。
        let ctor_obj = unsafe { &*ctor.as_js_object_ptr() };
        match vm.ordinary_get(ctor_obj, resolve_si, ctor) {
            Ok(v) => v,
            Err(e) => {
                let exc = vm
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
                let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
                return Ok(promise);
            }
        }
    } else {
        JsValue::undefined()
    };
    if !oxide_builtins::iterator::is_callable(promise_resolve) {
        let exc = oxide_builtins::error::create_type_error(vm, "resolve is not a function");
        let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
        return Ok(promise);
    }

    // 结果数组（race 不需要）。
    let values = if kind == AggregateKind::Race {
        JsValue::undefined()
    } else {
        let array_proto = JsValue::from_js_object(vm.session.builtin_world().array_proto.as_ptr() as *mut JsObject);
        let ptr = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, array_proto, 0, vm.epoch.bump()));
        JsValue::from_js_object(ptr)
    };
    let record = vm.make_agg_record(values, resolve, reject);

    // GetIterator 失败 → 拒绝（尚无迭代器可关闭）。
    let iterator = match oxide_builtins::iterator::make_iterator_for_value(vm, iterable) {
        Ok(it) => it,
        Err(err) => {
            let _ = vm.call_function_sync(reject, JsValue::undefined(), &[err]);
            return Ok(promise);
        }
    };

    enum IterAbrupt {
        Close(JsValue),
        NoClose(JsValue),
    }
    let mut index: i32 = 0;
    let result: Result<(), IterAbrupt> = (|| {
        let next_si = vm.kernel_core.perm_interner().intern("next").0;
        let done_si = vm.kernel_core.perm_interner().intern("done").0;
        let value_si = vm.kernel_core.perm_interner().intern("value").0;
        // SAFETY: iterator 是存活对象。
        let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
        let next_fn = vm
            .ordinary_get(iter_obj, next_si, iterator)
            .map_err(|e| IterAbrupt::Close(agg_engine_error(vm, &e)))?;
        loop {
            let step = vm
                .call_function_sync(next_fn, iterator, &[])
                .map_err(|e| IterAbrupt::Close(agg_engine_error(vm, &e)))?;
            if !step.is_object() {
                return Err(IterAbrupt::NoClose(oxide_builtins::error::create_type_error(
                    vm,
                    "iterator result is not an object",
                )));
            }
            // SAFETY: step 是存活对象。
            let step_obj = unsafe { &*step.as_js_object_ptr() };
            let done = vm
                .ordinary_get(step_obj, done_si, step)
                .map_err(|e| IterAbrupt::NoClose(agg_engine_error(vm, &e)))?;
            if oxide_runtime_api::to_boolean(done) {
                return Ok(());
            }
            let elem = vm
                .ordinary_get(step_obj, value_si, step)
                .map_err(|e| IterAbrupt::NoClose(agg_engine_error(vm, &e)))?;
            let next_promise = vm
                .call_function_sync(promise_resolve, ctor, &[elem])
                .map_err(|e| IterAbrupt::Close(agg_engine_error(vm, &e)))?;

            // 剩余计数先 +1 再注册：thenable 同步结算时 handler 依赖该计数已含自身。
            let cur = agg_read_remaining(vm, record);
            agg_write_remaining(vm, record, cur + 1);
            match kind {
                AggregateKind::All => {
                    let re = vm.make_agg_element_fn(promise_all_resolve_element, record, index);
                    vm.invoke_then(next_promise, &[re, reject]).map_err(IterAbrupt::Close)?;
                }
                AggregateKind::Race => {
                    // race 直接把能力 resolve/reject 作为反应处理器（规范无元素函数包装，
                    // 能力结算一次后其余调用为 no-op）。
                    vm.invoke_then(next_promise, &[resolve, reject]).map_err(IterAbrupt::Close)?;
                }
                AggregateKind::AllSettled => {
                    let re = vm.make_agg_element_fn(promise_all_settled_resolve_element, record, index);
                    let rj = vm.make_agg_element_fn(promise_all_settled_reject_element, record, index);
                    vm.invoke_then(next_promise, &[re, rj]).map_err(IterAbrupt::Close)?;
                }
                AggregateKind::Any => {
                    // any 的完成侧直接用能力 resolve；拒绝侧用带 AlreadyCalled 的 reject 元素。
                    let rj = vm.make_agg_element_fn(promise_any_reject_element, record, index);
                    vm.invoke_then(next_promise, &[resolve, rj]).map_err(IterAbrupt::Close)?;
                }
            }
            index += 1;
        }
    })();

    match result {
        Ok(()) => {
            // 迭代完成：哨兵 1 递减；为 0 时结算（race 无计数语义）。
            let remaining = agg_read_remaining(vm, record) - 1;
            agg_write_remaining(vm, record, remaining);            match kind {
                AggregateKind::All | AggregateKind::AllSettled => {
                    if remaining == 0 {
                        let values = agg_record_val(vm, record, AGG_VALUES_PROP);
                        agg_call_resolve(vm, record, values);
                    }
                }
                AggregateKind::Any => {
                    if remaining == 0 {
                        let errors = agg_record_val(vm, record, AGG_VALUES_PROP);
                        let agg = vm.make_aggregate_error(errors, JsValue::undefined());
                        let rej = agg_record_val(vm, record, AGG_REJECT_PROP);
                        let _ = vm.call_function_sync(rej, JsValue::undefined(), &[agg]);
                    }
                }
                AggregateKind::Race => {}
            }
        }
        Err(IterAbrupt::Close(e)) => {
            close_agg_iterator(vm, iterator);
            let rej = agg_record_val(vm, record, AGG_REJECT_PROP);
            let _ = vm.call_function_sync(rej, JsValue::undefined(), &[e]);
        }
        Err(IterAbrupt::NoClose(e)) => {
            let rej = agg_record_val(vm, record, AGG_REJECT_PROP);
            let _ = vm.call_function_sync(rej, JsValue::undefined(), &[e]);
        }
    }
    Ok(promise)
}

/// 聚合元素处理器的公共 prologue：取共享记录与下标；`already` 已置位返回 None
/// （元素函数只生效一次），否则置位后返回记录。
fn agg_element_state(vm: &mut Vm) -> Option<(JsValue, i32)> {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return None;
    }
    let callee_ptr = callee.as_js_object_ptr();
    let rec_si = vm.kernel_core.perm_interner().intern(AGG_RECORD_PROP).0;
    let idx_si = vm.kernel_core.perm_interner().intern(AGG_INDEX_PROP).0;
    let al_si = vm.kernel_core.perm_interner().intern(AGG_ALREADY_PROP).0;
    // SAFETY: callee 是当前调用的存活函数对象。
    let callee_ref = unsafe { &*callee_ptr };
    if vm.resolve_property(callee_ref, al_si).map_or(false, oxide_runtime_api::to_boolean) {
        return None;
    }
    let record = vm.resolve_property(callee_ref, rec_si).unwrap_or(JsValue::undefined());
    let index = vm
        .resolve_property(callee_ref, idx_si)
        .map_or(0, |v| if v.is_int() { v.as_int() } else { 0 });
    // SAFETY: 同一 callee 对象，此处仅写 already 标志。
    vm.set_or_create_prop_value(unsafe { &mut *callee_ptr }, al_si, JsValue::bool(true));
    Some((record, index))
}

/// 读取记录对象属性（非对象/缺失返回 undefined）。
fn agg_record_val(vm: &Vm, record: JsValue, prop: &str) -> JsValue {
    if !record.is_object() {
        return JsValue::undefined();
    }
    let si = vm.kernel_core.perm_interner().intern(prop).0;
    // SAFETY: record 是存活对象。
    vm.resolve_property(unsafe { &*record.as_js_object_ptr() }, si)
        .unwrap_or(JsValue::undefined())
}

/// 读取记录剩余计数。
fn agg_read_remaining(vm: &Vm, record: JsValue) -> i32 {
    let v = agg_record_val(vm, record, AGG_REMAINING_PROP);
    if v.is_int() {
        v.as_int()
    } else {
        0
    }
}

/// 写回记录剩余计数。
fn agg_write_remaining(vm: &mut Vm, record: JsValue, n: i32) {
    if !record.is_object() {
        return;
    }
    let si = vm.kernel_core.perm_interner().intern(AGG_REMAINING_PROP).0;
    // SAFETY: record 是存活对象。
    vm.set_or_create_prop_value(unsafe { &mut *record.as_js_object_ptr() }, si, JsValue::int(n));
}

/// 调用记录上的能力 resolve；抛错时改以能力 reject 拒绝（IfAbruptRejectPromise）。
fn agg_call_resolve(vm: &mut Vm, record: JsValue, arg: JsValue) {
    let resolve = agg_record_val(vm, record, AGG_RESOLVE_PROP);
    if let Err(e) = vm.call_function_sync(resolve, JsValue::undefined(), &[arg]) {
        let exc = vm
            .last_uncaught_value
            .take()
            .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
        let reject = agg_record_val(vm, record, AGG_REJECT_PROP);
        let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
    }
}

/// 把 `call_function_sync` 的错误文本恢复为原始异常值。
fn agg_engine_error(vm: &mut Vm, err: &str) -> JsValue {
    vm.last_uncaught_value
        .take()
        .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, err))
}

/// IteratorClose：调用迭代器的 `return()`（可调用时），忽略其抛错。
fn close_agg_iterator(vm: &mut Vm, iterator: JsValue) {
    if !iterator.is_object() {
        return;
    }
    let return_si = vm.kernel_core.perm_interner().intern("return").0;
    // SAFETY: iterator 是存活对象。
    let iter_obj = unsafe { &*iterator.as_js_object_ptr() };
    if let Ok(ret) = vm.ordinary_get(iter_obj, return_si, iterator) {
        if oxide_builtins::iterator::is_callable(ret) {
            let _ = vm.call_function_sync(ret, iterator, &[]);
        }
    }
}

/// `Promise.all` resolve 元素：写 `values[index]`，剩余计数归零时完成数组。
fn promise_all_resolve_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = agg_record_val(vm, record, AGG_VALUES_PROP);
    if values.is_object() {
        let key_si = vm.kernel_core.perm_interner().intern(&index.to_string()).0;
        // SAFETY: values 是存活数组对象；直写元素区不触发数组 setter。
        vm.set_or_create_prop_value(unsafe { &mut *values.as_js_object_ptr() }, key_si, value);
    }
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        agg_call_resolve(vm, record, values);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Promise.race` 使用能力 resolve/reject 直连，无独立元素函数。

/// `Promise.allSettled` resolve 元素：写 `{status:'fulfilled', value}` 记录。
fn promise_all_settled_resolve_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = agg_record_val(vm, record, AGG_VALUES_PROP);
    if values.is_object() {
        let settled = vm.make_settled_record("fulfilled", "value", value);
        let key_si = vm.kernel_core.perm_interner().intern(&index.to_string()).0;
        // SAFETY: values 是存活数组对象。
        vm.set_or_create_prop_value(unsafe { &mut *values.as_js_object_ptr() }, key_si, settled);
    }
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        agg_call_resolve(vm, record, values);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Promise.allSettled` reject 元素：写 `{status:'rejected', reason}` 记录。
fn promise_all_settled_reject_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let reason = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = agg_record_val(vm, record, AGG_VALUES_PROP);
    if values.is_object() {
        let settled = vm.make_settled_record("rejected", "reason", reason);
        let key_si = vm.kernel_core.perm_interner().intern(&index.to_string()).0;
        // SAFETY: values 是存活数组对象。
        vm.set_or_create_prop_value(unsafe { &mut *values.as_js_object_ptr() }, key_si, settled);
    }
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        agg_call_resolve(vm, record, values);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Promise.any` reject 元素：写 `errors[index]`，全部拒绝时以 AggregateError 拒绝。
fn promise_any_reject_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let reason = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let errors = agg_record_val(vm, record, AGG_VALUES_PROP);
    if errors.is_object() {
        let key_si = vm.kernel_core.perm_interner().intern(&index.to_string()).0;
        // SAFETY: errors 是存活数组对象。
        vm.set_or_create_prop_value(unsafe { &mut *errors.as_js_object_ptr() }, key_si, reason);
    }
    let remaining = agg_read_remaining(vm, record) - 1;
    agg_write_remaining(vm, record, remaining);
    if remaining == 0 {
        let agg = vm.make_aggregate_error(errors, JsValue::undefined());
        let reject = agg_record_val(vm, record, AGG_REJECT_PROP);
        let _ = vm.call_function_sync(reject, JsValue::undefined(), &[agg]);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `AggregateError(errors, message)` 构造器：message 非 undefined 时先 ToString 建
/// 自身属性，再把 errors 可迭代收集为 `errors` 数据属性。
fn aggregate_error_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let errors = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let message = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let this = if this_val.is_object() {
        this_val.as_js_object_ptr()
    } else {
        let proto_val = JsValue::from_js_object(vm.aggregate_error_proto.as_ptr() as *mut JsObject);
        vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, proto_val))
    };
    if !message.is_undefined() {
        let msg_str = match oxide_runtime_api::to_string_full(message, vm) {
            Ok(s) => s,
            Err(e) => {
                let exc = vm
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
                return NativeResult::Err(exc);
            }
        };
        let msg_si = vm.kernel_core.perm_interner().intern("message").0;
        let msg_val = vm.new_string(&msg_str);
        // SAFETY: this 是本次构造的存活对象。
        let _ = vm.define_data_property(
            unsafe { &mut *this },
            msg_si,
            msg_val,
            PropAttributes::new(true, false, true),
        );
    }
    let errors_list = match vm.aggregate_errors_to_list(errors) {
        Ok(list) => list,
        Err(err) => return NativeResult::Err(err),
    };
    let err_si = vm.kernel_core.perm_interner().intern("errors").0;
    let _ = vm.define_data_property(
        unsafe { &mut *this },
        err_si,
        errors_list,
        PropAttributes::new(true, false, true),
    );
    NativeResult::Ok(JsValue::from_js_object(this))
}

/// GetCapabilitiesExecutor：捕获 resolve/reject 到自身 prop；能力已持有任一
/// 非 undefined 值时再次调用抛 TypeError。
fn capability_executor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "executor is invalid"));
    }
    let obj = unsafe { &*callee.as_js_object_ptr() };
    let res_si = vm.kernel_core.perm_interner().intern(CAP_RESOLVE_PROP).0;
    let rej_si = vm.kernel_core.perm_interner().intern(CAP_REJECT_PROP).0;
    let held = vm.resolve_property(obj, res_si).map_or(false, |v| !v.is_undefined())
        || vm.resolve_property(obj, rej_si).map_or(false, |v| !v.is_undefined());
    if held {
        return NativeResult::Err(oxide_builtins::error::create_type_error(
            vm,
            "Promise capability executor has already been called",
        ));
    }
    let resolve = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let reject = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let obj = unsafe { &mut *callee.as_js_object_ptr() };
    vm.set_or_create_prop_value(obj, res_si, resolve);
    vm.set_or_create_prop_value(obj, rej_si, reject);
    NativeResult::Ok(JsValue::undefined())
}

// ── session GC 支撑：状态盒中的 JsValues 作为 Promise 对象边追踪 ──

fn promise_state_ref(obj: &JsObject) -> Option<&PromiseState> {
    let ptr = obj.native_data() as *const PromiseState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: native_data 由 Box::into_raw 分配，生命周期与 Promise 对象一致。
        Some(unsafe { &*ptr })
    }
}

fn promise_state_mut(obj: &JsObject) -> Option<&mut PromiseState> {
    let ptr = obj.native_data() as *mut PromiseState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: 同上，改写发生在 GC 移动/清扫期间，对象仍存活。
        Some(unsafe { &mut *ptr })
    }
}

/// Promise 状态盒内所有对象引用（GC mark 边）。
pub(crate) fn promise_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = promise_state_ref(obj) else {
        return Vec::new();
    };
    let mut edges = Vec::new();
    let push = |v: JsValue, edges: &mut Vec<JsValue>| {
        if v.is_object() {
            edges.push(v);
        }
    };
    push(state.result, &mut edges);
    push(state.resolve_fn, &mut edges);
    push(state.reject_fn, &mut edges);
    for r in &state.reactions {
        push(r.promise, &mut edges);
        push(r.resolve, &mut edges);
        push(r.reject, &mut edges);
        push(r.handler, &mut edges);
    }
    edges
}

/// 用转发函数重写状态盒中的所有 JsValue（session GC 移动式清扫 / promote 用）。
pub(crate) fn rewrite_promise_native(obj: &JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue) {
    let Some(state) = promise_state_mut(obj) else {
        return;
    };
    state.result = rewrite(state.result);
    state.resolve_fn = rewrite(state.resolve_fn);
    state.reject_fn = rewrite(state.reject_fn);
    for r in &mut state.reactions {
        r.promise = rewrite(r.promise);
        r.resolve = rewrite(r.resolve);
        r.reject = rewrite(r.reject);
        r.handler = rewrite(r.handler);
    }
}

/// 深拷贝状态盒到新对象（promote / sweep 用）：新对象持独立 Box，源盒可安全释放。
pub(crate) fn clone_promise_native_with_rewrite(
    old: &JsObject, new: &mut JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue,
) {
    let Some(state) = promise_state_ref(old) else {
        return;
    };
    let cloned = PromiseState {
        state: state.state,
        result: rewrite(state.result),
        reactions: state
            .reactions
            .iter()
            .map(|r| PromiseReaction {
                promise: rewrite(r.promise),
                resolve: rewrite(r.resolve),
                reject: rewrite(r.reject),
                handler: rewrite(r.handler),
                is_fulfill: r.is_fulfill,
            })
            .collect(),
        resolve_fn: rewrite(state.resolve_fn),
        reject_fn: rewrite(state.reject_fn),
        already_resolved: state.already_resolved,
    };
    new.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 释放 Promise 状态盒（对象被回收时），返回释放字节数。
pub(crate) fn drop_promise_native(obj: &JsObject) -> u64 {
    if !obj.is_promise_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut PromiseState;
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: 指针来自 Box::into_raw，只在对象被回收时释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    std::mem::size_of::<PromiseState>() as u64
        + state.reactions.capacity() as u64 * std::mem::size_of::<PromiseReaction>() as u64
}

/// 供外部遍历微任务队列（GC mark/rewrite 用）。
pub(crate) fn for_each_job_value(job: &Microtask, mut f: impl FnMut(JsValue)) {
    match job {
        Microtask::Reaction {
            handler,
            argument,
            resolve,
            reject,
            ..
        } => {
            f(*handler);
            f(*argument);
            f(*resolve);
            f(*reject);
        }
        Microtask::Thenable {
            thenable,
            then,
            resolve,
            reject,
        } => {
            f(*thenable);
            f(*then);
            f(*resolve);
            f(*reject);
        }
    }
}

/// 供外部改写微任务队列中的 JsValue（session GC sweep 用）。
pub(crate) fn rewrite_job_values(job: &mut Microtask, mut rewrite: impl FnMut(JsValue) -> JsValue) {
    match job {
        Microtask::Reaction {
            handler,
            argument,
            resolve,
            reject,
            ..
        } => {
            *handler = rewrite(*handler);
            *argument = rewrite(*argument);
            *resolve = rewrite(*resolve);
            *reject = rewrite(*reject);
        }
        Microtask::Thenable {
            thenable,
            then,
            resolve,
            reject,
        } => {
            *thenable = rewrite(*thenable);
            *then = rewrite(*then);
            *resolve = rewrite(*resolve);
            *reject = rewrite(*reject);
        }
    }
}
