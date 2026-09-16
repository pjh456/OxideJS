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
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::make_int_key;
use oxide_types::value::JsValue;

use crate::native::NativeFn;
use crate::vm::Vm;

mod constructor;
mod jobs;
mod settlement;

pub(crate) use jobs::{for_each_job_value, rewrite_job_values};

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
    /// 结算传导链指针：非空时指向本 promise 晋升出的 session 克隆，原件结算时
    /// 结果沿它传导到克隆；再晋升场景旧克隆的残留反应被取回归并进新克隆，
    /// 指针更新为最新克隆。非 pending 原件恒为空指针。本指针在
    /// `promise_native_edges` 登记为 mark 边：克隆恒为 session 表成员，凭这条
    /// 边在原件仍存活时被同轮置活，结算不会读到悬垂克隆。
    pub promoted_clone: *mut JsObject,
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
    /// `PerformPromiseThen` 入口（内置能力）：注册 fulfill/reject 反应并返回派生
    /// promise。能力恒为 `%Promise%`——供引擎内部 await 机制（async /
    /// async generator / AsyncFromSync）使用，规范上这些路径由调用方提供能力，
    /// 不读 `this.constructor`，避免 await 子类 promise 时意外经子类构造器派生。
    pub(crate) fn perform_promise_then(
        &mut self, this_val: JsValue, on_fulfilled: JsValue, on_rejected: JsValue,
    ) -> Result<JsValue, JsValue> {
        let capability = self.new_promise_capability();
        self.perform_promise_then_cap(this_val, on_fulfilled, on_rejected, capability)
    }

    /// `PerformPromiseThen` 核心：以调用方给定能力注册 fulfill/reject 两条反应；
    /// 已 settle 则直接入队。能力三元组由调用方提供——`promise_then_species`
    /// 传 species 派生能力，内部 await 路径经薄包装传内置能力。
    fn perform_promise_then_cap(
        &mut self, this_val: JsValue, on_fulfilled: JsValue, on_rejected: JsValue,
        capability: (JsValue, JsValue, JsValue),
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
        let (new_promise, resolve, reject) = capability;
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

    /// `Promise.prototype.then` 的派生入口：IsPromise 校验后按 `this.constructor`
    /// 选派生构造器（SpeciesConstructor 的 constructor 语义），建能力并注册反应。
    fn promise_then_species(
        &mut self, this_val: JsValue, on_fulfilled: JsValue, on_rejected: JsValue,
    ) -> Result<JsValue, JsValue> {
        if !self.is_promise_value(this_val) {
            return Err(oxide_builtins::error::create_type_error(
                self,
                "Method Promise.prototype.then called on incompatible receiver",
            ));
        }
        let ctor = self.species_constructor(this_val)?;
        let capability = self.new_promise_capability_with_ctor(ctor)?;
        self.perform_promise_then_cap(this_val, on_fulfilled, on_rejected, capability)
    }

    /// 读 `promise.constructor` 选派生构造器（SpeciesConstructor 的 constructor
    /// 语义）：constructor 为 undefined 时退回内置 `%Promise%`，非可构造函数抛
    /// TypeError。constructor getter 抛错时透传原异常值。
    ///
    /// # 边界与前提
    /// - 调用方已做 IsPromise 校验，`promise` 必为 Promise 对象。
    /// - constructor 为 null/非对象/非构造器均抛 TypeError。
    ///
    /// # 注意事项
    /// - 不读 `@@species`：`%Promise%` 未注册 species getter，子类经原型链读到
    ///   undefined 会错误退回内置；`@@species` 改写支持待补
    ///   `Promise[Symbol.species]` getter 时一并落地。
    fn species_constructor(&mut self, promise: JsValue) -> Result<JsValue, JsValue> {
        let obj = unsafe { &*promise.as_js_object_ptr() };
        let ctor_si = self.kernel_core.perm_interner().intern("constructor").0;
        let ctor = match self.ordinary_get(obj, ctor_si, promise) {
            Ok(c) => c,
            Err(e) => {
                let exc = self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                return Err(exc);
            }
        };
        if ctor.is_undefined() {
            return Ok(JsValue::from_js_object(self.promise_constructor.as_ptr() as *mut JsObject));
        }
        if !is_constructor_value(ctor) {
            return Err(oxide_builtins::error::create_type_error(self, "Species constructor is not a constructor"));
        }
        Ok(ctor)
    }
}

/// IsConstructor 近似判定：可调用、非 arrow，且 native 函数须带构造器 tag
/// （与 `construct_ctor` 的校验一致）。
fn is_constructor_value(c: JsValue) -> bool {
    if !c.is_object() {
        return false;
    }
    let c_obj = unsafe { &*c.as_js_object_ptr() };
    c_obj.is_function()
        && !c_obj.is_arrow()
        && !(c_obj.native_fn().is_some() && c_obj.type_tag != JsObject::OBJ_TYPE_CONSTRUCTOR)
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

/// `Promise.prototype.then(onFulfilled, onRejected)`：按 `this.constructor` 派生
/// 并注册反应，返回派生 promise（子类实例走子类构造器）。
fn promise_then(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let on_fulfilled = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let on_rejected = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    match vm.promise_then_species(this_val, on_fulfilled, on_rejected) {
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
        let error_proto_val =
            JsValue::from_js_object(self.session.builtin_world().error_proto.as_ptr() as *mut JsObject);

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
        Self::swap_intrinsic_proto(&mut self.aggregate_error_proto, *proto);
        Self::swap_intrinsic_proto(&mut self.aggregate_error_constructor, *ctor);
        let proto_mut = unsafe { &mut *self.aggregate_error_proto.as_mut_ptr() };
        proto_mut
            .set_prop_at(0u32, JsValue::from_js_object(self.aggregate_error_constructor.as_ptr() as *mut JsObject));
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
        .is_some_and(oxide_runtime_api::to_boolean);
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

/// `Promise.withResolvers()`：按 `this` 构造器建能力（非构造器抛 TypeError），
/// 返回 `{promise, resolve, reject}` 普通对象（proto 为 `%Object.prototype%`）。
///
/// # 步骤
/// 1. 取 `this` 为构造器 C，经 `new_promise_capability_with_ctor` 建能力
/// 2. 创建普通对象，依次写入 promise / resolve / reject 三个数据属性
///
/// # 边界与前提
/// - C 非构造器（普通值 / arrow / 非构造 native）时抛 TypeError
/// - 返回值属性为默认数据描述符（writable/enumerable/configurable 均 true）
fn promise_static_with_resolvers(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let ctor = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let (promise, resolve, reject) = match vm.new_promise_capability_with_ctor(ctor) {
        Ok(t) => t,
        Err(err) => return NativeResult::Err(err),
    };
    let object_proto = JsValue::from_js_object(vm.session.builtin_world().object_proto.as_ptr() as *mut JsObject);
    let ptr = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, object_proto));
    let obj = unsafe { &mut *ptr };
    let sf = vm.kernel_core.perm_interner().as_ref();
    let sh = vm.kernel_core.shape_forge().as_ref();
    for (name, val) in [("promise", promise), ("resolve", resolve), ("reject", reject)] {
        let si = sf.intern(name).0;
        let shape = sh.make_shape(obj.shape_id(), si);
        obj.set_shape_id(shape);
        obj.push_prop(val);
    }
    NativeResult::Ok(JsValue::from_js_object(ptr))
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
            agg_write_remaining(vm, record, remaining);
            match kind {
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
    if vm
        .resolve_property(callee_ref, al_si)
        .is_some_and(oxide_runtime_api::to_boolean)
    {
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
            // return() 的抛错被忽略，其值不得外泄进槽覆盖在途异常。
            let saved_uncaught = vm.last_uncaught_value.take();
            let _ = vm.call_function_sync(ret, iterator, &[]);
            vm.last_uncaught_value = saved_uncaught;
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
        let key_si = make_int_key(index as u32);
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

// `Promise.race` 使用能力 resolve/reject 直连，无独立元素函数。

/// `Promise.allSettled` resolve 元素：写 `{status:'fulfilled', value}` 记录。
fn promise_all_settled_resolve_element(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let Some((record, index)) = agg_element_state(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let values = agg_record_val(vm, record, AGG_VALUES_PROP);
    if values.is_object() {
        let settled = vm.make_settled_record("fulfilled", "value", value);
        let key_si = make_int_key(index as u32);
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
        let key_si = make_int_key(index as u32);
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
        let key_si = make_int_key(index as u32);
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
        let _ = vm.define_data_property(unsafe { &mut *this }, msg_si, msg_val, PropAttributes::new(true, false, true));
    }
    let errors_list = match vm.aggregate_errors_to_list(errors) {
        Ok(list) => list,
        Err(err) => return NativeResult::Err(err),
    };
    let err_si = vm.kernel_core.perm_interner().intern("errors").0;
    let _ = vm.define_data_property(unsafe { &mut *this }, err_si, errors_list, PropAttributes::new(true, false, true));
    NativeResult::Ok(JsValue::from_js_object(this))
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

#[expect(clippy::mut_from_ref)]
fn promise_state_mut(obj: &JsObject) -> Option<&mut PromiseState> {
    let ptr = obj.native_data() as *mut PromiseState;
    if ptr.is_null() {
        None
    } else {
        // SAFETY: 同上，改写发生在 GC 移动/清扫期间，对象仍存活。
        Some(unsafe { &mut *ptr })
    }
}

/// Promise 状态盒内全部值边（GC mark 边）：对象/字符串/BigInt 均产出，
/// 消费侧按值类型分发到对象栈与存活集。
pub(crate) fn promise_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(state) = promise_state_ref(obj) else {
        return Vec::new();
    };
    let mut edges = Vec::new();
    edges.push(state.result);
    edges.push(state.resolve_fn);
    edges.push(state.reject_fn);
    // 结算链指针：克隆恒为 session 表成员，凭这条边在原件/旧克隆仍存活时被
    // 同轮置活，结算传导不会读到悬垂克隆。
    if !state.promoted_clone.is_null() {
        edges.push(JsValue::from_js_object(state.promoted_clone));
    }
    for r in &state.reactions {
        edges.push(r.promise);
        edges.push(r.resolve);
        edges.push(r.reject);
        edges.push(r.handler);
    }
    edges
}

/// 用转发函数重写状态盒中的所有 JsValue（session GC 移动式清扫 / promote 用）。
///
/// 结算链指针是裸指针边、不在本函数改写面：晋升路径克隆新分配于 session
/// arena（原件地址不变），同 arena 改写场景指针天然有效；搬移换址场景由
/// 调用方在重写完成后经 `repoint_promise_promoted_clone` 按转发表重定位。
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
///
/// 源的反应**迁移**（非复制）进新对象：源盒随 epoch 释放，留在源上的反应会
/// 永久丢失、原件侧消费者永不触发；每条反应的 JsValue 随状态盒其余字段同一
/// pass 晋升/改写。结算链指针按原值保留（晋升场景由
/// `migrate_settlement_to_newest_clone` 接链，搬移场景按转发表重定位）。
pub(crate) fn clone_promise_native_with_rewrite(
    old: &JsObject, new: &mut JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue,
) {
    // 先取全部 Copy 字段，避免与随后的可变 take 别名校验冲突。
    let (state_kind, prev_clone, result, resolve_fn, reject_fn, already_resolved) = {
        let Some(state) = promise_state_ref(old) else {
            return;
        };
        (
            state.state,
            state.promoted_clone,
            state.result,
            state.resolve_fn,
            state.reject_fn,
            state.already_resolved,
        )
    };

    // 源反应迁入新对象：源盒随 epoch 释放，反应留源即永久丢失。
    let mut reactions: Vec<PromiseReaction> = Vec::new();
    if let Some(src) = promise_state_mut(old) {
        for r in std::mem::take(&mut src.reactions) {
            reactions.push(rewrite_reaction(r, &mut rewrite));
        }
    }

    let cloned = PromiseState {
        state: state_kind,
        result: rewrite(result),
        reactions,
        resolve_fn: rewrite(resolve_fn),
        reject_fn: rewrite(reject_fn),
        already_resolved,
        // 结算链指针按原值保留（不改写源指针）：晋升后继步
        // `migrate_settlement_to_newest_clone` 负责接链与更新源指针，
        // 搬移后继步按转发表重定位。
        promoted_clone: prev_clone,
    };
    new.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 晋升路径专属：新克隆取代前一克隆成为结算枢纽——前一克隆的残留反应取回归并
/// 进新克隆，旧克隆的结算改链到新克隆，原件的传导指针指向新克隆。任一结算
/// 入口（原件 / 旧克隆 / 新克隆）都沿链落到携全部反应的最新克隆，反应恰触发
/// 一次。
///
/// # 边界与前提
/// - 仅 epoch 原件晋升后调用（`promote_object_inner` 的 Promise 分支，紧随
///   `clone_promise_native_with_rewrite`）；session 搬移（GC sweep）不改结算
///   拓扑，不得调用。
/// - 源非 pending 时无事可做直接返回（已结算原件不再传导）。
pub(crate) fn migrate_settlement_to_newest_clone(
    old: &JsObject, new: &mut JsObject, mut rewrite: impl FnMut(JsValue) -> JsValue,
) {
    // 链上的前克隆 = 源当前指向的克隆（克隆步只保留不改写源指针）。
    let (pending, prev) = {
        let Some(src) = promise_state_ref(old) else {
            return;
        };
        (src.state == PromiseStateKind::Pending, src.promoted_clone)
    };
    if !pending {
        return;
    }
    // 前克隆残留并入本克隆（旧克隆腾空，防双触发），并把它的结算改链到新克隆。
    if !prev.is_null() {
        // SAFETY: prev 是早前晋升产生的 session 克隆，session arena 存活期内有效。
        if let Some(prev_state) = promise_state_mut(unsafe { &*prev }) {
            if let Some(dst) = promise_state_mut(new) {
                for r in std::mem::take(&mut prev_state.reactions) {
                    dst.reactions.push(rewrite_reaction(r, &mut rewrite));
                }
            }
            prev_state.promoted_clone = new as *mut JsObject;
        }
    }
    // 新克隆即最新：自身无后继，原件传导指针指向它。
    if let Some(dst) = promise_state_mut(new) {
        dst.promoted_clone = std::ptr::null_mut();
    }
    if let Some(src) = promise_state_mut(old) {
        src.promoted_clone = new as *mut JsObject;
    }
}

/// 按给定转发改写结算链指针（裸指针边、非 JsValue），session GC 搬移后调用。
pub(crate) fn repoint_promise_promoted_clone(obj: &JsObject, forward: impl FnOnce(*mut JsObject) -> *mut JsObject) {
    let Some(state) = promise_state_mut(obj) else {
        return;
    };
    if !state.promoted_clone.is_null() {
        state.promoted_clone = forward(state.promoted_clone);
    }
}

/// 改写单条反应的四条 JsValue 引用（派生 promise 能力与处理器），源反应迁移
/// 与再晋升合并共用。
fn rewrite_reaction<F: FnMut(JsValue) -> JsValue>(r: PromiseReaction, rewrite: &mut F) -> PromiseReaction {
    PromiseReaction {
        promise: rewrite(r.promise),
        resolve: rewrite(r.resolve),
        reject: rewrite(r.reject),
        handler: rewrite(r.handler),
        is_fulfill: r.is_fulfill,
    }
}

/// 只读核算 Promise 状态盒字节（不释放），供 GC 账目核算。
pub(crate) fn promise_native_size(obj: &JsObject) -> u64 {
    if !obj.is_promise_obj() {
        return 0;
    }
    let ptr = obj.native_data() as *mut PromiseState;
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        std::mem::size_of::<PromiseState>() as u64
            + (*ptr).reactions.capacity() as u64 * std::mem::size_of::<PromiseReaction>() as u64
    }
}

/// 释放 Promise 状态盒（对象被回收时），返回释放字节数。
pub(crate) fn drop_promise_native(obj: &JsObject) -> u64 {
    let bytes = promise_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let ptr = obj.native_data() as *mut PromiseState;
    // SAFETY: ptr 非空（promise_native_size 已验证），Box::from_raw 恰好释放一次。
    let state = unsafe { Box::from_raw(ptr) };
    drop(state);
    bytes
}
