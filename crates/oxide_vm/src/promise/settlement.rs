//! Promise 能力与结算：能力构造、resolve/reject 闭包构造，
//! PromiseResolve / fulfill / reject 结算路径。
//!
//! 结算单次性：闭包首调置位 `already_resolved`，`state != Pending`
//! 最终守卫；结算后反应入微任务队列，并沿 `promoted_clone` 链传导克隆。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use crate::native::NativeFn;
use crate::vm::Vm;

use super::{
    Microtask, PromiseState, PromiseStateKind, CAP_REJECT_PROP, CAP_RESOLVE_PROP, DELEGATED_PROP, PROMISE_PROP,
};

impl Vm {
    /// 新建 Promise 能力 `(promise, resolve, reject)`：resolve/reject 为携带
    /// 目标 promise 的 native 闭包，同时写入状态盒供 thenable 委托取用。
    pub(crate) fn new_promise_capability(&mut self) -> (JsValue, JsValue, JsValue) {
        let promise = self.create_promise_object();
        let resolve = self.make_resolve_reject_fn(promise, false, false);
        let reject = self.make_resolve_reject_fn(promise, true, false);
        let state = self.promise_state_ptr(promise);
        // SAFETY: `state` 来自 create_promise_object 写入的状态盒，非空且随对象存活；此处块内唯一可变借用，无别名。
        let state = unsafe { &mut *state };
        state.resolve_fn = resolve;
        state.reject_fn = reject;
        (promise, resolve, reject)
    }

    /// NewPromiseCapability(C)：按构造器 C 建能力。C 为内置 Promise 时走直接路径，
    /// 否则经 GetCapabilitiesExecutor 间接构造 `new C(executor)`。
    pub(super) fn new_promise_capability_with_ctor(
        &mut self, ctor: JsValue,
    ) -> Result<(JsValue, JsValue, JsValue), JsValue> {
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
            // SAFETY: `executor` 由 make_capability_executor 新建，为存活的 arena 函数
            // 对象、指针非空；此处只读 CAP_* 属性，不跨 GC/reset 点。
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
        // SAFETY: `alloc_object` 返回非空 arena 指针，本次 native 调用内不搬移；借出期间无别名。
        let obj = unsafe { &mut *ptr };
        self.add_fn_name_length(obj, "", 2);
        JsValue::from_js_object(ptr)
    }

    /// 构造携带目标 promise 的 native 闭包（resolve 或 reject 角色）。
    /// `delegated` 为 true 表示 thenable 委托用的结算代理：绕过 alreadyResolved
    /// （委托是唯一真正的结算路径，由 then 的 resolve/reject 触发），但 state != Pending
    /// 仍阻止二次结算。
    pub(super) fn make_resolve_reject_fn(&mut self, promise: JsValue, reject_role: bool, delegated: bool) -> JsValue {
        let native_fn: NativeFn = if reject_role { promise_reject_closure } else { promise_resolve_closure };
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: native_fn 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(native_fn as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        // SAFETY: `alloc_object` 返回非空 arena 指针；后续属性/shape 分配不改对象地址，无别名。
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
        // SAFETY: `is_object` 已保证指针非空且指向存活对象；只读 PROMISE_PROP，不跨 GC/reset。
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
        // SAFETY: `is_object` 已保证指针非空且指向存活对象；只读 DELEGATED_PROP，不跨 GC/reset。
        let obj = unsafe { &*callee.as_js_object_ptr() };
        let si = self.kernel_core.perm_interner().intern(DELEGATED_PROP).0;
        self.resolve_property(obj, si)
            .map(|v| v.is_bool() && v.as_bool())
            .unwrap_or(false)
    }

    /// 取出 Promise 状态盒指针（调用方须先校验 `is_promise_value`）。
    pub(super) fn promise_state_ptr(&self, promise: JsValue) -> *mut PromiseState {
        // SAFETY: 调用方已按文档前提校验 `is_promise_value`，对象指针非空且 native_data
        // 为 create_promise_object 或 promise_constructor 写入的 Box<PromiseState>；裸指针由调用方即时解引用。
        unsafe { (*promise.as_js_object_ptr()).native_data() as *mut PromiseState }
    }

    /// 是否原生 Promise 对象。
    pub(crate) fn is_promise_value(&self, v: JsValue) -> bool {
        v.is_object() && {
            let ptr = v.as_js_object_ptr();
            // SAFETY: `is_object` 与 `ptr` 非空双守卫保证可解引用；只读 type_tag 判定，立即消费。
            !ptr.is_null() && unsafe { &*ptr }.is_promise_obj()
        }
    }

    /// 校验值的原型链含 `%Promise.prototype%`（`new Promise` 构造出的实例特征）。
    pub(super) fn has_promise_proto(&self, v: JsValue) -> bool {
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
            // SAFETY: 循环内 `is_object` 与 `ptr` 非空守卫保证可解引用；只读 proto()，链深受深度上限约束。
            cursor = unsafe { &*ptr }.proto();
        }
        false
    }

    /// 入队一条微任务（追加到队尾，保证 FIFO）。
    pub(super) fn enqueue(&mut self, job: Microtask) {
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
            // SAFETY: `is_object` 保证指针非空且指向存活对象；只读 `then` 属性，本次读取内即时消费。
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
                // SAFETY: 状态盒随已建 Promise 存活、指针非空；本块唯一可变借用，写 already_resolved 后结束。
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
            // SAFETY: `state_ptr` 取自 promise_state_ptr，盒非空且随 promise 存活；本块唯一可变借用，块末释放。
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
        // 原件若已晋升出克隆，把结算传导到克隆：克隆上晋升后新挂的反应方随此触发
        // （顶层 var 读克隆，原件反应已在本处直接触发，不重复传导）。
        // SAFETY: 同一 `state_ptr`，前块可变借用已结束；盒仍存活，promoted_clone
        // 由 promise_native_edges 的 mark 边保证同轮存活（mod.rs 模块头不变量）。
        let clone_ptr = unsafe { (*state_ptr).promoted_clone };
        if !clone_ptr.is_null() {
            let clone = JsValue::from_js_object(clone_ptr);
            let _ = self.fulfill_promise(clone, value);
        }
        Ok(())
    }

    /// 以 `reason` 拒绝 promise：只 settle 一次，完成后触发该态反应入队。
    pub(crate) fn reject_promise(&mut self, promise: JsValue, reason: JsValue) -> Result<(), String> {
        let state_ptr = self.promise_state_ptr(promise);
        let settled = {
            // SAFETY: `state_ptr` 取自 promise_state_ptr，盒非空且随 promise 存活；本块唯一可变借用，块末释放。
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
        // 原件若已晋升出克隆，把拒绝传导到克隆（同 fulfill：顶层 var 读克隆）。
        // SAFETY: 同一 `state_ptr`，前块可变借用已结束；盒仍存活，promoted_clone
        // 由 promise_native_edges 的 mark 边保证同轮存活（mod.rs 模块头不变量）。
        let clone_ptr = unsafe { (*state_ptr).promoted_clone };
        if !clone_ptr.is_null() {
            let clone = JsValue::from_js_object(clone_ptr);
            let _ = self.reject_promise(clone, reason);
        }
        Ok(())
    }

    /// PromiseResolve（%Promise%, value）核心：value 为原生 Promise 时读取其
    /// `constructor`，与 %Promise% 相同则原样返回；否则新建能力并经能力 resolve
    /// 结算（thenable 委托）。constructor getter / 结算抛错时透传原异常值。
    pub(crate) fn promise_resolve(&mut self, value: JsValue) -> Result<JsValue, JsValue> {
        if self.is_promise_value(value) {
            // SAFETY: `is_promise_value` 保证指针非空且 type_tag 为 Promise；只读 `constructor`，即时消费。
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
        // SAFETY: 非委托分支的唯一可变借用；状态盒随 promise 存活，写 already_resolved 后结束。
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
        // SAFETY: 非委托分支的唯一可变借用；状态盒随 promise 存活，写 already_resolved 后结束。
        let state = unsafe { &mut *state };
        if state.already_resolved {
            return NativeResult::Ok(JsValue::undefined());
        }
        state.already_resolved = true;
    }
    let _ = vm.reject_promise(promise, reason);
    NativeResult::Ok(JsValue::undefined())
}

/// GetCapabilitiesExecutor：捕获 resolve/reject 到自身 prop；能力已持有任一
/// 非 undefined 值时再次调用抛 TypeError。
fn capability_executor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "executor is invalid"));
    }
    // SAFETY: `callee` 来自 reg(254) 且 `is_object` 守卫保证指针非空存活；只读 CAP_* 判定，不跨 GC/reset。
    let obj = unsafe { &*callee.as_js_object_ptr() };
    let res_si = vm.kernel_core.perm_interner().intern(CAP_RESOLVE_PROP).0;
    let rej_si = vm.kernel_core.perm_interner().intern(CAP_REJECT_PROP).0;
    let held = vm.resolve_property(obj, res_si).is_some_and(|v| !v.is_undefined())
        || vm.resolve_property(obj, rej_si).is_some_and(|v| !v.is_undefined());
    if held {
        return NativeResult::Err(oxide_builtins::error::create_type_error(
            vm,
            "Promise capability executor has already been called",
        ));
    }
    let resolve = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let reject = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // SAFETY: 同一 `callee`，前一读借用已结束；对象存活且本块唯一可变借用，写 CAP_* 无别名。
    let obj = unsafe { &mut *callee.as_js_object_ptr() };
    vm.set_or_create_prop_value(obj, res_si, resolve);
    vm.set_or_create_prop_value(obj, rej_si, reject);
    NativeResult::Ok(JsValue::undefined())
}
