//! MessageChannel / MessagePort 行为面：`MessageChannel` 构造器（建双向 mpsc
//! 通道与两枚端口对象）、`MessagePort` 原型方法（postMessage 同步交付 /
//! close / start / readyState）与 `onmessage` 属性。
//!
//! 端口载荷盒（mpsc 双端与对端端口边）存于对象 `native_fn` 槽，向 GC 家族表
//! 暴露 edges/size/drop 三自由函数。

use std::collections::HashSet;
use std::mem;

use oxide_kernel::message_queue;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, ProtoKind, VmHost};

use crate::message_value::{detach_message, rehydrate_message, MessageValue};

/// MessagePort 载荷盒：存于对象 `native_fn` 槽（`Box::into_raw`，realm 局部
/// 单线程）。`peer` 是发往对端的 mpsc 发送端（无 GC 边），`rx` 是收自对端的
/// mpsc 接收端（无 GC 边），`peer_port` 是对端端口对象的裸指针（双向 GC 对象
/// 边，保对端存活），`closed` 是关闭标志。
///
struct MessagePortInner {
    peer: message_queue::Sender<MessageValue>,
    rx: message_queue::Receiver<MessageValue>,
    peer_port: *mut JsObject,
    closed: bool,
}

/// 取载荷盒指针：仅 MessagePort 标签对象返回 `Some`，其余标签返回 `None`。
fn message_port_payload_ptr(obj: &JsObject) -> Option<*mut MessagePortInner> {
    if !obj.is_message_port() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut MessagePortInner)
}

/// 收集 MessagePort 的对端端口边（GC 根边）。
///
/// `peer_port` 空窗口（构造期回填之前）返回空边集；边函数自守，语义自含。
pub fn message_port_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(ptr) = message_port_payload_ptr(obj) else {
        return Vec::new();
    };
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: ptr 指向有效载荷盒（槽位仅构造期写入、释放时置空），
    // 只读单个裸指针字段，不搬移载荷。
    let peer_port = unsafe { (*ptr).peer_port };
    if peer_port.is_null() {
        return Vec::new();
    }
    vec![JsValue::from_js_object(peer_port)]
}

/// 只读核算 MessagePort 载荷盒字节（不释放）。
///
/// mpsc 内部缓冲在 Arc 通道内、盒不可寻址，不计入（有界轻微少计，
/// 只推迟 GC 触发，不影响触发正确性）。
pub fn message_port_native_size(obj: &JsObject) -> u64 {
    let Some(ptr) = message_port_payload_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    mem::size_of::<MessagePortInner>() as u64
}

/// 释放 MessagePort 载荷盒（`Box<MessagePortInner>`），返回释放字节数。
///
/// 盒 drop 时发送端与接收端各减一次 mpsc 引用计数；`peer_port` 裸指针
/// 无 drop。槽置空保证重复调用零释放（幂等）。
pub fn drop_message_port_native(obj: &mut JsObject) -> u64 {
    let bytes = message_port_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = message_port_payload_ptr(obj) else {
        return 0;
    };
    // SAFETY: ptr 非空（message_port_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe { drop(Box::from_raw(ptr)) };
    obj.set_native_fn(None);
    bytes
}

/// MessageChannel 构造器：建两条 mpsc 通道与两枚端口对象，回填双向
/// peer_port 边，返回 `{port1, port2}` 结果对象。
///
/// # 步骤
/// 1. 非构造形态（`!constructing_native`）抛 TypeError（HTML 规范只有
///    [[Construct]]，无 [[Call]]）
/// 2. 建两条 mpsc 通道（A→B 与 B→A），载荷类型 `MessageValue`
/// 3. 建两枚端口对象（proto = %MessagePort.prototype%，载荷盒入 native_fn 槽）
/// 4. 回填双向 peer_port（裸指针写，跨 JS 调用不持借用）
/// 5. 两端口各 define onmessage 数据属性 = null（非枚举）
/// 6. 建 plain 结果对象（proto = %Object.prototype%），define 可枚举
///    port1 / port2
///
/// # 副作用
/// - 两端口对象经 `alloc_object` 入对象表即成 GC 根；
/// - 双向 peer_port 边闭合环（两端口自然共亡拆环）。
///
/// # 注意事项
/// - 端口固定挂 %MessagePort.prototype%，不读 new.target（与 HTML 规范一致）；
/// - 构造器不占 BuiltinWorld P 字段（绑定层就地填充占位）。
pub fn message_channel_constructor<H: VmHost>(vm: &mut H, _args: &[u8]) -> NativeResult {
    if !vm.constructing_native() {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "MessageChannel constructor requires new",
        ));
    }

    let (a_to_b_tx, b_rx) = message_queue::channel::<MessageValue>();
    let (b_to_a_tx, a_rx) = message_queue::channel::<MessageValue>();
    let proto_val = JsValue::from_js_object(vm.builtin_proto(ProtoKind::MessagePortProto));

    let (a_ptr, a_inner) = make_port_object(vm, proto_val, a_to_b_tx, a_rx);
    let (b_ptr, b_inner) = make_port_object(vm, proto_val, b_to_a_tx, b_rx);

    // 回填双向 peer_port：peer_port 空窗口的唯一来源，回填后双向 GC 边环闭合。
    // SAFETY: a_inner / b_inner 是刚建的载荷盒，本段存活，只写单个裸指针字段。
    unsafe {
        (*a_inner).peer_port = b_ptr;
        (*b_inner).peer_port = a_ptr;
    }

    // onmessage：普通 own 数据属性（非枚举），初值 null。
    let si_onmessage = vm.perm_intern("onmessage");
    // SAFETY: a_ptr / b_ptr 是本函数刚 alloc_object 的端口对象，存活且本段无别名。
    unsafe {
        let a_obj = &mut *a_ptr;
        let _ = vm.define_data_property(
            a_obj,
            si_onmessage,
            JsValue::null(),
            PropAttributes::new(true, false, true),
        );
        let b_obj = &mut *b_ptr;
        let _ = vm.define_data_property(
            b_obj,
            si_onmessage,
            JsValue::null(),
            PropAttributes::new(true, false, true),
        );
    }

    // 结果对象：plain 对象（proto = %Object.prototype%），两枚可枚举 port1 / port2。
    let object_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::ObjectProto));
    let result_ptr = vm.alloc_object(JsObject::new_empty(EMPTY_SHAPE_ID, object_proto));
    // SAFETY: result_ptr 是本函数刚 alloc_object 的对象，存活且本段无别名。
    let result_obj = unsafe { &mut *result_ptr };
    let si_port1 = vm.perm_intern("port1");
    let _ = vm.define_data_property(
        result_obj,
        si_port1,
        JsValue::from_js_object(a_ptr),
        PropAttributes::DEFAULT_DATA,
    );
    let si_port2 = vm.perm_intern("port2");
    let _ = vm.define_data_property(
        result_obj,
        si_port2,
        JsValue::from_js_object(b_ptr),
        PropAttributes::DEFAULT_DATA,
    );
    NativeResult::Ok(JsValue::from_js_object(result_ptr))
}

/// 建单枚端口对象：空形状对象挂载荷盒，`alloc_object` 入对象表。
fn make_port_object<H: VmHost>(
    vm: &mut H,
    proto_val: JsValue,
    peer: message_queue::Sender<MessageValue>,
    rx: message_queue::Receiver<MessageValue>,
) -> (*mut JsObject, *mut MessagePortInner) {
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
    obj.type_tag = JsObject::OBJ_TYPE_MESSAGE_PORT;
    let inner = Box::into_raw(Box::new(MessagePortInner {
        peer,
        rx,
        peer_port: std::ptr::null_mut(),
        closed: false,
    }));
    // SAFETY: 端口对象不可调用，native_fn 存不透明 `Box<MessagePortInner>` 指针，
    // 与 ArrayBuffer / RegExp 的类型化存储模式一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(inner as *const ()) }));
    let ptr = vm.alloc_object(obj);
    (ptr, inner)
}

/// 解析 postMessage 的 transfer 参数为 ArrayBuffer 指针集合。
///
/// # 步骤
/// 1. undefined 返空集
/// 2. ToList 迭代一切可迭代（数组 / 字符串 / 生成器 / Map / Set）
/// 3. 每元素须为 ArrayBuffer，重复元素报 DataCloneError
///
/// # 边界
/// - 不可迭代对象经 GetIterator 抛 TypeError（原值透传）；
/// - 迭代中途抛错经 IteratorClose 后透传原异常；
/// - 非 AB 元素 / 重复元素报 DataCloneError（与 structuredClone 口径统一）。
fn parse_transfer_list<H: VmHost>(vm: &mut H, transfer: JsValue) -> Result<HashSet<*const JsObject>, JsValue> {
    let mut set = HashSet::new();
    if transfer.is_undefined() {
        return Ok(set);
    }
    crate::iterator::iterate_elements(vm, transfer, |vm, e| {
        if !e.is_object() || !unsafe { &*e.as_js_object_ptr() }.is_array_buffer_obj() {
            return Err(crate::structured_clone::data_clone_error(
                vm,
                "transfer list element is not an ArrayBuffer",
            ));
        }
        if !set.insert(e.as_js_object_ptr() as *const JsObject) {
            return Err(crate::structured_clone::data_clone_error(vm, "duplicate buffer in transfer list"));
        }
        Ok(())
    })?;
    Ok(set)
}

/// MessagePort.postMessage(message, transfer)：detach 消息、入队对端 mpsc、
/// 同步 `drain_port` 排空对端队列触发对端 onmessage。
///
/// # 步骤
/// 1. this 须为 MessagePort，closed 则 TypeError
/// 2. 解析 transfer 列表（undefined → 空集；违例报 DataCloneError）
/// 3. `detach_message` 产 `MessageValue`（失败原值透传）
/// 4. 入队对端 mpsc（SendError 理论不达，防御性报 TypeError）
/// 5. `drain_port` 排空对端队列并触发对端 onmessage
///
/// # 边界与前提
/// - 源对象由调用方寄存器保活；
/// - 同步重入：handler 内再 postMessage 同通道则嵌套 drain（try_recv 非阻塞，无死锁）。
pub fn message_port_post_message<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "postMessage called on a non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let this_obj = unsafe { &*this_ptr };
    let Some(inner_ptr) = message_port_payload_ptr(this_obj) else {
        return NativeResult::Err(crate::error::create_type_error(vm, "postMessage called on a non-MessagePort"));
    };
    // SAFETY: inner_ptr 经 message_port_payload_ptr 校验为存活载荷盒；标量读取
    // 短命，不跨 JS 调用。
    if unsafe { (*inner_ptr).closed } {
        return NativeResult::Err(crate::error::create_type_error(vm, "port is closed"));
    }

    let message = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let transfer = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let transfer_set = match parse_transfer_list(vm, transfer) {
        Ok(s) => s,
        Err(e) => return NativeResult::Err(e),
    };
    let mv = match detach_message(vm, message, &transfer_set) {
        Ok(m) => m,
        Err(e) => return NativeResult::Err(e),
    };

    // SAFETY: 同 inner_ptr 校验；peer_port 构造期回填完成，指针有效。
    let peer_port = unsafe { (*inner_ptr).peer_port };
    if peer_port.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "port has no peer"));
    }
    // SAFETY: 对端 rx 由对端载荷盒保活，对端存活则接收端存活，SendError 理论
    // 不达（防御性报 TypeError）。
    if unsafe { (*inner_ptr).peer.send(mv) }.is_err() {
        return NativeResult::Err(crate::error::create_type_error(vm, "peer port is gone"));
    }

    match drain_port(vm, peer_port) {
        Ok(()) => NativeResult::Ok(JsValue::undefined()),
        Err(e) => NativeResult::Err(e),
    }
}

/// 排空对端队列：逐条 rehydrate 并触发对端 onmessage。
///
/// # 步骤
/// 1. 读对端载荷盒（对端必存活——双向 GC 边），盒缺失防御性返回 Ok
/// 2. try_recv 循环：Empty / Disconnected 退出；`rehydrate_message` 得 data；
///    `ordinary_get` 读 handler（读期用户代码窗口，异常原值透传）；handler 是
///    函数则 `call_function_sync(handler, peer, [data])`（用户异常原值透传，
///    返回 Err 结束 drain）；handler 非函数（null / undefined / 非函数）则静默丢弃该条
///
/// # 边界与前提
/// - rehydrate 新对象经 `alloc_object` 入对象表即成根，交付后无引用自然回收；
/// - 同步重入：handler 内再 postMessage 同通道则嵌套 drain（try_recv 非阻塞，无死锁）。
fn drain_port<H: VmHost>(vm: &mut H, peer_port: *mut JsObject) -> Result<(), JsValue> {
    // SAFETY: peer_port 是有效端口对象（双向 GC 边保活）。
    let peer_obj = unsafe { &*peer_port };
    let Some(inner_ptr) = message_port_payload_ptr(peer_obj) else {
        return Ok(());
    };
    let peer_val = JsValue::from_js_object(peer_port);
    let si_onmessage = vm.perm_intern("onmessage");
    // SAFETY: inner_ptr 是有效载荷盒，rx 的 try_recv 非阻塞、不跨 JS 调用。
    while let Ok(mv) = unsafe { (*inner_ptr).rx.try_recv() } {
        let data = rehydrate_message(vm, &mv);
        let handler = match vm.ordinary_get(peer_obj, si_onmessage, peer_val) {
            Ok(h) => h,
            Err(e) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return Err(exc);
                }
                return Err(crate::error::create_type_error(vm, &e));
            }
        };
        if !crate::iterator::is_callable(handler) {
            continue;
        }
        if let Err(e) = vm.call_function_sync(handler, peer_val, &[data]) {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::iterator::engine_error(vm, &e));
        }
    }
    Ok(())
}

/// MessagePort.close()：置 closed 标志（幂等，重复关闭 no-op）。
///
/// # 边界
/// - this 须为 MessagePort；
/// - 不拆 peer_port 环（两端口自然共亡拆环）；对端 close 事件延后。
pub fn message_port_close<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "close called on a non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let this_obj = unsafe { &*this_ptr };
    let Some(inner_ptr) = message_port_payload_ptr(this_obj) else {
        return NativeResult::Err(crate::error::create_type_error(vm, "close called on a non-MessagePort"));
    };
    // SAFETY: inner_ptr 是有效载荷盒，标志写短命、不跨 JS 调用。
    unsafe { (*inner_ptr).closed = true };
    NativeResult::Ok(JsValue::undefined())
}

/// MessagePort.start()：no-op（端口自动激活，无需显式启动）。
pub fn message_port_start<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "start called on a non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let this_obj = unsafe { &*this_ptr };
    if message_port_payload_ptr(this_obj).is_none() {
        return NativeResult::Err(crate::error::create_type_error(vm, "start called on a non-MessagePort"));
    }
    NativeResult::Ok(JsValue::undefined())
}

/// MessagePort.readyState：读 closed 标志返 "open" / "closed"。
pub fn message_port_ready_state<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "readyState called on a non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let this_obj = unsafe { &*this_ptr };
    let Some(inner_ptr) = message_port_payload_ptr(this_obj) else {
        return NativeResult::Err(crate::error::create_type_error(vm, "readyState called on a non-MessagePort"));
    };
    // SAFETY: inner_ptr 是有效载荷盒，标志读短命、不跨 JS 调用。
    let closed = unsafe { (*inner_ptr).closed };
    NativeResult::Ok(vm.new_string(if closed { "closed" } else { "open" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
    use oxide_types::object::NativeFnPtr;

    /// 构造真实通道并把双端移入载荷盒。
    fn port_payload() -> MessagePortInner {
        let (peer, rx) = message_queue::channel::<MessageValue>();
        MessagePortInner {
            peer,
            rx,
            peer_port: std::ptr::null_mut(),
            closed: false,
        }
    }

    /// 手工构造带载荷盒的 MessagePort 对象（不经 Vm，只验 GC 三自由函数）。
    fn port_object_with_payload(inner: MessagePortInner) -> JsObject {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.type_tag = JsObject::OBJ_TYPE_MESSAGE_PORT;
        let payload_ptr = Box::into_raw(Box::new(inner));
        // SAFETY: 载荷盒形态与 native_fn 槽存储约定一致，测试结束前恰好释放一次。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        obj
    }

    #[test]
    fn size_and_drop_idempotent() {
        let mut obj = port_object_with_payload(port_payload());
        let struct_bytes = mem::size_of::<MessagePortInner>() as u64;
        assert_eq!(message_port_native_size(&obj), struct_bytes);
        // 首次 drop 释放结构体；槽置空后二次 drop 零释放。
        assert_eq!(drop_message_port_native(&mut obj), struct_bytes);
        assert_eq!(drop_message_port_native(&mut obj), 0);
    }

    #[test]
    fn edges_peer_port_null_and_present() {
        let mut obj = port_object_with_payload(port_payload());
        // peer_port 空窗口（构造期回填之前）：边函数返回零条边。
        assert!(message_port_native_edges(&obj).is_empty());

        // 回填对端端口边：一条对象边。
        let peer = Box::leak(Box::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined())));
        peer.type_tag = JsObject::OBJ_TYPE_MESSAGE_PORT;
        let ptr = message_port_payload_ptr(&obj).unwrap();
        // SAFETY: peer 是测试局部对象，测试期间存活，边只读不搬移。
        unsafe { (*ptr).peer_port = peer as *mut JsObject };
        let edges = message_port_native_edges(&obj);
        assert_eq!(edges.len(), 1);
        assert!(edges[0].is_object());
        // 对端对象是测试局部泄漏，不随端口 drop 释放（裸指针无 drop）。
        drop_message_port_native(&mut obj);
    }

    #[test]
    fn channel_roundtrip_through_box() {
        let mut obj = port_object_with_payload(port_payload());
        let ptr = message_port_payload_ptr(&obj).unwrap();
        // SAFETY: 载荷盒测试期间存活。
        unsafe {
            assert!(!(*ptr).closed);
            (*ptr).peer.send(MessageValue::Number(1.0)).unwrap();
            assert!(matches!((*ptr).rx.try_recv().unwrap(), MessageValue::Number(n) if n == 1.0));
        }
        drop_message_port_native(&mut obj);
    }

    #[test]
    fn payload_ptr_brand_guard() {
        // 非 MessagePort 类型标签（PLAIN）对象取载荷指针恒 None。
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        let payload_ptr = Box::into_raw(Box::new(port_payload()));
        // SAFETY: 同 port_object_with_payload 的载荷盒形态。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        assert!(message_port_payload_ptr(&obj).is_none());
        // SAFETY: 测试构造的盒，恰好释放一次。
        unsafe { drop(Box::from_raw(payload_ptr)) };
    }

    // ---- 端到端行为面（经 Vm 跑脚本，验证构造器与方法绑定）----

    use oxide_vm::vm::Vm;

    /// 编译并执行单脚本，返回 VM 与完成值。
    fn eval(source: &str) -> Result<(Vm, JsValue), String> {
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("parse: {:?}", e))?;
        let module = oxide_compiler::compiler::Compiler::new()
            .compile(&program)
            .map_err(|e| format!("compile: {e}"))?;
        let mut vm = Vm::new();
        let result = vm.run(&std::sync::Arc::new(module))?;
        Ok((vm, result))
    }

    /// 跑一段返回布尔的断言脚本，断言为真。
    fn assert_script(source: &str) {
        let (_vm, result) = eval(source).unwrap();
        assert!(result.as_bool(), "script failed: {source}");
    }

    #[test]
    fn constructor_shape_and_identity() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                return (
                    typeof ch.port1 === 'object' &&
                    typeof ch.port2 === 'object' &&
                    ch.port1 !== ch.port2 &&
                    MessageChannel.length === 0 &&
                    MessageChannel.name === 'MessageChannel' &&
                    Object.getPrototypeOf(ch.port1).constructor === MessageChannel
                );
            })()
            "#,
        );
    }

    #[test]
    fn port_type_tag_is_message_port() {
        let (_vm, result) = eval("new MessageChannel().port1").unwrap();
        assert!(result.is_object());
        // SAFETY: result 是脚本返回的端口对象，测试期间存活。
        let obj = unsafe { &*result.as_js_object_ptr() };
        assert_eq!(obj.type_tag, JsObject::OBJ_TYPE_MESSAGE_PORT);
    }

    #[test]
    fn port_onmessage_is_own_non_enumerable() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var p = ch.port1;
                var desc = Object.getOwnPropertyDescriptor(p, 'onmessage');
                return (
                    desc !== undefined &&
                    desc.value === null &&
                    desc.writable === true &&
                    desc.enumerable === false &&
                    desc.configurable === true
                );
            })()
            "#,
        );
    }

    #[test]
    fn basic_delivery_object() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var received = null;
                ch.port2.onmessage = function(d) { received = d; };
                ch.port1.postMessage({a: 1});
                return received !== null && received.a === 1;
            })()
            "#,
        );
    }

    #[test]
    fn roundtrip_all_cloneable_types() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var got = null;
                ch.port2.onmessage = function(d) { got = d; };
                ch.port1.postMessage({
                    num: 42,
                    neg: -1.5,
                    big: 10n,
                    str: 'hello',
                    bool: true,
                    nil: null,
                    undef: undefined,
                    arr: [1, 2, 3],
                    obj: {x: 1, y: 'z'},
                    map: new Map([['k', 1]]),
                    set: new Set([1, 2]),
                    date: new Date(1700000000000),
                    re: /ab+c/g,
                    err: new Error('boom')
                });
                if (got.num !== 42 || got.neg !== -1.5 || got.big !== 10n) return false;
                if (got.str !== 'hello' || got.bool !== true || got.nil !== null) return false;
                if (!('undef' in got) || got.undef !== undefined) return false;
                if (got.arr.length !== 3 || got.arr[1] !== 2) return false;
                if (got.obj.x !== 1 || got.obj.y !== 'z') return false;
                if (got.map.get('k') !== 1) return false;
                if (got.set.size !== 2 || !got.set.has(2)) return false;
                if (got.date.getTime() !== 1700000000000) return false;
                if (got.re.source !== 'ab+c' || got.re.flags !== 'g') return false;
                if (got.err.name !== 'Error' || got.err.message !== 'boom') return false;
                return true;
            })()
            "#,
        );
    }

    #[test]
    fn transfer_detaches_source() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var ab = new ArrayBuffer(8);
                var received = null;
                ch.port2.onmessage = function(d) { received = d; };
                ch.port1.postMessage(ab, [ab]);
                return ab.byteLength === 0 && received.byteLength === 8;
            })()
            "#,
        );
    }

    #[test]
    fn clone_path_keeps_source_alive() {
        // 未列入 transfer 的 ArrayBuffer 走克隆路径：源不 detach。
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var ab = new ArrayBuffer(4);
                var received = null;
                ch.port2.onmessage = function(d) { received = d; };
                ch.port1.postMessage(ab);
                return ab.byteLength === 4 && received.byteLength === 4 && received !== ab;
            })()
            "#,
        );
    }

    #[test]
    fn data_clone_error_on_function() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                try {
                    ch.port1.postMessage(function(){});
                    return false;
                } catch (e) {
                    return e.name === 'DataCloneError';
                }
            })()
            "#,
        );
    }

    #[test]
    fn data_clone_error_on_circular() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var a = {};
                a.self = a;
                try {
                    ch.port1.postMessage(a);
                    return false;
                } catch (e) {
                    return e.name === 'DataCloneError';
                }
            })()
            "#,
        );
    }

    #[test]
    fn data_clone_error_on_shared_array_buffer() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                try {
                    ch.port1.postMessage(new SharedArrayBuffer(8));
                    return false;
                } catch (e) {
                    return e.name === 'DataCloneError';
                }
            })()
            "#,
        );
    }

    #[test]
    fn transfer_violation_is_data_clone_error() {
        // 非 AB 元素与重复元素均报 DataCloneError（与 structuredClone 口径统一）。
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var ab = new ArrayBuffer(4);
                try {
                    ch.port1.postMessage(1, [ab, 'not-a-buffer']);
                    return false;
                } catch (e) {
                    if (e.name !== 'DataCloneError') return false;
                }
                try {
                    ch.port1.postMessage(1, [ab, ab]);
                    return false;
                } catch (e) {
                    return e.name === 'DataCloneError';
                }
            })()
            "#,
        );
    }

    #[test]
    fn close_semantics() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var p = ch.port1;
                if (p.readyState !== 'open') return false;
                p.close();
                if (p.readyState !== 'closed') return false;
                // 重复关闭幂等 no-op。
                p.close();
                try {
                    p.postMessage(1);
                    return false;
                } catch (e) {
                    return e.name === 'TypeError';
                }
            })()
            "#,
        );
    }

    #[test]
    fn start_is_noop() {
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                ch.port1.start();
                ch.port2.start();
                return ch.port1.readyState === 'open' && ch.port2.readyState === 'open';
            })()
            "#,
        );
    }

    #[test]
    fn no_handler_silently_drops() {
        // 未设 handler 时 postMessage 不抛错、不派发。
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                ch.port1.postMessage({a: 1});
                ch.port1.postMessage('x');
                return ch.port1.readyState === 'open';
            })()
            "#,
        );
    }

    #[test]
    fn non_construct_form_type_error() {
        assert_script(
            r#"
            (function() {
                try {
                    MessageChannel();
                    return false;
                } catch (e) {
                    return e.name === 'TypeError';
                }
            })()
            "#,
        );
    }

    #[test]
    fn non_port_receiver_type_error() {
        // 对非对象 this 直调 native 入口抛 TypeError。
        let (mut vm, _result) = eval("new MessageChannel()").unwrap();
        vm.set_reg(0, JsValue::int(1));
        vm.set_reg(1, JsValue::int(42));
        let args = [0u8, 1];
        match message_port_post_message(&mut vm, &args) {
            NativeResult::Err(e) => {
                assert!(e.is_object());
            }
            _ => panic!("expected TypeError"),
        }
    }

    #[test]
    fn non_port_object_type_error() {
        // 对象但非 MessagePort（plain 对象）调 postMessage 抛 TypeError。
        let (mut vm, plain) = eval("({})").unwrap();
        vm.set_reg(0, plain);
        vm.set_reg(1, JsValue::int(42));
        let args = [0u8, 1];
        match message_port_post_message(&mut vm, &args) {
            NativeResult::Err(e) => {
                assert!(e.is_object());
            }
            _ => panic!("expected TypeError"),
        }
    }

    #[test]
    fn gc_edge_keeps_peer_alive() {
        // 仅持 port1 时 port2 不被回收：双向 GC 边保活对端，
        // 强制收集后对端 handler 仍可触发。
        assert_script(
            r#"
            (function() {
                var ch = new MessageChannel();
                var p1 = ch.port1;
                var received = null;
                (function() {
                    var p2 = ch.port2;
                    p2.onmessage = function(d) { received = d; };
                })();
                $262.gc();
                p1.postMessage({x: 42});
                return received !== null && received.x === 42;
            })()
            "#,
        );
    }
}
