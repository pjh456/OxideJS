//! BroadcastChannel 行为面：`new BroadcastChannel(name)` 构造器（建通道对象并
//! 登记 per-realm 注册表）、`postMessage` 原型方法（向同名全部其他通道同步
//! 广播交付）、`close`（注销并关闭）、`onmessage` 普通 own 属性与 `name` /
//! `readyState` 原型 getter。
//!
//! 通道载荷盒（mpsc 对与通道名）存于对象 `native_fn` 槽，向 GC 家族表暴露
//! size/drop 两自由函数。

use std::collections::HashSet;
use std::mem;

use oxide_kernel::message_queue;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_runtime_api::{to_string_full, NativeResult, ProtoKind, VmHost};
use oxide_types::object::{JsObject, NativeFnPtr, PropAttributes};
use oxide_types::value::JsValue;

use crate::message_value::{detach_message, rehydrate_message, MessageValue};

/// BroadcastChannel 载荷盒：存于对象 `native_fn` 槽（`Box::into_raw`，realm
/// 局部单线程）。`name` 是通道名（Rust `String`，注册表键），`tx` 是本通道的
/// mpsc 发送端（`Send`），`rx` 是配对的 mpsc 接收端（`!Send`，realm 局部），
/// `closed` 是关闭标志。`tx` 与 `rx` 是 `channel()` 配对，`tx.send(v)` 写入本
/// 通道 `rx`。
struct BroadcastChannelInner {
    name: String,
    tx: message_queue::Sender<MessageValue>,
    rx: message_queue::Receiver<MessageValue>,
    closed: bool,
}

/// 取载荷盒指针：仅 BroadcastChannel 标签对象返回 `Some`，其余标签返回 `None`。
fn broadcast_channel_payload_ptr(obj: &JsObject) -> Option<*mut BroadcastChannelInner> {
    if !obj.is_broadcast_channel() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut BroadcastChannelInner)
}

/// 只读核算 BroadcastChannel 载荷盒字节（不释放）。
///
/// mpsc 内部缓冲在 Arc 通道内、盒不可寻址，不计入（有界轻微少计，只推迟
/// GC 触发，不影响触发正确性）。
pub fn broadcast_channel_native_size(obj: &JsObject) -> u64 {
    let Some(ptr) = broadcast_channel_payload_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    mem::size_of::<BroadcastChannelInner>() as u64
}

/// 释放 BroadcastChannel 载荷盒（`Box<BroadcastChannelInner>`），返回释放字节数。
///
/// 盒 drop 时发送端与接收端各减一次 mpsc 引用计数，`name` String 随盒释放。
/// 槽置空保证重复调用零释放（幂等）。
pub fn drop_broadcast_channel_native(obj: &mut JsObject) -> u64 {
    let bytes = broadcast_channel_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = broadcast_channel_payload_ptr(obj) else {
        return 0;
    };
    // SAFETY: ptr 非空（broadcast_channel_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe { drop(Box::from_raw(ptr)) };
    obj.set_native_fn(None);
    bytes
}

/// BroadcastChannel 构造器：建通道对象（载荷盒 + onmessage own 属性）并登记
/// per-realm 注册表，返回通道对象。
///
/// # 步骤
/// 1. 非构造形态（`!constructing_native`）抛 TypeError（HTML 规范只有
///    [[Construct]]，无 [[Call]]）
/// 2. `name` 实参经 `to_string_full` 强转 Rust `String`（DOMString 语义：非
///    字符串经 ToPrimitive(string hint)，Symbol 抛 TypeError，不静默降级）
/// 3. 建 mpsc 通道（载荷类型 `MessageValue`），建通道对象（proto =
///    %BroadcastChannel.prototype%，载荷盒入 native_fn 槽）
/// 4. define onmessage 数据属性 = null（非枚举）
/// 5. 登记通道对象进 per-realm 注册表（弱引用）
///
/// # 副作用
/// - 通道对象经 `alloc_object` 入对象表即成 GC 根；
/// - 注册表条目为弱引用，不保活通道（GC sweep 剪枝）。
///
/// # 注意事项
/// - 通道固定挂 %BroadcastChannel.prototype%，不读 new.target；
/// - 构造器不占 BuiltinWorld P 字段（绑定层就地填充占位）。
pub fn broadcast_channel_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if !vm.constructing_native() {
        return NativeResult::Err(crate::error::create_type_error(vm, "BroadcastChannel constructor requires new"));
    }

    let name_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let name = match to_string_full(name_val, vm) {
        Ok(s) => s,
        Err(e) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, &e));
        }
    };

    let (tx, rx) = message_queue::channel::<MessageValue>();
    let proto_val = JsValue::from_js_object(vm.builtin_proto(ProtoKind::BroadcastChannelProto));
    let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
    obj.type_tag = JsObject::OBJ_TYPE_BROADCAST_CHANNEL;
    let inner = Box::into_raw(Box::new(BroadcastChannelInner { name, tx, rx, closed: false }));
    // SAFETY: 通道对象不可调用，native_fn 存不透明 `Box<BroadcastChannelInner>` 指针，
    // 与 ArrayBuffer / RegExp 的类型化存储模式一致。
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(inner as *const ()) }));
    let ptr = vm.alloc_object(obj);

    // onmessage：普通 own 数据属性（非枚举），初值 null。
    let si_onmessage = vm.perm_intern("onmessage");
    // SAFETY: ptr 是本函数刚 alloc_object 的通道对象，存活且本段无别名。
    unsafe {
        let channel_obj = &mut *ptr;
        let _ =
            vm.define_data_property(channel_obj, si_onmessage, JsValue::null(), PropAttributes::new(true, false, true));
    }

    // 登记 per-realm 注册表（弱引用，不保活通道）。
    // SAFETY: 盒内 name 字段本函数所有，借用短命、不跨 JS 调用。
    let name_str = unsafe { &(*inner).name };
    vm.bc_register(name_str, ptr);

    NativeResult::Ok(JsValue::from_js_object(ptr))
}

/// BroadcastChannel.postMessage(message)：detach 消息、向同名全部其他通道
/// 同步广播交付。
///
/// # 步骤
/// 1. this 须为 BroadcastChannel，closed 则 TypeError
/// 2. `detach_message` 产 `MessageValue`（空 transfer 集，ArrayBuffer 一律克隆；
///    失败原值透传）
/// 3. 发送阶段（无用户调用、无 GC）：查注册表全部同名通道（排除自身），逐条
///    入队对端 mpsc
/// 4. 交付阶段（用户调用、可触发 GC）：逐通道排空队列并触发其 onmessage；
///    每个 drain 前重新校验存活（GC sweep 已剪枝死通道）
///
/// # 边界与前提
/// - 源对象由调用方寄存器保活；
/// - 不广播给自己（`peer_ptr == this_ptr` 跳过）；
/// - 同步重入：handler 内再 postMessage 同通道则嵌套广播（try_recv 非阻塞，无死锁）。
pub fn broadcast_channel_post_message<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "postMessage called on a non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let this_obj = unsafe { &*this_ptr };
    let Some(inner_ptr) = broadcast_channel_payload_ptr(this_obj) else {
        return NativeResult::Err(crate::error::create_type_error(vm, "postMessage called on a non-BroadcastChannel"));
    };
    // SAFETY: inner_ptr 经 broadcast_channel_payload_ptr 校验为存活载荷盒；标量读取
    // 短命，不跨 JS 调用。
    if unsafe { (*inner_ptr).closed } {
        return NativeResult::Err(crate::error::create_type_error(vm, "channel is closed"));
    }

    let message = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let empty_transfer = HashSet::new();
    let mv = match detach_message(vm, message, &empty_transfer) {
        Ok(m) => m,
        Err(e) => return NativeResult::Err(e),
    };

    // SAFETY: 同 inner_ptr 校验；盒内 name 字段本函数所有，借用短命、不跨 JS 调用。
    let name = unsafe { &(*inner_ptr).name };

    // 发送阶段：无用户调用、无 GC，peers 指针全部有效。
    let peers = vm.bc_lookup(name);
    for peer_ptr in &peers {
        if *peer_ptr == this_ptr {
            continue;
        }
        // SAFETY: peer_ptr 来自注册表登记，native 执行期间有效。
        let peer_obj = unsafe { &**peer_ptr };
        let Some(peer_inner) = broadcast_channel_payload_ptr(peer_obj) else {
            continue;
        };
        // SAFETY: 对端 rx 由对端载荷盒保活，对端存活则接收端存活，SendError 理论
        // 不达（防御性报 TypeError）。
        if unsafe { (*peer_inner).tx.send(mv.clone()) }.is_err() {
            return NativeResult::Err(crate::error::create_type_error(vm, "peer channel is gone"));
        }
    }

    // 交付阶段：用户调用、可触发 GC；每个 drain 前重新校验存活（GC sweep 已
    // 剪枝死通道），校验通过即该通道在最后一次 sweep 被标记存活、对象仍分配。
    for peer_ptr in &peers {
        if *peer_ptr == this_ptr {
            continue;
        }
        if !vm.bc_lookup(name).contains(peer_ptr) {
            continue;
        }
        if let Err(e) = drain_channel(vm, name, *peer_ptr) {
            return NativeResult::Err(e);
        }
    }
    NativeResult::Ok(JsValue::undefined())
}

/// 排空一个通道的队列：逐条 rehydrate 并触发其 onmessage。
///
/// # 步骤
/// 1. 读通道载荷盒（通道必存活——调用前已校验），盒缺失防御性返回 Ok
/// 2. try_recv 循环：Empty / Disconnected 退出；`rehydrate_message` 得 data；
///    `ordinary_get` 读 handler（读期用户代码窗口，异常原值透传）；读后二次
///    校验存活（getter 窗口 GC 防护）；handler 是函数则
///    `call_function_sync(handler, channel, [data])`（用户异常原值透传，返回
///    Err 结束 drain）；handler 非函数（null / undefined / 非函数）则静默丢弃该条
///
/// # 边界与前提
/// - rehydrate 新对象经 `alloc_object` 入对象表即成根，交付后无引用自然回收；
/// - 同步重入：handler 内再 postMessage 同通道则嵌套 drain（try_recv 非阻塞，无死锁）。
fn drain_channel<H: VmHost>(vm: &mut H, name: &str, channel: *mut JsObject) -> Result<(), JsValue> {
    // SAFETY: channel 是有效通道对象（调用前已校验存活）。
    let channel_obj = unsafe { &*channel };
    let Some(inner_ptr) = broadcast_channel_payload_ptr(channel_obj) else {
        return Ok(());
    };
    let channel_val = JsValue::from_js_object(channel);
    let si_onmessage = vm.perm_intern("onmessage");
    // SAFETY: inner_ptr 是有效载荷盒，rx 的 try_recv 非阻塞、不跨 JS 调用。
    while let Ok(mv) = unsafe { (*inner_ptr).rx.try_recv() } {
        let data = rehydrate_message(vm, &mv);
        let handler = match vm.ordinary_get(channel_obj, si_onmessage, channel_val) {
            Ok(h) => h,
            Err(e) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return Err(exc);
                }
                return Err(crate::error::create_type_error(vm, &e));
            }
        };
        // getter 窗口二次校验：onmessage 读取是用户代码窗口，期间 GC 可释放本
        // 通道并剪枝注册表条目；校验失败即停止 drain（data 是根，无悬垂）。
        if !vm.bc_lookup(name).contains(&channel) {
            return Ok(());
        }
        if !crate::iterator::is_callable(handler) {
            continue;
        }
        if let Err(e) = vm.call_function_sync(handler, channel_val, &[data]) {
            if let Some(exc) = vm.take_uncaught_value() {
                return Err(exc);
            }
            return Err(crate::iterator::engine_error(vm, &e));
        }
    }
    Ok(())
}

/// BroadcastChannel.close()：置 closed 标志并从 per-realm 注册表注销
/// （幂等，重复关闭 no-op）。
///
/// # 边界
/// - this 须为 BroadcastChannel；
/// - 不拆 tx/rx 环（两端口自然共亡）。
pub fn broadcast_channel_close<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "close called on a non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let this_obj = unsafe { &*this_ptr };
    let Some(inner_ptr) = broadcast_channel_payload_ptr(this_obj) else {
        return NativeResult::Err(crate::error::create_type_error(vm, "close called on a non-BroadcastChannel"));
    };
    // SAFETY: inner_ptr 是有效载荷盒，标志写短命、不跨 JS 调用。
    unsafe { (*inner_ptr).closed = true };
    // 注册表注销（幂等，重复关闭不报错）。
    // SAFETY: 盒内 name 字段本函数所有，借用短命、不跨 JS 调用。
    let name = unsafe { &(*inner_ptr).name };
    vm.bc_unregister(name, this_ptr);
    NativeResult::Ok(JsValue::undefined())
}

/// BroadcastChannel.name：读通道名字段返 JS 字符串。
pub fn broadcast_channel_name<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "name called on a non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let this_obj = unsafe { &*this_ptr };
    let Some(inner_ptr) = broadcast_channel_payload_ptr(this_obj) else {
        return NativeResult::Err(crate::error::create_type_error(vm, "name called on a non-BroadcastChannel"));
    };
    // SAFETY: inner_ptr 是有效载荷盒，name 字段读短命、不跨 JS 调用。
    let name = unsafe { (*inner_ptr).name.clone() };
    NativeResult::Ok(vm.new_string_owned(name))
}

/// BroadcastChannel.readyState：读 closed 标志返 "open" / "closed"。
pub fn broadcast_channel_ready_state<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "readyState called on a non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();
    // SAFETY: is_object 保证非空指针；对象在 native 执行期间被根保持、不被回收。
    let this_obj = unsafe { &*this_ptr };
    let Some(inner_ptr) = broadcast_channel_payload_ptr(this_obj) else {
        return NativeResult::Err(crate::error::create_type_error(vm, "readyState called on a non-BroadcastChannel"));
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
    use oxide_types::value::JsValue;

    /// 构造真实通道并把双端移入载荷盒。
    fn channel_payload() -> BroadcastChannelInner {
        let (tx, rx) = message_queue::channel::<MessageValue>();
        BroadcastChannelInner {
            name: "test".to_owned(),
            tx,
            rx,
            closed: false,
        }
    }

    /// 手工构造带载荷盒的 BroadcastChannel 对象（不经 Vm，只验 GC 两自由函数）。
    fn channel_object_with_payload(inner: BroadcastChannelInner) -> JsObject {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        obj.type_tag = JsObject::OBJ_TYPE_BROADCAST_CHANNEL;
        let payload_ptr = Box::into_raw(Box::new(inner));
        // SAFETY: 载荷盒形态与 native_fn 槽存储约定一致，测试结束前恰好释放一次。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        obj
    }

    #[test]
    fn size_and_drop_idempotent() {
        let mut obj = channel_object_with_payload(channel_payload());
        let struct_bytes = mem::size_of::<BroadcastChannelInner>() as u64;
        assert_eq!(broadcast_channel_native_size(&obj), struct_bytes);
        // 首次 drop 释放结构体；槽置空后二次 drop 零释放。
        assert_eq!(drop_broadcast_channel_native(&mut obj), struct_bytes);
        assert_eq!(drop_broadcast_channel_native(&mut obj), 0);
    }

    #[test]
    fn channel_roundtrip_through_box() {
        let mut obj = channel_object_with_payload(channel_payload());
        let ptr = broadcast_channel_payload_ptr(&obj).unwrap();
        // SAFETY: 载荷盒测试期间存活。
        unsafe {
            assert!(!(*ptr).closed);
            (*ptr).tx.send(MessageValue::Number(1.0)).unwrap();
            assert!(matches!((*ptr).rx.try_recv().unwrap(), MessageValue::Number(n) if n == 1.0));
        }
        drop_broadcast_channel_native(&mut obj);
    }

    #[test]
    fn payload_ptr_brand_guard() {
        // 非 BroadcastChannel 类型标签（PLAIN）对象取载荷指针恒 None。
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::undefined());
        let payload_ptr = Box::into_raw(Box::new(channel_payload()));
        // SAFETY: 同 channel_object_with_payload 的载荷盒形态。
        obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(payload_ptr as *const ()) }));
        assert!(broadcast_channel_payload_ptr(&obj).is_none());
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
                var ch = new BroadcastChannel("test");
                return (
                    typeof ch === 'object' &&
                    BroadcastChannel.length === 1 &&
                    BroadcastChannel.name === 'BroadcastChannel' &&
                    ch.name === 'test' &&
                    Object.getPrototypeOf(ch).constructor === BroadcastChannel
                );
            })()
            "#,
        );
    }

    #[test]
    fn channel_type_tag_is_broadcast_channel() {
        let (_vm, result) = eval("new BroadcastChannel('test')").unwrap();
        assert!(result.is_object());
        // SAFETY: result 是脚本返回的通道对象，测试期间存活。
        let obj = unsafe { &*result.as_js_object_ptr() };
        assert_eq!(obj.type_tag, JsObject::OBJ_TYPE_BROADCAST_CHANNEL);
    }

    #[test]
    fn channel_onmessage_is_own_non_enumerable() {
        assert_script(
            r#"
            (function() {
                var ch = new BroadcastChannel("test");
                var desc = Object.getOwnPropertyDescriptor(ch, 'onmessage');
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
    fn basic_broadcast_and_no_self_delivery() {
        // ch2 收到同构对象，ch1 自身不收到（发送方不投递自己）。
        assert_script(
            r#"
            (function() {
                var ch1 = new BroadcastChannel("basic");
                var ch2 = new BroadcastChannel("basic");
                var received2 = null;
                var received1 = null;
                ch1.onmessage = function(d) { received1 = d; };
                ch2.onmessage = function(d) { received2 = d; };
                ch1.postMessage({a: 1});
                return received2 !== null && received2.a === 1 && received1 === null;
            })()
            "#,
        );
    }

    #[test]
    fn roundtrip_all_cloneable_types() {
        assert_script(
            r#"
            (function() {
                var ch1 = new BroadcastChannel("roundtrip");
                var ch2 = new BroadcastChannel("roundtrip");
                var got = null;
                ch2.onmessage = function(d) { got = d; };
                ch1.postMessage({
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
                    err: new Error('boom'),
                    ab: new ArrayBuffer(4)
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
                if (got.ab.byteLength !== 4) return false;
                return true;
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
                var ch1 = new BroadcastChannel("clone");
                var ch2 = new BroadcastChannel("clone");
                var ab = new ArrayBuffer(4);
                var received = null;
                ch2.onmessage = function(d) { received = d; };
                ch1.postMessage(ab);
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
                var ch1 = new BroadcastChannel("clone");
                var ch2 = new BroadcastChannel("clone");
                try {
                    ch1.postMessage(function(){});
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
                var ch1 = new BroadcastChannel("clone");
                var ch2 = new BroadcastChannel("clone");
                var a = {};
                a.self = a;
                try {
                    ch1.postMessage(a);
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
                var ch1 = new BroadcastChannel("clone");
                var ch2 = new BroadcastChannel("clone");
                try {
                    ch1.postMessage(new SharedArrayBuffer(8));
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
                var ch1 = new BroadcastChannel("close");
                var ch2 = new BroadcastChannel("close");
                if (ch1.readyState !== 'open') return false;
                ch1.close();
                if (ch1.readyState !== 'closed') return false;
                // 重复关闭幂等 no-op。
                ch1.close();
                try {
                    ch1.postMessage(1);
                    return false;
                } catch (e) {
                    if (e.name !== 'TypeError') return false;
                }
                // close 后 ch1 不再收到广播（ch2.postMessage 不触发 ch1.onmessage）。
                var received = false;
                ch1.onmessage = function(d) { received = true; };
                ch2.postMessage(1);
                return received === false;
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
                var ch1 = new BroadcastChannel("nohandler");
                var ch2 = new BroadcastChannel("nohandler");
                ch1.postMessage({a: 1});
                ch1.postMessage('x');
                return ch1.readyState === 'open' && ch2.readyState === 'open';
            })()
            "#,
        );
    }

    #[test]
    fn multi_channel_broadcast() {
        // 三通道同名，ch1.postMessage 触发 ch2 与 ch3 的 onmessage（不触发 ch1）。
        assert_script(
            r#"
            (function() {
                var ch1 = new BroadcastChannel("multi");
                var ch2 = new BroadcastChannel("multi");
                var ch3 = new BroadcastChannel("multi");
                var r1 = null;
                var r2 = null;
                var r3 = null;
                ch1.onmessage = function(d) { r1 = d; };
                ch2.onmessage = function(d) { r2 = d; };
                ch3.onmessage = function(d) { r3 = d; };
                ch1.postMessage({a: 1});
                return r2 !== null && r2.a === 1 && r3 !== null && r3.a === 1 && r1 === null;
            })()
            "#,
        );
    }

    #[test]
    fn different_name_isolation() {
        // 异名通道互不广播。
        assert_script(
            r#"
            (function() {
                var ch1 = new BroadcastChannel("a");
                var ch2 = new BroadcastChannel("b");
                var received = false;
                ch2.onmessage = function(d) { received = true; };
                ch1.postMessage(1);
                return received === false;
            })()
            "#,
        );
    }

    #[test]
    fn gc_prunes_dead_channel_from_registry() {
        // ch2 释放引用（IIFE 局部）加 $262.gc() 强制收集，断言 ch2 被回收
        // （注册表剪枝生效）；ch1 仍存活（用户持有引用）；再 postMessage 不抛错
        // （注册表已无 ch2，广播到空集）。
        assert_script(
            r#"
            (function() {
                var ch1 = new BroadcastChannel("gc");
                var received = false;
                (function() {
                    var ch2 = new BroadcastChannel("gc");
                    ch2.onmessage = function(d) { received = true; };
                })();
                $262.gc();
                ch1.postMessage(2);
                return received === false;
            })()
            "#,
        );
    }

    #[test]
    fn reentry_nested_broadcast_no_deadlock() {
        // handler 内再 postMessage 同通道，嵌套广播不死锁（try_recv 非阻塞）。
        assert_script(
            r#"
            (function() {
                var ch1 = new BroadcastChannel("reentry");
                var ch2 = new BroadcastChannel("reentry");
                var nested = false;
                ch2.onmessage = function(d) {
                    ch2.postMessage("nested");
                    nested = true;
                };
                ch1.postMessage(1);
                return nested === true;
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
                    BroadcastChannel("x");
                    return false;
                } catch (e) {
                    return e.name === 'TypeError';
                }
            })()
            "#,
        );
    }

    #[test]
    fn name_coercion_domstring() {
        // DOMString 强转：非字符串经 ToPrimitive(string hint)，不静默降级。
        assert_script(
            r#"
            (function() {
                var ch = new BroadcastChannel(123);
                if (ch.name !== '123') return false;
                var ch2 = new BroadcastChannel();
                return ch2.name === 'undefined';
            })()
            "#,
        );
    }

    #[test]
    fn non_channel_receiver_type_error() {
        // 对非对象 this 直调 native 入口抛 TypeError。
        let (mut vm, _result) = eval("new BroadcastChannel('x')").unwrap();
        vm.set_reg(0, JsValue::int(1));
        vm.set_reg(1, JsValue::int(42));
        let args = [0u8, 1];
        match broadcast_channel_post_message(&mut vm, &args) {
            NativeResult::Err(e) => {
                assert!(e.is_object());
            }
            _ => panic!("expected TypeError"),
        }
    }

    #[test]
    fn non_channel_object_type_error() {
        // 对象但非 BroadcastChannel（plain 对象）调 postMessage 抛 TypeError。
        let (mut vm, plain) = eval("({})").unwrap();
        vm.set_reg(0, plain);
        vm.set_reg(1, JsValue::int(42));
        let args = [0u8, 1];
        match broadcast_channel_post_message(&mut vm, &args) {
            NativeResult::Err(e) => {
                assert!(e.is_object());
            }
            _ => panic!("expected TypeError"),
        }
    }
}
