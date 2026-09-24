//! Promise then 链：反应注册（PerformPromiseThen）、species 构造器
//! 派生、finally 处理与原型方法闭包（then/catch/finally）。
//!
//! `finally` 处理器为携带 onFinally 回调与派生构造器的 native 闭包，角色以
//! 属性标志区分；处理器经 PromiseResolve 解包 onFinally 返回值后以直通
//! thunk 调 `Invoke(p, "then")` 透传原值/重抛原原因；`invoke_then` 经
//! Invoke 语义调用用户可覆盖的 `then`。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::private_key::{make_well_known_symbol_key, WELL_KNOWN_SYMBOL_SPECIES};
use oxide_types::value::JsValue;

use crate::vm::Vm;

use super::{
    is_constructor_value, promise_state_ref, Microtask, PromiseReaction, PromiseStateKind, FINALLY_CTOR_PROP,
    FINALLY_REJECT_PROP, FINALLY_THUNK_VALUE_PROP, ON_FINALLY_PROP,
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
            // SAFETY: this_val 已过 is_promise_value 校验，state_ptr 取自 promise_state_ptr；状态盒非空且随对象存活，本块唯一可变借用，读完 state/result 即结束。
            let state = unsafe { &mut *state_ptr };
            (state.state, state.result)
        };
        match kind {
            PromiseStateKind::Pending => {
                // SAFETY: 同一 state_ptr，前一可变借用已结束；状态盒非空且随 this_val 存活，此处仅向 reactions 追加两条反应，无别名。
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

    /// SpeciesConstructor(promise, %Promise%)：读 `promise.constructor` 为 C，
    /// C 为 undefined 时退回内置 `%Promise%`，C 非对象（含 null）抛 TypeError；
    /// 再从 C 读 `@@species` 为 S，S 为 undefined/null 时取 C，S 非构造器抛
    /// TypeError。constructor 与 `@@species` getter 抛错时透传原异常值。
    ///
    /// # 边界与前提
    /// - 调用方已做 IsPromise 校验，`promise` 必为 Promise 对象。
    ///
    /// # 注意事项
    /// - 标准链零漂移：`%Promise%` 自身 `@@species` getter 返回 receiver，
    ///   子类构造器继承同一 getter，终值与 C 本身一致；仅显式覆写/新定义
    ///   `@@species` 时行为改变。
    fn species_constructor(&mut self, promise: JsValue) -> Result<JsValue, JsValue> {
        // SAFETY: 调用方已按函数文档前提做 is_promise_value 校验，指针非空且指向存活 Promise 对象；此处只读 constructor。
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
        if !ctor.is_object() {
            return Err(oxide_builtins::error::create_type_error(
                self,
                "The .constructor property is not an object",
            ));
        }
        // SAFETY: is_object 保证指针非空且指向存活对象；此处只读 @@species，不跨 GC/reset。
        let c_obj = unsafe { &*ctor.as_js_object_ptr() };
        let species_si = make_well_known_symbol_key(WELL_KNOWN_SYMBOL_SPECIES);
        let s = match self.ordinary_get(c_obj, species_si, ctor) {
            Ok(v) => v,
            Err(e) => {
                let exc = self
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_from_text(self, &e));
                return Err(exc);
            }
        };
        if s.is_undefined() || s.is_null() {
            return Ok(ctor);
        }
        if !is_constructor_value(s) {
            return Err(oxide_builtins::error::create_type_error(self, "Species constructor is not a constructor"));
        }
        Ok(s)
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

/// `Promise.prototype.finally(onFinally)`：调 onFinally 后经 PromiseResolve
/// 解包其返回值，再透传原值/原拒绝原因；经 Invoke 语义调用 `this.then`。
///
/// # 步骤
/// 1. this 非 Object → TypeError。
/// 2. C = ? SpeciesConstructor(this, %Promise%)。
/// 3. onFinally 不可调用 → 双处理器均取 onFinally 自身（值原样传给 then）。
/// 4. 否则建 fulfill/reject 两个 finally 处理器闭包。
/// 5. 返回 ? Invoke(this, "then", « thenFinally, catchFinally »)。
pub(super) fn promise_finally(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let on_finally = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !this_val.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(
            vm,
            "Method Promise.prototype.finally called on incompatible receiver",
        ));
    }
    let ctor = match vm.species_constructor(this_val) {
        Ok(c) => c,
        Err(err) => return NativeResult::Err(err),
    };
    let (then_finally, catch_finally) = if oxide_builtins::iterator::is_callable(on_finally) {
        (
            vm.make_finally_handler(on_finally, ctor, false),
            vm.make_finally_handler(on_finally, ctor, true),
        )
    } else {
        (on_finally, on_finally)
    };
    match vm.invoke_then(this_val, &[then_finally, catch_finally]) {
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
        // SAFETY: get_receiver 两分支均为对象（is_object 原值或 to_object 返回对象），指针非空且存活；此处只读 then 属性。
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

    /// 构造 finally 处理器 native 闭包（CreateBuiltinFunction(closure, 1, "")）：
    /// 调 onFinally 后经 PromiseResolve(ctor, 返回值) 解包，再以直通 thunk
    /// 调 Invoke(p, "then")——fulfill 角色透传原值，reject 角色重抛原拒绝原因。
    fn make_finally_handler(&mut self, on_finally: JsValue, ctor: JsValue, reject_role: bool) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: promise_finally_handler 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(promise_finally_handler as *const ()) }));
        func.set_native_arg_count(1);
        let ptr = self.alloc_object(func);
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；本次 native 调用内不搬移，借出期间无别名。
        let obj = unsafe { &mut *ptr };
        let of_si = self.kernel_core.perm_interner().intern(ON_FINALLY_PROP).0;
        self.set_or_create_prop_value(obj, of_si, on_finally);
        let c_si = self.kernel_core.perm_interner().intern(FINALLY_CTOR_PROP).0;
        self.set_or_create_prop_value(obj, c_si, ctor);
        let rj_si = self.kernel_core.perm_interner().intern(FINALLY_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, rj_si, JsValue::bool(reject_role));
        self.add_fn_name_length(obj, "", 1);
        JsValue::from_js_object(ptr)
    }

    /// 构造 finally 直通 thunk（CreateBuiltinFunction(closure, 0, "")）：
    /// fulfill 角色返回携带值，reject 角色抛携带的拒绝原因。
    fn make_finally_thunk(&mut self, reject_role: bool, value: JsValue) -> JsValue {
        let fn_proto = self.session.builtin_world().fn_proto_val();
        let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
        func.set_function(true);
        // SAFETY: finally_thunk 是 NativeFn 函数项。
        func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(finally_thunk as *const ()) }));
        func.set_native_arg_count(0);
        let ptr = self.alloc_object(func);
        // SAFETY: ptr 由 alloc_object 新建，返回非空 arena 指针；本次 native 调用内不搬移，借出期间无别名。
        let obj = unsafe { &mut *ptr };
        let v_si = self.kernel_core.perm_interner().intern(FINALLY_THUNK_VALUE_PROP).0;
        self.set_or_create_prop_value(obj, v_si, value);
        let rj_si = self.kernel_core.perm_interner().intern(FINALLY_REJECT_PROP).0;
        self.set_or_create_prop_value(obj, rj_si, JsValue::bool(reject_role));
        self.add_fn_name_length(obj, "", 0);
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

/// finally 处理器闭包：读自身 prop 的 onFinally/构造器/角色，调 onFinally 后
/// 经 PromiseResolve(ctor, 返回值) 解包，再以直通 thunk 调 Invoke(p, "then")
/// 透传原值/重抛原拒绝原因；onFinally 抛错时原值上抛。
fn promise_finally_handler(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "finally handler is invalid"));
    }
    // SAFETY: callee 来自 reg(254) 且 is_object 守卫保证指针非空存活；此处只读 ON_FINALLY/FINALLY_CTOR/FINALLY_REJECT 属性，不跨 GC/reset。
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let of_si = vm.kernel_core.perm_interner().intern(ON_FINALLY_PROP).0;
    let on_finally = vm.resolve_property(callee_obj, of_si).unwrap_or(JsValue::undefined());
    let c_si = vm.kernel_core.perm_interner().intern(FINALLY_CTOR_PROP).0;
    let ctor = vm
        .resolve_property(callee_obj, c_si)
        .unwrap_or_else(|| JsValue::from_js_object(vm.promise_constructor.as_ptr() as *mut JsObject));
    let rj_si = vm.kernel_core.perm_interner().intern(FINALLY_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, rj_si)
        .is_some_and(oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    // 调 onFinally，抛错透传原异常值。
    let result = match vm.call_function_sync(on_finally, JsValue::undefined(), &[]) {
        Ok(r) => r,
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_from_text(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    // PromiseResolve(ctor, result)：原生 promise 且 constructor 同值时快路径原样返回。
    let p = match vm.promise_resolve_ctor(ctor, result) {
        Ok(p) => p,
        Err(exc) => return NativeResult::Err(exc),
    };
    // 直通 thunk + Invoke(p, "then", « thunk »)。
    let thunk = vm.make_finally_thunk(is_reject, value);
    match vm.invoke_then(p, &[thunk]) {
        Ok(v) => NativeResult::Ok(v),
        Err(exc) => NativeResult::Err(exc),
    }
}

/// finally 直通 thunk：读自身 prop 的携带值，fulfill 角色返回、reject 角色抛出。
fn finally_thunk(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "finally thunk is invalid"));
    }
    // SAFETY: callee 来自 reg(254) 且 is_object 守卫保证指针非空存活；此处只读携带值/角色属性，不跨 GC/reset。
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let v_si = vm.kernel_core.perm_interner().intern(FINALLY_THUNK_VALUE_PROP).0;
    let value = vm.resolve_property(callee_obj, v_si).unwrap_or(JsValue::undefined());
    let rj_si = vm.kernel_core.perm_interner().intern(FINALLY_REJECT_PROP).0;
    let is_reject = vm
        .resolve_property(callee_obj, rj_si)
        .is_some_and(oxide_runtime_api::to_boolean);
    if is_reject {
        NativeResult::Err(value)
    } else {
        NativeResult::Ok(value)
    }
}
