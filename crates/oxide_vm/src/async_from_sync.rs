//! AsyncFromSyncIterator：把同步迭代器包装为异步迭代器，供 for-await-of 回退消费。
//!
//! GetAsyncIterator 对没有 `@@asyncIterator` 的普通可迭代对象（数组/字符串等）
//! 走同步迭代协议取迭代器，再包一层 AsyncFromSyncIterator：其 `next`/`return`/`throw`
//! 转发内层同步迭代器并把结果包成 Promise（非对象结果按 TypeError 拒绝），
//! `@@asyncIterator` 返回自身。

use oxide_builtins::iterator::make_iter_result;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::private_key::make_well_known_symbol_key;
use oxide_types::value::JsValue;

use crate::vm::Vm;

/// AsyncFromSyncIterator 对象上存内层同步迭代器的属性名。
const INNER_PROP: &str = "__inner__";
/// `@@asyncIterator` 的 well-known symbol 键序号。
const ASYNC_ITERATOR_SYMBOL_ID: u32 = 8;

/// GetAsyncIterator（ECMA-262）：取 `value[@@asyncIterator]`，不可调用时回退同步
/// 迭代器并包 AsyncFromSyncIterator。
///
/// # 步骤
/// 1. ToObject 后读取 `@@asyncIterator`，可调用则调用并校验结果为对象。
/// 2. 否则经同步迭代协议取迭代器包装器，包成 AsyncFromSyncIterator。
///
/// # 返回值
/// - `Ok(iterator)`：异步迭代器对象；
/// - `Err`：`@@asyncIterator` getter/调用抛错或 ToObject 失败，透传原异常值。
pub(crate) fn make_async_iterator(vm: &mut Vm, value: JsValue) -> Result<JsValue, JsValue> {
    let obj_value = if value.is_object() {
        value
    } else {
        match oxide_runtime_api::to_object(value, vm) {
            Ok(o) => o,
            Err(e) => return Err(oxide_builtins::error::create_type_error(vm, &e)),
        }
    };
    let obj = unsafe { &*obj_value.as_js_object_ptr() };
    let aiter_key = make_well_known_symbol_key(ASYNC_ITERATOR_SYMBOL_ID);
    let method = match vm.ordinary_get(obj, aiter_key, obj_value) {
        Ok(m) => m,
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_type_error(vm, &e));
            return Err(exc);
        }
    };
    if oxide_builtins::iterator::is_callable(method) {
        let iterator = match vm.call_function_sync(method, obj_value, &[]) {
            Ok(it) => it,
            Err(e) => {
                let exc = vm
                    .last_uncaught_value
                    .take()
                    .unwrap_or_else(|| oxide_builtins::error::create_type_error(vm, &e));
                return Err(exc);
            }
        };
        if !iterator.is_object() {
            return Err(oxide_builtins::error::create_type_error(
                vm,
                "Result of the Symbol.asyncIterator method is not an object",
            ));
        }
        return Ok(iterator);
    }
    let sync = oxide_builtins::iterator::make_iterator_for_value(vm, value)?;
    Ok(vm.create_async_from_sync_iterator(sync))
}

impl Vm {
    /// 创建 AsyncFromSyncIterator 包装器：挂内层同步迭代器，装 next/return/throw 与
    /// `@@asyncIterator`（返回自身）方法。
    pub(crate) fn create_async_from_sync_iterator(&mut self, sync_iter: JsValue) -> JsValue {
        let next_fn = make_wrapper_fn(self, async_from_sync_next as *const ());
        let return_fn = make_wrapper_fn(self, async_from_sync_return as *const ());
        let throw_fn = make_wrapper_fn(self, async_from_sync_throw as *const ());
        let self_fn = make_wrapper_fn(self, async_from_sync_symbol_async_iterator as *const ());
        let object_proto = self.session.builtin_world().object_proto.as_ptr() as *mut JsObject;
        let ptr = self.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::from_js_object(object_proto)));
        let obj = unsafe { &mut *ptr };
        let inner_si = self.kernel_core.perm_interner().intern(INNER_PROP).0;
        self.set_or_create_prop_value(obj, inner_si, sync_iter);
        let next_si = self.kernel_core.perm_interner().intern("next").0;
        self.set_or_create_prop_value(obj, next_si, next_fn);
        let return_si = self.kernel_core.perm_interner().intern("return").0;
        self.set_or_create_prop_value(obj, return_si, return_fn);
        let throw_si = self.kernel_core.perm_interner().intern("throw").0;
        self.set_or_create_prop_value(obj, throw_si, throw_fn);
        let aiter_key = make_well_known_symbol_key(ASYNC_ITERATOR_SYMBOL_ID);
        self.set_or_create_prop_value(obj, aiter_key, self_fn);
        JsValue::from_js_object(ptr)
    }
}

/// 构造包装器方法 native 函数对象（name/length 属性由 `add_fn_name_length` 补全）。
fn make_wrapper_fn(vm: &mut Vm, native_fn: *const ()) -> JsValue {
    let fn_proto = vm.session.builtin_world().fn_proto_val();
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
    func.set_function(true);
    // SAFETY: native_fn 来自 NativeFn 函数项。
    func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(native_fn) }));
    func.set_native_arg_count(1);
    let ptr = vm.alloc_object(func);
    let obj = unsafe { &mut *ptr };
    vm.add_fn_name_length(obj, "", 1);
    JsValue::from_js_object(ptr)
}

/// 读取包装器对象上的内层同步迭代器；接收者非法或内层缺失时返回 TypeError。
fn sync_inner(vm: &mut Vm, this_val: JsValue) -> Result<JsValue, JsValue> {
    if !this_val.is_object() {
        return Err(oxide_builtins::error::create_type_error(
            vm,
            "AsyncFromSyncIterator methods called on non-object",
        ));
    }
    let obj = unsafe { &*this_val.as_js_object_ptr() };
    let inner_si = vm.kernel_core.perm_interner().intern(INNER_PROP).0;
    match vm.ordinary_get(obj, inner_si, this_val) {
        Ok(inner) if !inner.is_undefined() => Ok(inner),
        _ => Err(oxide_builtins::error::create_type_error(vm, "AsyncFromSyncIterator has no sync iterator")),
    }
}

/// AsyncFromSyncIteratorContinuation 的 unwrap 函数闭包上存目标 resolve 的属性名。
const ASFS_RESOLVE_PROP: &str = "__oxide_asfs_resolve__";
/// unwrap 函数闭包上存 done 标志的属性名。
const ASFS_DONE_PROP: &str = "__oxide_asfs_done__";
/// reject-close 闭包上存目标 reject 的属性名。
const ASFS_REJECT_PROP: &str = "__oxide_asfs_reject__";
/// reject-close 闭包上存内层同步迭代器的属性名。
const ASFS_INNER_PROP: &str = "__oxide_asfs_inner__";

/// AsyncFromSyncIteratorContinuation：解出同步迭代器结果对象的 done/value，
/// value 经 PromiseResolve 后 await，最终以 `{value: 解开值, done}` 结算能力。
/// 任一步骤抛错（done/value getter、PromiseResolve 的 constructor getter）都拒绝；
/// `close_on_rejection` 时 valueWrapper 拒绝还会先关闭内层同步迭代器。
///
/// # 参数
/// - `inner`：内层同步迭代器（关闭目标）。
/// - `result`：内层同步方法调用结果（`Err` 为调用抛出的字符串展平文本）。
fn continue_async_from_sync(vm: &mut Vm, inner: JsValue, result: Result<JsValue, String>, close_on_rejection: bool) -> NativeResult {
    let (promise, resolve, reject) = vm.new_promise_capability();
    let reject_with = |vm: &mut Vm, exc: JsValue| {
        if close_on_rejection {
            close_sync_iterator(vm, inner);
        }
        let _ = vm.reject_promise(promise, exc);
    };
    match result {
        Ok(r) if r.is_object() => {
            let result_obj = unsafe { &*r.as_js_object_ptr() };
            let done_si = vm.kernel_core.perm_interner().intern("done").0;
            let done = match vm.ordinary_get(result_obj, done_si, r) {
                Ok(d) => oxide_runtime_api::to_boolean(d),
                Err(e) => {
                    let exc = vm
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
                    reject_with(vm, exc);
                    return NativeResult::Ok(promise);
                }
            };
            let value_si = vm.kernel_core.perm_interner().intern("value").0;
            let value = match vm.ordinary_get(result_obj, value_si, r) {
                Ok(v) => v,
                Err(e) => {
                    let exc = vm
                        .last_uncaught_value
                        .take()
                        .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
                    reject_with(vm, exc);
                    return NativeResult::Ok(promise);
                }
            };
            let value_wrapper = match vm.promise_resolve(value) {
                Ok(w) => w,
                Err(e) => {
                    reject_with(vm, e);
                    return NativeResult::Ok(promise);
                }
            };
            let fulfill = make_unwrap_fn(vm, resolve, done);
            let on_rejected = if close_on_rejection {
                make_reject_close_fn(vm, reject, inner)
            } else {
                reject
            };
            let _ = vm.perform_promise_then(value_wrapper, fulfill, on_rejected);
            NativeResult::Ok(promise)
        }
        Ok(_) => {
            let err = oxide_builtins::error::create_type_error(vm, "sync iterator result is not an object");
            reject_with(vm, err);
            NativeResult::Ok(promise)
        }
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
            reject_with(vm, exc);
            NativeResult::Ok(promise)
        }
    }
}

/// 同步关闭内层迭代器（IteratorClose 的 return 调用），忽略 return 自身抛错。
fn close_sync_iterator(vm: &mut Vm, inner: JsValue) {
    if !inner.is_object() {
        return;
    }
    let inner_obj = unsafe { &*inner.as_js_object_ptr() };
    let return_si = vm.kernel_core.perm_interner().intern("return").0;
    let return_fn = match vm.ordinary_get(inner_obj, return_si, inner) {
        Ok(f) => f,
        Err(_) => return,
    };
    if oxide_builtins::iterator::is_callable(return_fn) {
        let _ = vm.call_function_sync(return_fn, inner, &[]);
    }
}

/// 构造 valueWrapper 拒绝时关闭迭代器的闭包：携带目标 reject 与内层同步迭代器。
fn make_reject_close_fn(vm: &mut Vm, reject: JsValue, inner: JsValue) -> JsValue {
    let fn_proto = vm.session.builtin_world().fn_proto_val();
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
    func.set_function(true);
    // SAFETY: async_from_sync_reject_close 是 NativeFn 函数项。
    func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_from_sync_reject_close as *const ()) }));
    func.set_native_arg_count(1);
    let ptr = vm.alloc_object(func);
    let obj = unsafe { &mut *ptr };
    let rej_si = vm.kernel_core.perm_interner().intern(ASFS_REJECT_PROP).0;
    vm.set_or_create_prop_value(obj, rej_si, reject);
    let inner_si = vm.kernel_core.perm_interner().intern(ASFS_INNER_PROP).0;
    vm.set_or_create_prop_value(obj, inner_si, inner);
    vm.add_fn_name_length(obj, "", 1);
    JsValue::from_js_object(ptr)
}

/// reject-close 闭包：关闭内层同步迭代器后以原拒绝原因结算能力。
fn async_from_sync_reject_close(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "reject handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let rej_si = vm.kernel_core.perm_interner().intern(ASFS_REJECT_PROP).0;
    let reject = vm.resolve_property(callee_obj, rej_si).unwrap_or(JsValue::undefined());
    let inner_si = vm.kernel_core.perm_interner().intern(ASFS_INNER_PROP).0;
    let inner = vm.resolve_property(callee_obj, inner_si).unwrap_or(JsValue::undefined());
    let error = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    close_sync_iterator(vm, inner);
    let _ = vm.call_function_sync(reject, JsValue::undefined(), &[error]);
    NativeResult::Ok(JsValue::undefined())
}

/// 构造 AsyncFromSyncIteratorContinuation 的 value unwrap 闭包：携带目标 resolve 与
/// done 标志，valueWrapper settle 后把解开值包成 `{value, done}` 结算。
fn make_unwrap_fn(vm: &mut Vm, resolve: JsValue, done: bool) -> JsValue {
    let fn_proto = vm.session.builtin_world().fn_proto_val();
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
    func.set_function(true);
    // SAFETY: async_from_sync_unwrap 是 NativeFn 函数项。
    func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(async_from_sync_unwrap as *const ()) }));
    func.set_native_arg_count(1);
    let ptr = vm.alloc_object(func);
    let obj = unsafe { &mut *ptr };
    let res_si = vm.kernel_core.perm_interner().intern(ASFS_RESOLVE_PROP).0;
    vm.set_or_create_prop_value(obj, res_si, resolve);
    let done_si = vm.kernel_core.perm_interner().intern(ASFS_DONE_PROP).0;
    vm.set_or_create_prop_value(obj, done_si, JsValue::bool(done));
    vm.add_fn_name_length(obj, "", 1);
    JsValue::from_js_object(ptr)
}

/// value unwrap 闭包：读自身 prop 的 resolve 与 done，把入参包装为迭代器结果结算。
fn async_from_sync_unwrap(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "unwrap handler is invalid"));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let res_si = vm.kernel_core.perm_interner().intern(ASFS_RESOLVE_PROP).0;
    let resolve = vm.resolve_property(callee_obj, res_si).unwrap_or(JsValue::undefined());
    let done_si = vm.kernel_core.perm_interner().intern(ASFS_DONE_PROP).0;
    let done = vm
        .resolve_property(callee_obj, done_si)
        .map_or(false, oxide_runtime_api::to_boolean);
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let result = make_iter_result(vm, value, done);
    let _ = vm.call_function_sync(resolve, JsValue::undefined(), &[result]);
    NativeResult::Ok(JsValue::undefined())
}

/// `AsyncFromSyncIterator.next(value)`：转发内层 `next`，结果经 continuation 结算。
/// 未传实参时不向 `next` 传参（与 IteratorNext 的空实参调用一致）。
fn async_from_sync_next(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let inner = match sync_inner(vm, this_val) {
        Ok(i) => i,
        Err(e) => return NativeResult::Err(e),
    };
    let next_si = vm.kernel_core.perm_interner().intern("next").0;
    let next_fn = match vm.ordinary_get(unsafe { &*inner.as_js_object_ptr() }, next_si, inner) {
        Ok(f) => f,
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    let result = if args.len() > 1 {
        vm.call_function_sync(next_fn, inner, &[vm.reg(args[1])])
    } else {
        vm.call_function_sync(next_fn, inner, &[])
    };
    continue_async_from_sync(vm, inner, result, true)
}

/// `AsyncFromSyncIterator.return(value)`：内层无 `return` 时直接以 `{value, done:true}`
/// 完成，否则转发并经 continuation 结算。
fn async_from_sync_return(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let inner = match sync_inner(vm, this_val) {
        Ok(i) => i,
        Err(e) => return NativeResult::Err(e),
    };
    let return_si = vm.kernel_core.perm_interner().intern("return").0;
    let return_fn = match vm.ordinary_get(unsafe { &*inner.as_js_object_ptr() }, return_si, inner) {
        Ok(f) => f,
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    if !oxide_builtins::iterator::is_callable(return_fn) {
        let result = make_iter_result(vm, arg, true);
        let (promise, _, _) = vm.new_promise_capability();
        let _ = vm.resolve_promise(promise, result);
        return NativeResult::Ok(promise);
    }
    let result = if args.len() > 1 {
        vm.call_function_sync(return_fn, inner, &[arg])
    } else {
        vm.call_function_sync(return_fn, inner, &[])
    };
    continue_async_from_sync(vm, inner, result, false)
}

/// `AsyncFromSyncIterator.throw(value)`：内层无 `throw` 时拒绝，否则转发并经 continuation 结算。
fn async_from_sync_throw(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let inner = match sync_inner(vm, this_val) {
        Ok(i) => i,
        Err(e) => return NativeResult::Err(e),
    };
    let throw_si = vm.kernel_core.perm_interner().intern("throw").0;
    let throw_fn = match vm.ordinary_get(unsafe { &*inner.as_js_object_ptr() }, throw_si, inner) {
        Ok(f) => f,
        Err(e) => {
            let exc = vm
                .last_uncaught_value
                .take()
                .unwrap_or_else(|| oxide_builtins::error::create_error(vm, &e));
            return NativeResult::Err(exc);
        }
    };
    if !oxide_builtins::iterator::is_callable(throw_fn) {
        let (promise, _, _reject) = vm.new_promise_capability();
        let err = oxide_builtins::error::create_type_error(vm, "sync iterator has no throw method");
        let _ = vm.reject_promise(promise, err);
        return NativeResult::Ok(promise);
    }
    let result = if args.len() > 1 {
        vm.call_function_sync(throw_fn, inner, &[arg])
    } else {
        vm.call_function_sync(throw_fn, inner, &[])
    };
    continue_async_from_sync(vm, inner, result, false)
}

/// `AsyncFromSyncIterator[@@asyncIterator]`：返回自身。
fn async_from_sync_symbol_async_iterator(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    NativeResult::Ok(this_val)
}
