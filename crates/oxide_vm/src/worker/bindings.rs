//! Worker API 的 concrete `&mut Vm` native 函数：构造器、`postMessage`/`terminate`
//! 原型方法、`self` 面（`self.postMessage`/`self.close`/`self.name`/`self.location`）
//! 与 `MessageEvent` 构造辅助。
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

use oxide_builtins::message_value::detach_message;
use oxide_kernel::message_queue::Sender;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{NativeResult, ProtoKind, VmHost};
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

/// `new Worker(url)`：读 url 文件、派生 worker、建 Worker 对象。
///
/// # 步骤
/// 1. 构造形态校验（HTML 规范只有 `[[Construct]]`，非构造调用抛 TypeError）。
/// 2. 读 `args[1]` 为 url 字符串（非字符串抛 TypeError）。
/// 3. 主线程侧读 url 文件（`std::fs::read_to_string`），失败抛 TypeError。
/// 4. `vm.spawn_worker(source)` 派生 worker，得 worker 编号。
/// 5. 建 Worker 对象：PLAIN 对象（`[[Prototype]]` → Worker.prototype）加非枚举
///    `workerId` 数据属性（值为 worker 编号）与 `onmessage`/`onerror`/
///    `onmessageerror` 普通 own 属性（初值 undefined）。无 native 盒、无 GC 家族。
/// 6. 返回 Worker 对象。
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

    // 派生 worker（建 WorkerHandle 并 spawn OS 线程）。
    let id = match vm.spawn_worker(&source) {
        Ok(id) => id,
        Err(e) => return NativeResult::Err(oxide_builtins::error::create_type_error(vm, &e)),
    };

    // 建 Worker 对象：PLAIN 对象（[[Prototype]] → Worker.prototype）。
    let worker_proto_val = worker_prototype_value(vm);
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, worker_proto_val);
    let ptr = vm.alloc_object(obj);
    // SAFETY: ptr 是本函数刚 alloc_object 的对象，存活且本段无别名。
    let worker_obj = unsafe { &mut *ptr };

    // onmessage / onerror / onmessageerror：普通 own 数据属性（非枚举），初值 undefined。
    let si_onmessage = vm.perm_intern("onmessage");
    let _ =
        vm.define_data_property(worker_obj, si_onmessage, JsValue::undefined(), PropAttributes::new(true, false, true));
    let si_onerror = vm.perm_intern("onerror");
    let _ =
        vm.define_data_property(worker_obj, si_onerror, JsValue::undefined(), PropAttributes::new(true, false, true));
    let si_onmessageerror = vm.perm_intern("onmessageerror");
    let _ = vm.define_data_property(
        worker_obj,
        si_onmessageerror,
        JsValue::undefined(),
        PropAttributes::new(true, false, true),
    );

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

/// `worker.postMessage(msg)`：读 `workerId`、detach 消息、投递到 worker。
///
/// # 步骤
/// 1. 读 `this`（`args[0]`）的 `workerId` 自有属性（快路径）。
/// 2. 读 `args[1]` 为 msg，经 `detach_message` 转 `MessageValue`（空 transfer 集合）。
/// 3. 委托 `vm.worker_post_message(id, mv)`（935.3 固有方法，内部 `send`）。
///
/// # 边界
/// - DataCloneError 经 `detach_message` 的 `Err` 臂抛出。
/// - worker 通道已断开（worker 线程退出）时 `vm.worker_post_message` 返 `Err`。
pub(crate) fn worker_post_message(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let id = match read_worker_id(vm, this_val) {
        Ok(id) => id,
        Err(e) => return NativeResult::Err(e),
    };
    let msg_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let transfer: std::collections::HashSet<*const JsObject> = std::collections::HashSet::new();
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

/// `self.postMessage(msg)`（worker 侧）：detach 消息、发回主线程。
///
/// # 步骤
/// 1. 读 `args[1]` 为 msg，经 `detach_message` 转 `MessageValue`（空 transfer 集合）。
/// 2. 经 thread-local `WORKER_OUT_TX` 发回主线程（主 realm 无通道时静默丢弃）。
///
/// # 边界
/// - DataCloneError 经 `detach_message` 的 `Err` 臂抛出。
/// - 主 realm（无 `WORKER_OUT_TX`）或主线程已 drop 接收端时静默丢弃。
pub(crate) fn self_post_message(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let msg_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let transfer: std::collections::HashSet<*const JsObject> = std::collections::HashSet::new();
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

/// `self.name` getter（worker 侧）：返空串（首版占位，规范面对齐留后续）。
#[expect(dead_code)]
pub(crate) fn self_name(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    NativeResult::Ok(vm.new_string(""))
}

/// `self.location` getter（worker 侧）：返空串（首版占位，规范面对齐留后续）。
#[expect(dead_code)]
pub(crate) fn self_location(vm: &mut Vm, _args: &[u8]) -> NativeResult {
    NativeResult::Ok(vm.new_string(""))
}

/// 事件构造辅助：建事件对象（`data` + `type` + `message` 属性）。
///
/// 消息事件与错误事件同型，共用本辅助：消息事件 `message` 为 undefined，
/// 错误事件 `data` 为 undefined、`message` 为错误串。
///
/// # 步骤
/// 1. 建 PLAIN 对象（`[[Prototype]]` → `%Object.prototype%`）。
/// 2. define `data` 属性（rehydrate 后的消息值，可枚举）。
/// 3. define `type` 属性（事件类型串，可枚举）。
/// 4. define `message` 属性（错误消息串，可枚举；消息事件为 undefined）。
/// 5. 返回对象。
///
/// # 边界与前提
/// - `data` 是 rehydrate 后的消息值（GC 边，经 `alloc_object` 入对象表即成根）。
/// - `type` 是事件类型串（`"message"`/`"error"`/`"messageerror"`）。
/// - `message` 是错误消息串（仅错误事件定义，消息事件为 undefined）。
///
/// # 副作用
/// - 新建一个事件对象（经 `alloc_object` 入对象表）。
pub(crate) fn message_event_constructor(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let data_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let type_val = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let message_val = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };

    let object_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::ObjectProto));
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, object_proto);
    let ptr = vm.alloc_object(obj);
    // SAFETY: ptr 是本函数刚 alloc_object 的对象，存活且本段无别名。
    let event_obj = unsafe { &mut *ptr };

    let si_data = vm.perm_intern("data");
    let _ = vm.define_data_property(event_obj, si_data, data_val, PropAttributes::DEFAULT_DATA);
    let si_type = vm.perm_intern("type");
    let _ = vm.define_data_property(event_obj, si_type, type_val, PropAttributes::DEFAULT_DATA);
    let si_message = vm.perm_intern("message");
    let _ = vm.define_data_property(event_obj, si_message, message_val, PropAttributes::DEFAULT_DATA);

    NativeResult::Ok(JsValue::from_js_object(ptr))
}
