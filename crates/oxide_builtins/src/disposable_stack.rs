//! DisposableStack（同步资源栈）内置对象实现。
//!
//! 状态盒 `DisposeCapability` 以 `Box::into_raw` 存入对象 `native_data`：`state`
//! 记录栈状态，`entries` 按入栈序存待释放资源。同步栈 dispose 只用
//! Pending/Disposed 两态；Disposing 为异步资源栈 disposeAsync 预留。GC 四函数
//! （edges/rewrite/clone/drop）供 session 层跨 epoch 追踪，签名与 Map/Promise
//! 状态盒同构。

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, VmHost};

/// 分配空状态盒并存入新 DisposableStack 对象（proto 固定为 world 字段）。
fn alloc_disposable_stack<H: VmHost>(vm: &mut H, proto_val: JsValue) -> *mut JsObject {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
    obj.type_tag = JsObject::OBJ_TYPE_DISPOSABLE_STACK;
    obj.set_native_data(Box::into_raw(Box::new(DisposeCapability {
        state: DisposeState::Pending,
        entries: Vec::new(),
    })) as *mut u8);
    vm.alloc_object(obj)
}

macro_rules! native_try {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => return NativeResult::Err(err),
        }
    };
}

/// 取出栈对象 native-data 槽中的 `DisposeCapability` 可变指针。
///
/// # 边界与前提
/// - this 非对象 → TypeError；
/// - type_tag 非 24/25 → TypeError（无槽对象）；
/// - native_data 为空 → TypeError（防御，正常构造路径不会出现）。
fn require_dispose_capability<H: VmHost>(vm: &mut H, this_val: JsValue) -> Result<*mut DisposeCapability, JsValue> {
    if !this_val.is_object() {
        return Err(crate::error::create_type_error(vm, "called on non-DisposableStack object"));
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "DisposableStack internal state invalid"));
    }
    // SAFETY: obj_ptr 是当前 epoch 分配的 JsObject 非空指针；native 执行期间 epoch 不重置。
    let obj = unsafe { &*obj_ptr };
    if !obj.is_disposable_stack_obj() && !obj.is_async_disposable_stack_obj() {
        return Err(crate::error::create_type_error(
            vm,
            "DisposableStack.prototype method called on incompatible receiver",
        ));
    }
    let cap_ptr = get_capability_ptr(obj);
    if cap_ptr.is_null() {
        return Err(crate::error::create_type_error(vm, "DisposableStack internal state invalid"));
    }
    Ok(cap_ptr)
}

fn disposed_reference_error<H: VmHost>(vm: &mut H) -> JsValue {
    crate::error::create_reference_error(vm, "DisposableStack has been disposed")
}

/// `DisposableStack` 构造函数：创建带空状态盒的 DisposableStack 对象。
///
/// # 步骤
/// 1. 校验 NewTarget：`this` 的原型链须命中 DisposableStack.prototype
///    （`new DisposableStack()` 直接命中，子类 `super()` 经子类原型链命中；
///    普通调用抛 TypeError）。
/// 2. 建空状态盒对象，proto 固定为 world.disposable_stack_proto。
pub fn disposable_stack_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let is_new_call = this_val.is_object() && {
        let stack_proto = vm.session().builtin_world().disposable_stack_proto.as_ptr() as *mut JsObject;
        let this_ptr = this_val.as_js_object_ptr();
        if this_ptr.is_null() {
            false
        } else {
            let mut proto = unsafe { &*this_ptr }.proto();
            let mut found = false;
            for _ in 0..16 {
                if !proto.is_object() {
                    break;
                }
                let proto_ptr = proto.as_js_object_ptr();
                if proto_ptr.is_null() {
                    break;
                }
                if std::ptr::eq(proto_ptr, stack_proto) {
                    found = true;
                    break;
                }
                proto = unsafe { &*proto_ptr }.proto();
            }
            found
        }
    };
    if !is_new_call {
        return NativeResult::Err(crate::error::create_type_error(vm, "DisposableStack must be called with new"));
    }
    let proto_val =
        JsValue::from_js_object(vm.session().builtin_world().disposable_stack_proto.as_ptr() as *mut JsObject);
    let stack = alloc_disposable_stack(vm, proto_val);
    NativeResult::Ok(JsValue::from_js_object(stack))
}

/// `DisposableStack.prototype.use(value)`：把资源的 `value[@@dispose]` 方法入栈。
///
/// # 步骤
/// 1. 校验栈对象与状态；Disposed 抛 ReferenceError。
/// 2. value 为 null/undefined → 直接返回 value 不入栈。
/// 3. value 非对象 → TypeError。
/// 4. 读 `value[@@dispose]`（只读一次，getter 抛错透传原值）；缺/null/undefined/
///    非 callable → TypeError。
/// 5. 入栈 `{value, method, sync, receiver 模式}`，返回 value。
pub fn disposable_stack_use<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let cap = native_try!(require_dispose_capability(vm, this_val));
    // SAFETY: cap 由 require 校验，native 执行期间对象存活。
    let cap_ref = unsafe { &mut *cap };
    if cap_ref.state != DisposeState::Pending {
        return NativeResult::Err(disposed_reference_error(vm));
    }
    let value = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    if value.is_null() || value.is_undefined() {
        return NativeResult::Ok(value);
    }
    if !value.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "value is not an Object"));
    }
    let value_obj = unsafe { &*value.as_js_object_ptr() };
    let dispose_key = oxide_types::private_key::make_well_known_symbol_key(12);
    let method = match vm.ordinary_get(value_obj, dispose_key, value) {
        Ok(m) => m,
        Err(err) => return NativeResult::Err(crate::iterator::engine_error(vm, &err)),
    };
    if method.is_null() || method.is_undefined() || !crate::iterator::is_callable(method) {
        return NativeResult::Err(crate::error::create_type_error(vm, "value[Symbol.dispose] is not callable"));
    }
    cap_ref.entries.push(DisposeEntry {
        value,
        method,
        hint: 0,
        arg_style: false,
    });
    NativeResult::Ok(value)
}

/// `DisposableStack.prototype.adopt(value, onDispose)`：把 value 与其释放回调入栈。
///
/// # 步骤
/// 1. 校验栈对象与状态；Disposed 抛 ReferenceError。
/// 2. onDispose 非 callable → TypeError。
/// 3. 入栈 `{value, onDispose, sync, arg-style}`（dispose 时 `Call(onDispose, undefined, «value»)`），
///    返回 value。
pub fn disposable_stack_adopt<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let cap = native_try!(require_dispose_capability(vm, this_val));
    // SAFETY: cap 由 require 校验，native 执行期间对象存活。
    let cap_ref = unsafe { &mut *cap };
    if cap_ref.state != DisposeState::Pending {
        return NativeResult::Err(disposed_reference_error(vm));
    }
    let value = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    let on_dispose = vm.reg(if args.len() > 2 { args[2] } else { 0 });
    if !crate::iterator::is_callable(on_dispose) {
        return NativeResult::Err(crate::error::create_type_error(vm, "onDispose is not callable"));
    }
    cap_ref.entries.push(DisposeEntry {
        value,
        method: on_dispose,
        hint: 0,
        arg_style: true,
    });
    NativeResult::Ok(value)
}

/// `DisposableStack.prototype.defer(onDispose)`：把无参数释放回调入栈。
///
/// # 步骤
/// 1. 校验栈对象与状态；Disposed 抛 ReferenceError。
/// 2. onDispose 非 callable → TypeError。
/// 3. 入栈 `{undefined, onDispose, sync, receiver 模式}`（dispose 时 `Call(onDispose, undefined, «»)`），
///    返回 undefined。
pub fn disposable_stack_defer<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let cap = native_try!(require_dispose_capability(vm, this_val));
    // SAFETY: cap 由 require 校验，native 执行期间对象存活。
    let cap_ref = unsafe { &mut *cap };
    if cap_ref.state != DisposeState::Pending {
        return NativeResult::Err(disposed_reference_error(vm));
    }
    let on_dispose = vm.reg(if args.len() > 1 { args[1] } else { 0 });
    if !crate::iterator::is_callable(on_dispose) {
        return NativeResult::Err(crate::error::create_type_error(vm, "onDispose is not callable"));
    }
    cap_ref.entries.push(DisposeEntry {
        value: JsValue::undefined(),
        method: on_dispose,
        hint: 0,
        arg_style: false,
    });
    NativeResult::Ok(JsValue::undefined())
}

/// `DisposableStack.prototype.dispose()`：逆序执行全部释放回调并置栈为 Disposed。
///
/// # 步骤
/// 1. 校验栈对象；state 非 Pending → 直接返回 undefined（幂等，Disposing/Disposed 同返）。
/// 2. 先置 state=Disposed 再执行（防执行中 use/adopt 重入入栈）。
/// 3. 逆序逐条调用：arg-style 走 `Call(method, undefined, «value»)`，否则
///    `Call(method, value, «»)`；method 为 undefined 时跳过（防御）。
/// 4. 单错原样抛；多错用 SuppressedError 链式合并（error=后抛、suppressed=前值），
///    循环不因错误中断，剩余条目继续执行。
///
/// # 边界与前提
/// - 回调抛原始值（含 primitive）经 take_uncaught_value 恢复并存入 SuppressedError；
/// - 无 uncaught 时回退按错误文本建普通 Error。
pub fn disposable_stack_dispose<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let cap = native_try!(require_dispose_capability(vm, this_val));
    // SAFETY: cap 由 require 校验，native 执行期间对象存活。
    let cap_ref = unsafe { &mut *cap };
    if cap_ref.state != DisposeState::Pending {
        return NativeResult::Ok(JsValue::undefined());
    }
    // 先置位再执行：执行中调用 use/adopt/defer 一律命中 Disposed 分支。
    cap_ref.state = DisposeState::Disposed;
    let mut completion: Option<JsValue> = None;
    for entry in cap_ref.entries.iter().rev() {
        if entry.method.is_undefined() {
            continue;
        }
        let call_result = if entry.arg_style {
            vm.call_function_sync(entry.method, JsValue::undefined(), &[entry.value])
        } else {
            vm.call_function_sync(entry.method, entry.value, &[])
        };
        if let Err(err) = call_result {
            // 恢复本次回调抛出的原始异常值；无 uncaught 时按错误文本建普通 Error。
            let err_val = vm
                .take_uncaught_value()
                .unwrap_or_else(|| crate::error::create_kind_error(vm, "Error", &err));
            completion = Some(match completion {
                None => err_val,
                Some(prev) => crate::error::create_suppressed_error(vm, err_val, prev),
            });
        }
    }
    // 条目逐条执行完毕，清空释放引用（规范 DisposeResources 逐条移除）。
    cap_ref.entries.clear();
    match completion {
        Some(err_val) => NativeResult::Err(err_val),
        None => NativeResult::Ok(JsValue::undefined()),
    }
}

/// `DisposableStack.prototype.move()`：把全部 entries 转移到新 DisposableStack。
///
/// # 步骤
/// 1. 校验栈对象；state 非 Pending → ReferenceError。
/// 2. `mem::take` 转移 entries；建新栈（proto 固定为 DisposableStack.prototype，
///    非子类原型）并装入转移的 entries。
/// 3. 源栈置 Disposed（不再执行任何释放）。
pub fn disposable_stack_move<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let cap = native_try!(require_dispose_capability(vm, this_val));
    // SAFETY: cap 由 require 校验，native 执行期间对象存活。
    let cap_ref = unsafe { &mut *cap };
    if cap_ref.state != DisposeState::Pending {
        return NativeResult::Err(disposed_reference_error(vm));
    }
    let taken = std::mem::take(&mut cap_ref.entries);
    let proto_val =
        JsValue::from_js_object(vm.session().builtin_world().disposable_stack_proto.as_ptr() as *mut JsObject);
    let new_stack = alloc_disposable_stack(vm, proto_val);
    // SAFETY: alloc_disposable_stack 刚分配的对象，native_data 为新建空状态盒。
    unsafe {
        let new_obj = &mut *new_stack;
        let new_cap = new_obj.native_data() as *mut DisposeCapability;
        (*new_cap).entries = taken;
    }
    cap_ref.state = DisposeState::Disposed;
    NativeResult::Ok(JsValue::from_js_object(new_stack))
}

/// `get DisposableStack.prototype.disposed`：栈是否已 dispose/move。
pub fn disposable_stack_disposed_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let cap = native_try!(require_dispose_capability(vm, this_val));
    // SAFETY: cap 由 require 校验，native 执行期间对象存活。
    let state = unsafe { (*cap).state };
    NativeResult::Ok(JsValue::bool(state != DisposeState::Pending))
}

/// 栈生命周期状态：Pending 可入栈，Disposed 后所有操作抛 ReferenceError；
/// Disposing 为异步资源栈 disposeAsync 执行期标记（当前未构造，语义预留）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DisposeState {
    Pending,
    #[allow(dead_code)]
    Disposing,
    Disposed,
}

/// 单条待释放资源：dispose 时按 `arg_style` 选择调用约定。
pub(crate) struct DisposeEntry {
    /// use() 的资源值（dispose 时作 receiver）；adopt() 的资源值（作参数）；defer() 恒 undefined。
    pub value: JsValue,
    /// 释放方法：use() 为 @@dispose 函数；adopt()/defer() 为用户回调。
    pub method: JsValue,
    /// 0=sync-dispose，1=async-dispose（异步资源栈用）。
    pub hint: u8,
    /// true=adopt 闭包语义（`Call(method, undefined, «value»)`）；
    /// false=use/defer（`Call(method, value, «»)`）。
    pub arg_style: bool,
}

/// 状态盒：栈状态 + 按入栈序的资源条目。
pub(crate) struct DisposeCapability {
    pub state: DisposeState,
    pub entries: Vec<DisposeEntry>,
}

fn get_capability_ptr(obj: &JsObject) -> *mut DisposeCapability {
    obj.native_data() as *mut DisposeCapability
}

/// 收集栈内作为对象引用的 value/method（GC 根边），供跨 epoch 遍历/重写时追踪。
pub fn dispose_edges(obj: &JsObject) -> Vec<JsValue> {
    if !obj.is_disposable_stack_obj() && !obj.is_async_disposable_stack_obj() {
        return Vec::new();
    }
    let ptr = get_capability_ptr(obj);
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: native_data 持有 `alloc_capability` 写入的有效 Box 指针，本函数
    // 只在对象存活期间被 GC/绑定层调用。
    unsafe {
        (*ptr)
            .entries
            .iter()
            .flat_map(|entry| [entry.value, entry.method])
            .filter(|value: &JsValue| value.is_object())
            .collect()
    }
}

/// 克隆状态盒到新对象，用 `rewrite` 改写其中的对象引用
/// （供跨 epoch 的对象重写/克隆流程使用）。
pub fn clone_dispose_native_with_rewrite<F>(src: &JsObject, dst: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !src.is_disposable_stack_obj() && !src.is_async_disposable_stack_obj() {
        return;
    }
    let ptr = get_capability_ptr(src);
    if ptr.is_null() {
        dst.set_native_data(std::ptr::null_mut());
        return;
    }
    // SAFETY: 同上，src 状态盒在 GC 移动期间仍存活。
    let src_cap = unsafe { &*ptr };
    let cloned = DisposeCapability {
        state: src_cap.state,
        entries: src_cap
            .entries
            .iter()
            .map(|entry| DisposeEntry {
                value: if entry.value.is_object() { rewrite(entry.value) } else { entry.value },
                method: if entry.method.is_object() { rewrite(entry.method) } else { entry.method },
                hint: entry.hint,
                arg_style: entry.arg_style,
            })
            .collect(),
    };
    dst.set_native_data(Box::into_raw(Box::new(cloned)) as *mut u8);
}

/// 原地重写状态盒，用 `rewrite` 改写其中的对象引用。
pub fn rewrite_dispose_native<F>(obj: &mut JsObject, mut rewrite: F)
where
    F: FnMut(JsValue) -> JsValue,
{
    if !obj.is_disposable_stack_obj() && !obj.is_async_disposable_stack_obj() {
        return;
    }
    let ptr = get_capability_ptr(obj);
    if ptr.is_null() {
        return;
    }
    // SAFETY: 同上，改写发生在 GC 移动/清扫期间，对象仍存活。
    unsafe {
        for entry in &mut (*ptr).entries {
            entry.value = rewrite(entry.value);
            entry.method = rewrite(entry.method);
        }
    }
}

/// 释放状态盒（对象被回收时），返回释放的字节数供泄漏统计。
pub fn drop_dispose_native(obj: &mut JsObject) -> u64 {
    if !obj.is_disposable_stack_obj() && !obj.is_async_disposable_stack_obj() {
        return 0;
    }
    let ptr = get_capability_ptr(obj);
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: 指针来自 Box::into_raw，只在对象被回收时释放一次。
    unsafe {
        let cap = Box::from_raw(ptr);
        let bytes =
            std::mem::size_of::<DisposeCapability>() + cap.entries.capacity() * std::mem::size_of::<DisposeEntry>();
        drop(cap);
        obj.set_native_data(std::ptr::null_mut());
        bytes as u64
    }
}
