//! AsyncDisposableStack.prototype.disposeAsync 的 VM 层实现。
//!
//! disposeAsync 返回 `%Promise%` 能力，逆序释放栈内条目。循环体以原生函数
//! 闭包实现（续链闭包），跨 await 的状态（游标/错误合并/needsAwait/hasAwaited）
//! 存函数对象属性槽；栈对象引用存入闭包槽保活状态盒（跨 await 不被 GC 回收）。
//! 2026 新版 DisposeResources 语义：null/undefined 型 async 条目的 Await 延迟到
//! 循环末单次执行；sync 条目遇待决 Await 先插队执行 Await；`@@dispose` 来源的
//! 方法（wrap_sync）返回值丢弃、同步异常异步化为拒绝再 await。
//!
//! 状态机仿 AsyncFromSync 续链模式：每次 await 经 `promise_resolve` +
//! `perform_promise_then` 注册同一 native（不同 reject_role 槽）的两个闭包实例，
//! 派生的中间 promise 不可观察。

use oxide_builtins::disposable_stack::{merge_dispose_error, require_dispose_capability, DisposeState};
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::NativeResult;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use crate::vm::Vm;

/// 续链闭包上保存状态盒所在栈对象的属性名（保活状态盒）。
const STACK_PROP: &str = "__oxide_ads_stack__";
/// 续链闭包上保存能力 resolve 的属性名。
const RESOLVE_PROP: &str = "__oxide_ads_resolve__";
/// 续链闭包上保存能力 reject 的属性名。
const REJECT_PROP: &str = "__oxide_ads_reject__";
/// 续链闭包上保存下一 entry 下标的属性名（逆序遍历 0→len）。
const INDEX_PROP: &str = "__oxide_ads_index__";
/// 续链闭包上保存已合并错误值的属性名（无错时为 undefined）。
const COMPLETION_PROP: &str = "__oxide_ads_completion__";
/// 续链闭包上保存是否已有错误值的属性名（区分"无错"与"错误值为 undefined"）。
const HAS_COMPLETION_PROP: &str = "__oxide_ads_has_completion__";
/// 续链闭包上保存 needsAwait 标志的属性名（末尾补 Await / sync 插队判断）。
const NEEDS_AWAIT_PROP: &str = "__oxide_ads_needs_await__";
/// 续链闭包上保存 hasAwaited 标志的属性名（真实 async 方法是否已 await）。
const HAS_AWAITED_PROP: &str = "__oxide_ads_has_awaited__";
/// 续链闭包上区分 reject 角色的属性名（reject 处理器先合并入参为错误）。
const REJECT_ROLE_PROP: &str = "__oxide_ads_reject_role__";

/// `AsyncDisposableStack.prototype.disposeAsync()`：逆序释放全部资源并返回能力。
///
/// # 步骤
/// 1. 新建 promise 能力；this 非 AsyncDisposableStack（type_tag=25）或无状态盒 →
///    reject 该能力（**reject 而非抛**）并返回。
/// 2. 已 Disposed → 同步 fulfill undefined（幂等，当前轮即入队反应）。
/// 3. 同步置 state=Disposed（防重入；执行中的二次调用命中步骤 2）。
/// 4. 建续链闭包并同步执行循环至首次 await 或完成：完成时 fulfill/reject 能力
///    同步结算（空栈/纯 sync 满足微任务序断言）；首次 await 注册续链后返回。
pub(crate) fn dispose_async(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let (promise, resolve, reject) = vm.new_promise_capability();
    let cap = match require_dispose_capability(vm, this_val, JsObject::OBJ_TYPE_ASYNC_DISPOSABLE_STACK) {
        Ok(c) => c,
        Err(err) => {
            let _ = vm.call_function_sync(reject, JsValue::undefined(), &[err]);
            return NativeResult::Ok(promise);
        }
    };
    // SAFETY: cap 由 require 校验，native 执行期间对象存活。
    let state = unsafe { (*cap).state };
    if state == DisposeState::Disposed {
        let _ = vm.call_function_sync(resolve, JsValue::undefined(), &[JsValue::undefined()]);
        return NativeResult::Ok(promise);
    }
    // 先置位再执行：执行中/后续调用 disposeAsync 均命中 Disposed 分支。
    unsafe { (*cap).state = DisposeState::Disposed };
    let entries_len = unsafe { (*cap).entries.len() };
    let continue_fn = make_dispose_continue_fn(vm, this_val, resolve, reject, false, None);
    // 逆序游标初始化为条目数：INDEX 槽表示下一待处理位置（len → 0 递减）。
    let continue_obj = unsafe { &mut *continue_fn.as_js_object_ptr() };
    write_prop(vm, continue_obj, INDEX_PROP, JsValue::int(entries_len as i32));
    run_dispose_loop(vm, &continue_fn, JsValue::undefined(), false);
    NativeResult::Ok(promise)
}

/// 构造 disposeAsync 续链闭包：native 函数对象，状态存属性槽。
///
/// # 步骤
/// 1. 以 `dispose_async_continue` 建原生函数对象（length=1）。
/// 2. 写栈对象/能力闭包/角色槽；`state_from` 给出当前循环状态（游标/错误合并/
///    await 标志）时逐槽复制，保证续链从上次让出点继续。
fn make_dispose_continue_fn(
    vm: &mut Vm, stack: JsValue, resolve: JsValue, reject: JsValue, reject_role: bool, state_from: Option<&JsObject>,
) -> JsValue {
    let fn_proto = vm.session.builtin_world().fn_proto_val();
    let mut func = JsObject::new_empty(EMPTY_SHAPE_ID, fn_proto);
    func.set_function(true);
    // SAFETY: dispose_async_continue 是 NativeFn 函数项。
    func.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(dispose_async_continue as *const ()) }));
    func.set_native_arg_count(1);
    let ptr = vm.alloc_object(func);
    let obj = unsafe { &mut *ptr };
    let stack_si = vm.kernel_core.perm_interner().intern(STACK_PROP).0;
    vm.set_or_create_prop_value(obj, stack_si, stack);
    let resolve_si = vm.kernel_core.perm_interner().intern(RESOLVE_PROP).0;
    vm.set_or_create_prop_value(obj, resolve_si, resolve);
    let reject_si = vm.kernel_core.perm_interner().intern(REJECT_PROP).0;
    vm.set_or_create_prop_value(obj, reject_si, reject);
    if let Some(src) = state_from {
        let index_si = vm.kernel_core.perm_interner().intern(INDEX_PROP).0;
        if let Some(v) = vm.resolve_property(src, index_si) {
            vm.set_or_create_prop_value(obj, index_si, v);
        }
        let completion_si = vm.kernel_core.perm_interner().intern(COMPLETION_PROP).0;
        if let Some(v) = vm.resolve_property(src, completion_si) {
            vm.set_or_create_prop_value(obj, completion_si, v);
        }
        let has_completion_si = vm.kernel_core.perm_interner().intern(HAS_COMPLETION_PROP).0;
        if let Some(v) = vm.resolve_property(src, has_completion_si) {
            vm.set_or_create_prop_value(obj, has_completion_si, v);
        }
        let needs_await_si = vm.kernel_core.perm_interner().intern(NEEDS_AWAIT_PROP).0;
        if let Some(v) = vm.resolve_property(src, needs_await_si) {
            vm.set_or_create_prop_value(obj, needs_await_si, v);
        }
        let has_awaited_si = vm.kernel_core.perm_interner().intern(HAS_AWAITED_PROP).0;
        if let Some(v) = vm.resolve_property(src, has_awaited_si) {
            vm.set_or_create_prop_value(obj, has_awaited_si, v);
        }
    } else {
        let index_si = vm.kernel_core.perm_interner().intern(INDEX_PROP).0;
        vm.set_or_create_prop_value(obj, index_si, JsValue::int(0));
        let completion_si = vm.kernel_core.perm_interner().intern(COMPLETION_PROP).0;
        vm.set_or_create_prop_value(obj, completion_si, JsValue::undefined());
        let has_completion_si = vm.kernel_core.perm_interner().intern(HAS_COMPLETION_PROP).0;
        vm.set_or_create_prop_value(obj, has_completion_si, JsValue::bool(false));
        let needs_await_si = vm.kernel_core.perm_interner().intern(NEEDS_AWAIT_PROP).0;
        vm.set_or_create_prop_value(obj, needs_await_si, JsValue::bool(false));
        let has_awaited_si = vm.kernel_core.perm_interner().intern(HAS_AWAITED_PROP).0;
        vm.set_or_create_prop_value(obj, has_awaited_si, JsValue::bool(false));
    }
    let role_si = vm.kernel_core.perm_interner().intern(REJECT_ROLE_PROP).0;
    vm.set_or_create_prop_value(obj, role_si, JsValue::bool(reject_role));
    vm.add_fn_name_length(obj, "", 1);
    JsValue::from_js_object(ptr)
}

/// 续链闭包回调：读角色槽区分 fulfill/reject，把 await 结果交给循环主体。
fn dispose_async_continue(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return NativeResult::Err(oxide_builtins::error::create_type_error(
            vm,
            "disposeAsync continuation handler is invalid",
        ));
    }
    let callee_obj = unsafe { &*callee.as_js_object_ptr() };
    let role_si = vm.kernel_core.perm_interner().intern(REJECT_ROLE_PROP).0;
    let reject_role = vm
        .resolve_property(callee_obj, role_si)
        .map(|v| v.is_bool() && v.as_bool())
        .unwrap_or(false);
    let arg = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    run_dispose_loop(vm, &callee, arg, reject_role);
    NativeResult::Ok(JsValue::undefined())
}

/// disposeAsync 循环主体：逆序遍历 entries，按 2026 DisposeResources 语义释放。
///
/// # 步骤
/// 1. 读全部状态槽；reject 角色先把 await 拒绝原因合并进完成记录。
/// 2. 逐条逆序处理：
///    - 游标越界：needsAwait 且未 await 过 → 末尾补一次 Await(undefined)；
///      否则清空 entries 并结算能力（有错 reject / 无错 fulfill）。
///    - sync 条目遇待决 needsAwait → 先插队 Await(undefined) 再执行。
///    - method 为 undefined（async-null 条目）→ 置 needsAwait，继续。
///    - 调用释放回调：sync 抛错 → 合并后继续；async 条目调用成功 → 记
///      hasAwaited 并 await（wrap_sync 丢弃返回值 await undefined）；wrap_sync
///      同步抛错 → 建 rejected promise 异步化后 await；真实 async 方法同步抛错
///      → 直接合并（规范不异步化）。
/// 3. 每次 await 前更新状态槽，注册同一 native 的 fulfill/reject 双闭包续链。
fn run_dispose_loop(vm: &mut Vm, callee: &JsValue, arg: JsValue, reject_role: bool) {
    if !callee.is_object() {
        return;
    }
    // SAFETY: callee 是当前 native 调用的函数对象，native 执行期间存活。
    let callee_obj = unsafe { &mut *callee.as_js_object_ptr() };
    let stack = read_prop(vm, callee_obj, STACK_PROP);
    let resolve = read_prop(vm, callee_obj, RESOLVE_PROP);
    let reject = read_prop(vm, callee_obj, REJECT_PROP);
    if !stack.is_object() || resolve.is_undefined() || reject.is_undefined() {
        return;
    }
    let mut index = read_int(vm, callee_obj, INDEX_PROP);
    let mut completion = read_prop(vm, callee_obj, COMPLETION_PROP);
    let mut has_completion = read_bool(vm, callee_obj, HAS_COMPLETION_PROP);
    let mut needs_await = read_bool(vm, callee_obj, NEEDS_AWAIT_PROP);
    let mut has_awaited = read_bool(vm, callee_obj, HAS_AWAITED_PROP);

    if reject_role {
        let merged = merge_dispose_error(vm, if has_completion { Some(completion) } else { None }, arg);
        (completion, has_completion) = match merged {
            Some(err) => (err, true),
            None => (JsValue::undefined(), false),
        };
        write_prop(vm, callee_obj, HAS_COMPLETION_PROP, JsValue::bool(has_completion));
        write_prop(vm, callee_obj, COMPLETION_PROP, completion);
    }

    loop {
        let stack_ptr = stack.as_js_object_ptr();
        if stack_ptr.is_null() {
            return;
        }
        // SAFETY: stack 对象由闭包槽保活，native 执行期间 epoch 不重置。
        let stack_obj = unsafe { &*stack_ptr };
        if !stack_obj.is_async_disposable_stack_obj() {
            return;
        }
        let cap = oxide_builtins::disposable_stack::get_capability_ptr(stack_obj);
        if cap.is_null() {
            return;
        }
        // SAFETY: cap 是构造时写入的有效 Box 指针，状态盒随栈对象存活。
        let cap_ref = unsafe { &mut *cap };
        if index == 0 {
            if needs_await && !has_awaited {
                needs_await = false;
                write_prop(vm, callee_obj, NEEDS_AWAIT_PROP, JsValue::bool(needs_await));
                try_await(vm, callee_obj, JsValue::undefined());
                return;
            }
            cap_ref.entries.clear();
            let _ = if has_completion {
                vm.call_function_sync(reject, JsValue::undefined(), &[completion])
            } else {
                vm.call_function_sync(resolve, JsValue::undefined(), &[JsValue::undefined()])
            };
            return;
        }
        let entry = cap_ref.entries[index - 1];
        if entry.hint == 0 && needs_await && !has_awaited {
            // 插队 await：不推进游标，await 续链后重读同一位置执行该 sync 条目。
            needs_await = false;
            write_prop(vm, callee_obj, NEEDS_AWAIT_PROP, JsValue::bool(needs_await));
            try_await(vm, callee_obj, JsValue::undefined());
            return;
        }
        index -= 1;
        write_prop(vm, callee_obj, INDEX_PROP, JsValue::int(index as i32));
        if entry.method.is_undefined() {
            needs_await = true;
            write_prop(vm, callee_obj, NEEDS_AWAIT_PROP, JsValue::bool(needs_await));
            continue;
        }
        match oxide_builtins::disposable_stack::call_entry_method(vm, &entry) {
            Err(err) => {
                if entry.hint == 1 && entry.wrap_sync {
                    has_awaited = true;
                    write_prop(vm, callee_obj, HAS_AWAITED_PROP, JsValue::bool(has_awaited));
                    let (rejected, _, _) = vm.new_promise_capability();
                    let _ = vm.reject_promise(rejected, err);
                    try_await(vm, callee_obj, rejected);
                    return;
                }
                let merged = merge_dispose_error(vm, if has_completion { Some(completion) } else { None }, err);
                (completion, has_completion) = match merged {
                    Some(err_val) => (err_val, true),
                    None => (JsValue::undefined(), false),
                };
                write_prop(vm, callee_obj, HAS_COMPLETION_PROP, JsValue::bool(has_completion));
                write_prop(vm, callee_obj, COMPLETION_PROP, completion);
            }
            Ok(value) => {
                if entry.hint == 1 {
                    has_awaited = true;
                    write_prop(vm, callee_obj, HAS_AWAITED_PROP, JsValue::bool(has_awaited));
                    let awaited = if entry.wrap_sync { JsValue::undefined() } else { value };
                    try_await(vm, callee_obj, awaited);
                    return;
                }
            }
        }
    }
}

/// 读闭包对象属性槽的 bool 值（缺失回退 false）。
fn read_bool(vm: &Vm, obj: &JsObject, prop: &str) -> bool {
    let v = read_prop(vm, obj, prop);
    v.is_bool() && v.as_bool()
}

/// 读闭包对象属性槽的 int 值（缺失回退 0）。
fn read_int(vm: &Vm, obj: &JsObject, prop: &str) -> usize {
    let v = read_prop(vm, obj, prop);
    if v.is_int() && v.as_int() >= 0 {
        v.as_int() as usize
    } else {
        0
    }
}

/// 读闭包对象属性槽值（缺失回退 undefined）。
fn read_prop(vm: &Vm, obj: &JsObject, prop: &str) -> JsValue {
    let si = vm.kernel_core.perm_interner().intern(prop).0;
    vm.resolve_property(obj, si).unwrap_or(JsValue::undefined())
}

/// 写闭包对象属性槽值。
fn write_prop(vm: &mut Vm, obj: &mut JsObject, prop: &str, val: JsValue) {
    let si = vm.kernel_core.perm_interner().intern(prop).0;
    vm.set_or_create_prop_value(obj, si, val);
}

/// 等待 `value`：PromiseResolve 包装后注册续链；失败（constructor getter 抛错）
/// 直接拒绝能力（规范 Await abrupt → IfAbruptRejectPromise）。
///
/// # 副作用
/// - 注册的 fulfill/reject 续链闭包持全套状态槽；派生 promise 不可观察。
fn try_await(vm: &mut Vm, callee_obj: &JsObject, value: JsValue) {
    let promise = match vm.promise_resolve(value) {
        Ok(p) => p,
        Err(exc) => {
            let reject = read_prop(vm, callee_obj, REJECT_PROP);
            let _ = vm.call_function_sync(reject, JsValue::undefined(), &[exc]);
            return;
        }
    };
    let stack = read_prop(vm, callee_obj, STACK_PROP);
    let resolve = read_prop(vm, callee_obj, RESOLVE_PROP);
    let reject = read_prop(vm, callee_obj, REJECT_PROP);
    let fulfill_fn = make_dispose_continue_fn(vm, stack, resolve, reject, false, Some(callee_obj));
    let reject_fn = make_dispose_continue_fn(vm, stack, resolve, reject, true, Some(callee_obj));
    let _ = vm.perform_promise_then(promise, fulfill_fn, reject_fn);
}
