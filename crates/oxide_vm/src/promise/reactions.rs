//! Promise then 链：反应注册（PerformPromiseThen）、species 构造器
//! 派生、finally 处理与原型方法闭包（then/catch/finally）。
//!
//! `finally` 处理器为携带 onFinally 回调的 native 闭包，角色以属性标志区分；
//! `invoke_then` 经 Invoke 语义调用用户可覆盖的 `then`。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::vm::Vm;

use super::{
    is_constructor_value, promise_state_ref, Microtask, PromiseReaction, PromiseStateKind, FINALLY_REJECT_PROP,
    ON_FINALLY_PROP,
};

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
pub(super) fn promise_then(vm: &mut Vm, args: &[u8]) -> NativeResult {
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
pub(super) fn promise_catch(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let on_rejected = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match vm.invoke_then(this_val, &[JsValue::undefined(), on_rejected]) {
        Ok(result) => NativeResult::Ok(result),
        Err(err) => NativeResult::Err(err),
    }
}

/// `Promise.prototype.finally(onFinally)`：调 onFinally 后透传原值/原拒绝原因；
/// 经 Invoke 语义调用 `this.then`。
pub(super) fn promise_finally(vm: &mut Vm, args: &[u8]) -> NativeResult {
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
    pub(super) fn invoke_then(&mut self, this_val: JsValue, args: &[JsValue]) -> Result<JsValue, JsValue> {
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
