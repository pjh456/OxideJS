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
/// finally 处理器上存 onFinally 回调的属性名。
const ON_FINALLY_PROP: &str = "__oxide_on_finally__";
/// finally 处理器上区分 reject 角色的属性名。
const FINALLY_REJECT_PROP: &str = "__oxide_finally_reject__";
/// 能力 executor 上捕获 resolve/reject 的属性名。
const CAP_RESOLVE_PROP: &str = "__oxide_cap_resolve__";
const CAP_REJECT_PROP: &str = "__oxide_cap_reject__";

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
        });
        obj.set_native_data(Box::into_raw(state) as *mut u8);
        JsValue::from_js_object(ptr)
    }

    /// 新建 Promise 能力 `(promise, resolve, reject)`：resolve/reject 为携带
    /// 目标 promise 的 native 闭包，同时写入状态盒供 thenable 委托取用。
    fn new_promise_capability(&mut self) -> (JsValue, JsValue, JsValue) {
        let promise = self.create_promise_object();
        let resolve = self.make_resolve_reject_fn(promise, false);
        let reject = self.make_resolve_reject_fn(promise, true);
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
                .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e))),
        }
    }

    /// 构造携带目标 promise 的 native 闭包（resolve 或 reject 角色）。
    fn make_resolve_reject_fn(&mut self, promise: JsValue, reject_role: bool) -> JsValue {
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

    /// 取出 Promise 状态盒指针（调用方须先校验 `is_promise_value`）。
    fn promise_state_ptr(&self, promise: JsValue) -> *mut PromiseState {
        unsafe { (*promise.as_js_object_ptr()).native_data() as *mut PromiseState }
    }

    /// 是否原生 Promise 对象。
    fn is_promise_value(&self, v: JsValue) -> bool {
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
                        .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
                    return self.reject_promise(promise, exc);
                }
            };
            if oxide_builtins::iterator::is_callable(then) {
                let state = self.promise_state_ptr(promise);
                let state = unsafe { &*state };
                let resolve = state.resolve_fn;
                let reject = state.reject_fn;
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

    /// `PerformPromiseThen` 核心：注册 fulfill/reject 两条反应；已 settle 则直接入队。
    fn perform_promise_then(
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
                            .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
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
                            .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
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
    let resolve = vm.make_resolve_reject_fn(this_val, false);
    let reject = vm.make_resolve_reject_fn(this_val, true);
    let state = Box::new(PromiseState {
        state: PromiseStateKind::Pending,
        result: JsValue::undefined(),
        reactions: Vec::new(),
        resolve_fn: resolve,
        reject_fn: reject,
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
                .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
            let _ = vm.reject_promise(this_val, exc);
            NativeResult::Ok(this_val)
        }
    }
}

/// resolve 闭包：`resolve(x)` 对目标 promise 执行 PromiseResolve。
fn promise_resolve_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some(promise) = vm.promise_from_callee() else {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "resolve function is invalid"));
    };
    let x = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if let Err(e) = vm.resolve_promise(promise, x) {
        let exc = vm
            .last_uncaught_value
            .take()
            .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
        let _ = vm.reject_promise(promise, exc);
    }
    NativeResult::Ok(JsValue::undefined())
}

/// reject 闭包：`reject(x)` 直接以 x 拒绝目标 promise。
fn promise_reject_closure(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some(promise) = vm.promise_from_callee() else {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "reject function is invalid"));
    };
    let reason = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
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
                    .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e));
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
                .unwrap_or_else(|| oxide_builtins::error::create_error(self, &e))),
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
    fn add_fn_name_length(&mut self, obj: &mut JsObject, name: &str, length: u8) {
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
                .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
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
                    .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
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
                .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
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
                .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
            NativeResult::Err(exc)
        }
    }
}

/// GetCapabilitiesExecutor：捕获 resolve/reject 到自身 prop，重复调用抛 TypeError。
fn capability_executor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "executor is invalid"));
    }
    let obj = unsafe { &*callee.as_js_object_ptr() };
    let res_si = vm.kernel_core.perm_interner().intern(CAP_RESOLVE_PROP).0;
    if vm.resolve_property(obj, res_si).is_some_and(|v| v.is_object()) {
        return NativeResult::Err(oxide_builtins::error::create_type_error(
            vm,
            "Promise capability executor has already been called",
        ));
    }
    let resolve = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let reject = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let obj = unsafe { &mut *callee.as_js_object_ptr() };
    let rej_si = vm.kernel_core.perm_interner().intern(CAP_REJECT_PROP).0;
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
