//! Worker API 的 concrete `&mut Vm` native 函数：构造器、`postMessage`/`terminate`
//! 原型方法、`self` 面（`self.postMessage`/`self.close`/`self.name`/`self.location`）
//! 与 `MessageEvent` / 错误事件构造辅助。
//!
//! 关键约定：
//! - 函数是 concrete `&mut Vm` 形态（`NativeFn`），不挂 `VmHost` trait（trait 是单
//!   realm 接口，worker 方法跨线程不挂 trait，见 934.9 边界条款）。
//! - Worker 构造器走 Box 分配（不占 `BuiltinWorld` P 字段、不新增 `BuiltinId`、
//!   不占 `BuiltinDirtySet` 脏位），由 `bind_worker` 安装。
//! - `self_*` 方法绑 worker realm 的 global（`self` === global 故经 `self` 可达）；
//!   主 realm 无 `self`，方法作为全局函数存在（无害占位）。
//! - worker → 主线程输出通道与自关停请求位经 thread-local 传递（worker 线程独占，
//!   不跨线程共享），`worker_event_loop` 建 Vm 后注入。

use std::cell::{Cell, RefCell};

use oxide_builtins::message_value::{detach_message, parse_transfer_list};
use oxide_kernel::message_queue::Sender;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{ListenerEntry, NativeResult, ProtoKind, VmHost};
use oxide_types::object::{JsObject, PropAttributes};
use oxide_types::value::JsValue;

use crate::vm::Vm;

use super::WorkerOutMail;

// worker → 主线程输出通道（worker realm 专用，`worker_event_loop` 建 Vm 后注入）。
// thread-local：worker 线程独占，不跨线程共享。主 realm 的 Vm 不注入（恒 `None`）。
thread_local! {
    static WORKER_OUT_TX: RefCell<Option<Sender<WorkerOutMail>>> = const { RefCell::new(None) };
}

// worker 自关停请求位（`self.close()` 置位，事件循环每轮检查）。
thread_local! {
    static WORKER_CLOSE_REQUESTED: Cell<bool> = const { Cell::new(false) };
}

// worker 名称（构造器 options.name，worker 线程独占，主 realm 不置位）。
thread_local! {
    static WORKER_NAME: RefCell<Option<String>> = const { RefCell::new(None) };
}

// worker 位置（构造器 url 字符串，worker 线程独占，主 realm 不置位）。
thread_local! {
    static WORKER_LOCATION: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// 注入 worker → 主线程输出通道（`worker_event_loop` 建 Vm 后调用）。
///
/// # 副作用
/// - 覆盖本线程的 `WORKER_OUT_TX`（worker 线程内恰好一次）。
pub(crate) fn set_worker_out_tx(tx: Sender<WorkerOutMail>) {
    WORKER_OUT_TX.with(|slot| *slot.borrow_mut() = Some(tx));
}

/// 置位 worker 自关停请求（`self.close()` 调用）。
///
/// # 副作用
/// - 置位本线程的 `WORKER_CLOSE_REQUESTED`（事件循环每轮检查后退出）。
pub(crate) fn request_worker_close() {
    WORKER_CLOSE_REQUESTED.with(|flag| flag.set(true));
}

/// 查询 worker 自关停请求位（事件循环每轮检查）。
pub(crate) fn worker_close_requested() -> bool {
    WORKER_CLOSE_REQUESTED.with(|flag| flag.get())
}

/// 注入 worker 名称与位置（`worker_event_loop` 建 Vm 前调用）。
///
/// # 副作用
/// - 覆盖本线程的 `WORKER_NAME` 与 `WORKER_LOCATION`（worker 线程内恰好一次）。
/// - 建 Vm 前调用：`bind_worker` 据本线程是否置位区分 worker realm 与主 realm。
pub(crate) fn set_worker_identity(name: String, location: String) {
    WORKER_NAME.with(|slot| *slot.borrow_mut() = Some(name));
    WORKER_LOCATION.with(|slot| *slot.borrow_mut() = Some(location));
}

/// 查询当前线程是否 worker 线程（绑定层区分 worker realm 与主 realm）。
pub(crate) fn worker_thread() -> bool {
    WORKER_NAME.with(|slot| slot.borrow().is_some())
}

/// `new Worker(url, options)`：读 url 文件与 options.name、派生 worker、建 Worker 对象。
///
/// # 步骤
/// 1. 构造形态校验（HTML 规范只有 `[[Construct]]`，非构造调用抛 TypeError）。
/// 2. 读 `args[1]` 为 url 字符串（非字符串抛 TypeError）。
/// 3. 读 `args[2]` 为 options 对象（缺省 undefined）；是对象时经 `ordinary_get`
///    读 `name` 属性（getter 副作用与异常原值传播），是字符串取之、非字符串取空串。
/// 4. 主线程侧读 url 文件（`std::fs::read_to_string`），失败抛 TypeError。
/// 5. `vm.spawn_worker(source, name, url)` 派生 worker，得 worker 编号。
/// 6. 建 Worker 对象：PLAIN 对象（`[[Prototype]]` → Worker.prototype）加非枚举
///    `workerId` 数据属性（值为 worker 编号）。`onmessage`/`onerror`/
///    `onmessageerror` 不在对象上建自有属性（改原型访问器属性）。无 native 盒、无 GC 家族。
/// 7. 返回 Worker 对象。
///
/// # 边界与前提
/// - `url` 是 worker 脚本的文件路径（主线程侧读取）。
/// - Worker 对象经 `alloc_object` 入对象表即成 GC 根；`workerId` 是数值（无 GC 边）。
///
/// # 副作用
/// - 派生一个 OS 线程（`spawn_worker`）；登记一个 `WorkerHandle` 与一条
///   Worker 对象注册表条目（GC 根）。
pub(crate) fn worker_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    // 构造形态校验（HTML 规范只有 [[Construct]]）。
    if !vm.constructing_native {
        return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "Worker constructor requires new"));
    }

    // 读 url 字符串（args[1]）。
    let url_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let url = match vm.lookup_str(url_val) {
        Some(u) => u,
        None => return NativeResult::Err(oxide_builtins::error::create_type_error(vm, "Worker url must be a string")),
    };

    // 读 options 对象（args[2]，缺省 undefined）。
    let options_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    // name：options 是对象时经 ordinary_get 读 name 属性（getter 副作用与异常
    // 原值传播），是字符串取之，非字符串或缺省取空串。
    let name = if options_val.is_object() {
        let opts_ptr = options_val.as_js_object_ptr();
        // SAFETY: opts_ptr 是 options 对象，native 调用期间存活。
        let opts_obj = unsafe { &*opts_ptr };
        let si_name = vm.perm_intern("name");
        let name_val = match vm.ordinary_get(opts_obj, si_name, options_val) {
            Ok(v) => v,
            Err(e) => return NativeResult::Err(oxide_builtins::error::create_type_error(vm, &e)),
        };
        vm.lookup_str(name_val).unwrap_or_default()
    } else {
        String::new()
    };

    // 主线程侧读 url 文件（worker 脚本源码）。
    let source = match std::fs::read_to_string(&url) {
        Ok(s) => s,
        Err(e) => {
            return NativeResult::Err(oxide_builtins::error::create_type_error(
                vm,
                &format!("failed to read worker script: {e}"),
            ))
        }
    };

    // 派生 worker（建 WorkerHandle 并 spawn OS 线程），传 name 与 url（location）。
    let id = match vm.spawn_worker(&source, &name, &url) {
        Ok(id) => id,
        Err(e) => return NativeResult::Err(oxide_builtins::error::create_type_error(vm, &e)),
    };

    // 建 Worker 对象：PLAIN 对象（[[Prototype]] → Worker.prototype）。
    let worker_proto_val = worker_prototype_value(vm);
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, worker_proto_val);
    let ptr = vm.alloc_object(obj);
    // SAFETY: ptr 是本函数刚 alloc_object 的对象，存活且本段无别名。
    let worker_obj = unsafe { &mut *ptr };

    // onmessage / onerror / onmessageerror 不在对象上建自有属性：改 Worker.prototype
    // 访问器属性（getter 读注册表属性条目、setter 写），经既有 dispatchEvent 路径交付。

    // workerId：非枚举数据属性（值为 worker 编号，数值表示无 GC 边）。
    let si_worker_id = vm.perm_intern("workerId");
    let _ = vm.define_data_property(
        worker_obj,
        si_worker_id,
        JsValue::float(id as f64),
        PropAttributes::new(true, false, true),
    );

    // 登记 Worker 对象进注册表（GC 根，主线程事件循环据编号反查 onmessage）。
    vm.worker_objects.insert(id, JsValue::from_js_object(ptr));

    NativeResult::Ok(JsValue::from_js_object(ptr))
}

/// 读全局 `Worker` 构造器的 `prototype` 属性（Worker.prototype）。
///
/// # 边界
/// - 全局 `Worker` 槽缺失或构造器无 `prototype` 属性时回退到 `%Object.prototype%`
///   （防御性兜底，正常路径 `bind_worker` 已安装）。
fn worker_prototype_value(vm: &mut Vm) -> JsValue {
    let object_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::ObjectProto));
    let global = vm.global_object();
    let global_ptr = global.as_ptr() as *mut JsObject;
    // SAFETY: global_ptr 是当前 session 的 global 对象，存活。
    let global_obj = unsafe { &*global_ptr };
    let si_worker = vm.perm_intern("Worker");
    let Some(store) = vm.get_own_property_slot(global_obj, si_worker) else {
        return object_proto;
    };
    let ctor_val = global_obj.get_prop_at(store);
    if !ctor_val.is_object() {
        return object_proto;
    }
    let ctor_ptr = ctor_val.as_js_object_ptr();
    // SAFETY: ctor_ptr 是 Worker 构造器对象，存活。
    let ctor_obj = unsafe { &*ctor_ptr };
    let si_prototype = vm.perm_intern("prototype");
    let Some(proto_store) = vm.get_own_property_slot(ctor_obj, si_prototype) else {
        return object_proto;
    };
    let proto_val = ctor_obj.get_prop_at(proto_store);
    if proto_val.is_object() {
        proto_val
    } else {
        object_proto
    }
}

/// 读 `this` 的 `workerId` 自有属性（快路径：`get_own_property_slot` + `get_prop_at`）。
///
/// # 边界
/// - `this` 非对象或缺 `workerId` 自有属性时抛 TypeError。
/// - `workerId` 数值转 `u64`（非有限数值或负数归 0）。
fn read_worker_id(vm: &mut Vm, this_val: JsValue) -> Result<u64, JsValue> {
    if !this_val.is_object() {
        return Err(oxide_builtins::error::create_type_error(vm, "worker method called on non-object"));
    }
    let this_obj = unsafe { &*this_val.as_js_object_ptr() };
    let si_worker_id = vm.perm_intern("workerId");
    let Some(store) = vm.get_own_property_slot(this_obj, si_worker_id) else {
        return Err(oxide_builtins::error::create_type_error(vm, "workerId not found"));
    };
    let id_val = this_obj.get_prop_at(store);
    let n = vm
        .coerce_number_bounded(id_val)
        .map_err(|e| oxide_builtins::error::create_type_error(vm, &e))?;
    Ok(n.max(0.0) as u64)
}

/// `worker.postMessage(msg, transfer)`：读 `workerId`、解析 transfer、detach 消息、投递到 worker。
///
/// # 步骤
/// 1. 读 `this`（`args[0]`）的 `workerId` 自有属性（快路径）。
/// 2. 读 `args[1]` 为 msg、`args[2]` 为 transfer，经 `parse_transfer_list` 解析
///    transfer 集合（undefined 返空集、违例报 DataCloneError）。
/// 3. 经 `detach_message` 转 `MessageValue`（transfer 命中的缓冲源 detach）。
/// 4. 委托 `vm.worker_post_message(id, mv)`（935.3 固有方法，内部 `send`）。
///
/// # 边界
/// - DataCloneError 经 `parse_transfer_list` 与 `detach_message` 的 `Err` 臂抛出。
/// - worker 通道已断开（worker 线程退出）时 `vm.worker_post_message` 返 `Err`。
pub(crate) fn worker_post_message(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let id = match read_worker_id(vm, this_val) {
        Ok(id) => id,
        Err(e) => return NativeResult::Err(e),
    };
    let msg_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let transfer_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let transfer = match parse_transfer_list(vm, transfer_val) {
        Ok(s) => s,
        Err(e) => return NativeResult::Err(e),
    };
    let mv = match detach_message(vm, msg_val, &transfer) {
        Ok(mv) => mv,
        Err(e) => return NativeResult::Err(e),
    };
    match vm.worker_post_message(id, mv) {
        Ok(()) => NativeResult::Ok(JsValue::undefined()),
        Err(e) => NativeResult::Err(oxide_builtins::error::create_type_error(vm, &e)),
    }
}

/// `worker.terminate()`：读 `workerId`、委托 `vm.worker_terminate(id)`（置 terminate 位并 join）。
///
/// # 边界
/// - `id` 不存在时 `vm.worker_terminate` 返 `Err`。
pub(crate) fn worker_terminate(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let id = match read_worker_id(vm, this_val) {
        Ok(id) => id,
        Err(e) => return NativeResult::Err(e),
    };
    match vm.worker_terminate(id) {
        Ok(()) => NativeResult::Ok(JsValue::undefined()),
        Err(e) => NativeResult::Err(oxide_builtins::error::create_type_error(vm, &e)),
    }
}

/// `self.postMessage(msg, transfer)`（worker 侧）：解析 transfer、detach 消息、发回主线程。
///
/// # 步骤
/// 1. 读 `args[1]` 为 msg、`args[2]` 为 transfer，经 `parse_transfer_list` 解析
///    transfer 集合（undefined 返空集、违例报 DataCloneError）。
/// 2. 经 `detach_message` 转 `MessageValue`（transfer 命中的缓冲源 detach）。
/// 3. 经 thread-local `WORKER_OUT_TX` 发回主线程（主 realm 无通道时静默丢弃）。
///
/// # 边界
/// - DataCloneError 经 `parse_transfer_list` 与 `detach_message` 的 `Err` 臂抛出。
/// - 主 realm（无 `WORKER_OUT_TX`）或主线程已 drop 接收端时静默丢弃。
pub(crate) fn self_post_message(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let transfer_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let transfer = match parse_transfer_list(vm, transfer_val) {
        Ok(s) => s,
        Err(e) => return NativeResult::Err(e),
    };
    let mv = match detach_message(vm, msg_val, &transfer) {
        Ok(mv) => mv,
        Err(e) => return NativeResult::Err(e),
    };
    WORKER_OUT_TX.with(|slot| {
        if let Some(tx) = slot.borrow().as_ref() {
            let _ = tx.send(WorkerOutMail::Message(mv));
        }
    });
    NativeResult::Ok(JsValue::undefined())
}

/// `self.close()`（worker 侧）：置位自关停请求（事件循环每轮检查后退出）。
///
/// # 副作用
/// - 置位本线程的 `WORKER_CLOSE_REQUESTED`（事件循环下一轮检查后退出）。
pub(crate) fn self_close(_vm: &mut Vm, _args: &[u8]) -> NativeResult {
    request_worker_close();
    NativeResult::Ok(JsValue::undefined())
}

/// `self.name` getter（worker 侧）：读本线程 worker 名称（未置位时返空串）。
pub(crate) fn self_name(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let name = WORKER_NAME.with(|slot| slot.borrow().as_deref().unwrap_or("").to_string());
    NativeResult::Ok(vm.new_string(&name))
}

/// `self.location` getter（worker 侧）：读本线程 worker 位置（未置位时返空串）。
pub(crate) fn self_location(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    let location = WORKER_LOCATION.with(|slot| slot.borrow().as_deref().unwrap_or("").to_string());
    NativeResult::Ok(vm.new_string(&location))
}

/// 从当前函数对象的 `name` 读事件类型（`get onmessage` / `set onmessage` →
/// `"message"`）。
///
/// # 步骤
/// 1. 读 callee 槽（寄存器 254）为当前函数对象（native 调用收口置位）。
/// 2. 读函数对象 `name` 自有属性，剥 `get ` / `set ` 前缀与 `on` 前缀得事件类型串。
/// 3. intern 事件类型串返 si。
///
/// # 边界
/// - callee 非对象、无 `name` 属性、或非 `get on<type>` / `set on<type>` 形态时返 `None`。
fn read_event_type_si(vm: &mut Vm) -> Option<u32> {
    let callee = vm.reg(254);
    if !callee.is_object() {
        return None;
    }
    let callee_ptr = callee.as_js_object_ptr();
    // SAFETY: callee 是当前函数对象，native 调用期间存活。
    let callee_obj = unsafe { &*callee_ptr };
    let si_name = vm.perm_intern("name");
    let store = vm.get_own_property_slot(callee_obj, si_name)?;
    let name_val = callee_obj.get_prop_at(store);
    let name = vm.lookup_str(name_val)?;
    let rest = name.strip_prefix("get ").or_else(|| name.strip_prefix("set "))?;
    let type_text = rest.strip_prefix("on")?;
    Some(vm.perm_intern(type_text))
}

/// `onmessage` / `onerror` / `onmessageerror` 访问器 getter（Worker.prototype 与
/// worker global 共用）：读注册表属性条目返回调，无则 null。
///
/// # 步骤
/// 1. 读 `this`（`args[0]`）为目标对象（非对象返 null）。
/// 2. 从函数对象 `name` 读事件类型 si。
/// 3. 查注册表取该目标同类型属性监听器条目，返其回调（无则 null）。
///
/// # 边界
/// - 注册表条目按目标对象裸指针为键（弱引用，不保活）。
pub(crate) fn worker_event_handler_getter(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Ok(JsValue::null());
    }
    let this_ptr = this_val.as_js_object_ptr();
    let Some(type_si) = read_event_type_si(vm) else {
        return NativeResult::Ok(JsValue::null());
    };
    for entry in vm.et_lookup(this_ptr) {
        if entry.is_attribute && entry.type_si == type_si {
            return NativeResult::Ok(entry.callback);
        }
    }
    NativeResult::Ok(JsValue::null())
}

/// `onmessage` / `onerror` / `onmessageerror` 访问器 setter（Worker.prototype 与
/// worker global 共用）：写注册表属性条目（可调用则登记、null / undefined 则移除）。
///
/// # 步骤
/// 1. 读 `this`（`args[0]`）为目标对象（非对象 no-op）。
/// 2. 从函数对象 `name` 读事件类型 si。
/// 3. 移除该目标同类型的既有属性监听器条目（若有）。
/// 4. 值为可调用时登记新属性监听器条目（`is_attribute` 置位）；null / undefined 不登记。
///
/// # 副作用
/// - 写注册表（`et_unregister` / `et_register`）。
pub(crate) fn worker_event_handler_setter(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Ok(JsValue::undefined());
    }
    let this_ptr = this_val.as_js_object_ptr();
    let Some(type_si) = read_event_type_si(vm) else {
        return NativeResult::Ok(JsValue::undefined());
    };
    let value = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    // 移除该目标同类型的既有属性监听器条目（若有）。
    for entry in vm.et_lookup(this_ptr) {
        if entry.is_attribute && entry.type_si == type_si {
            vm.et_unregister(this_ptr, entry.type_si, entry.callback, entry.capture);
        }
    }
    // 值为可调用时登记新属性监听器条目。
    if oxide_builtins::iterator::is_callable(value) {
        vm.et_register(
            this_ptr,
            ListenerEntry {
                type_si,
                callback: value,
                capture: false,
                once: false,
                is_attribute: true,
            },
        );
    }
    NativeResult::Ok(JsValue::undefined())
}

/// 消息事件构造辅助：建真实 MessageEvent 盒（`data` + `type`，`ports` 空数组）。
///
/// 消息路径（`deliver_worker_messages` / `deliver_self_message`）经本辅助建
/// 真实 MessageEvent，`e instanceof MessageEvent` 与 `e instanceof Event` 均
/// 成立，`e.data` 经原型 getter 读盒字段。
///
/// # 步骤
/// 1. 读 `data`（`args[1]`）与 `type`（`args[2]`）。
/// 2. 委托 `create_message_event_box` 建真实 MessageEvent 盒（`origin` 空串、
///    `lastEpoch` 0、`source` null、`ports` 空数组，`[[Prototype]]` →
///    `MessageEvent.prototype`），使 `dispatchEvent` 品牌守卫通过。
/// 3. 返回对象。
///
/// # 边界与前提
/// - `data` 是 rehydrate 后的消息值（GC 边，经盒边入 mark）。
/// - `type` 是事件类型串（`"message"`）。
///
/// # 副作用
/// - 新建一个 MessageEvent 盒对象（经 `alloc_object` 入对象表）。
pub(crate) fn message_event_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let data_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let type_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };

    // 建真实 MessageEvent 盒（type 取 type_val 的字符串形态）。
    let type_text = vm.lookup_str(type_val).unwrap_or_default();
    let ptr = oxide_builtins::event::create_message_event_box(vm, data_val, &type_text);
    NativeResult::Ok(JsValue::from_js_object(ptr))
}

/// 错误事件构造辅助：建真实 ErrorEvent 盒（`message` 为错误串、`filename`
/// 空串、`lineno` 与 `colno` 为 0、`error` 为 null）。
///
/// 错误路径（`deliver_error_event`）经本辅助建真实 ErrorEvent，
/// `e instanceof ErrorEvent` 与 `e instanceof Event` 均成立，`e.message`
/// 经原型 getter 读盒字段。
///
/// # 步骤
/// 1. 读 `type`（`args[1]`）与 `message`（`args[2]`）。
/// 2. 委托 `create_error_event_box` 建真实 ErrorEvent 盒（`[[Prototype]]` →
///    `ErrorEvent.prototype`），使 `dispatchEvent` 品牌守卫通过。
/// 3. 返回对象。
///
/// # 边界与前提
/// - `type` 是事件类型串（`"error"`/`"messageerror"`）。
/// - `message` 是错误消息串（GC 边，经盒边入 mark）。
///
/// # 副作用
/// - 新建一个 ErrorEvent 盒对象（经 `alloc_object` 入对象表）。
pub(crate) fn build_error_event(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let type_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let message_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };

    // 建真实 ErrorEvent 盒（type 与 message 取实参的字符串形态）。
    let type_text = vm.lookup_str(type_val).unwrap_or_default();
    let message_text = vm.lookup_str(message_val).unwrap_or_default();
    let ptr = oxide_builtins::event::create_error_event_box(vm, &message_text, &type_text);
    NativeResult::Ok(JsValue::from_js_object(ptr))
}
