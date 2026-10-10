//! Event 基类与 MessageEvent 派生类行为面：`Event` 构造器（`new Event(type,
//! init)`）、原型 getter（type / target / currentTarget / eventPhase /
//! isTrusted / bubbles / cancelable / defaultPrevented / composed）与原型方法
//! （preventDefault / stopPropagation / stopImmediatePropagation /
//! composedPath）；`MessageEvent` 构造器（`new MessageEvent(type, init)`）与
//! 原型 getter（data / origin / lastEpoch / source / ports）。
//!
//! 事件载荷盒（type 串与九项状态位，MessageEvent 加 data / origin /
//! lastEpoch / source / ports 五字段）存于对象 `native_fn` 槽，向 GC 家族表
//! 暴露 edges/size/drop 三自由函数。

use std::mem;

use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::object::{JsObject, NativeFnPtr};
use oxide_types::value::JsValue;

use oxide_runtime_api::{NativeResult, ProtoKind, VmHost};

/// Event 载荷盒：存于对象 `native_fn` 槽（`Box::into_raw`，realm 局部单线程）。
/// `r#type` 是事件类型串（GC 字符串边），`target` / `current_target` 是目标
/// 对象（GC 对象边，初始 null），其余为简单状态位。
///
/// `#[repr(C)]` 固定字段按声明序、首字段偏移 0：派生类盒以本结构为首字段嵌入，
/// 派发路径把盒指针转基类指针读首字段，依赖该偏移稳定。
#[repr(C)]
struct EventInner {
    r#type: JsValue,
    target: JsValue,
    current_target: JsValue,
    event_phase: u32,
    is_trusted: bool,
    bubbles: bool,
    cancelable: bool,
    default_prevented: bool,
    propagation_stopped: bool,
    immediate_stopped: bool,
    composed: bool,
}

/// MessageEvent 载荷盒：事件基类字段（`EventInner` 嵌入，位于偏移 0）加消息
/// 五字段（data / origin / lastEpoch / source / ports）。存于对象 `native_fn`
/// 槽（`Box::into_raw`，realm 局部单线程）。
///
/// `#[repr(C)]` 固定 `base` 位于偏移 0：派生类盒指针转基类指针读首字段依赖该
/// 偏移稳定（与 `EventInner` 同型约束）。
#[repr(C)]
struct MessageEventInner {
    base: EventInner,
    data: JsValue,
    origin: JsValue,
    last_epoch: u32,
    source: JsValue,
    ports: Vec<JsValue>,
}

/// 是否事件系对象（Event / MessageEvent / ErrorEvent / CustomEvent 四标签）。
pub fn is_event_family_obj(obj: &JsObject) -> bool {
    matches!(
        obj.type_tag,
        JsObject::OBJ_TYPE_EVENT
            | JsObject::OBJ_TYPE_MESSAGE_EVENT
            | JsObject::OBJ_TYPE_ERROR_EVENT
            | JsObject::OBJ_TYPE_CUSTOM_EVENT
    )
}

/// 取载荷盒指针：仅 Event 标签对象返回 `Some`，其余标签返回 `None`。
fn event_payload_ptr(obj: &JsObject) -> Option<*mut EventInner> {
    if !obj.is_event_obj() {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut EventInner)
}

/// 取事件系基类字段指针：仅事件系标签对象返回 `Some`，其余标签返回 `None`。
///
/// 派生类对象的 `native_fn` 槽存 `Box<派生类Inner>`，其首字段是 `EventInner`
/// （基类字段位于偏移 0），指针可安全转基类字段指针。
fn event_family_base_ptr(obj: &JsObject) -> Option<*mut EventInner> {
    if !is_event_family_obj(obj) {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut EventInner)
}

/// 收集 Event 的引用边（type 串边与 target / current_target 对象边）。
///
/// `target` / `current_target` 空窗口（构造期回填之前）为 null，边函数自守，
/// 语义自含。
pub fn event_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(ptr) = event_payload_ptr(obj) else {
        return Vec::new();
    };
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: ptr 指向有效载荷盒（槽位仅构造期写入、释放时置空），只读字段。
    let inner = unsafe { &*ptr };
    vec![inner.r#type, inner.target, inner.current_target]
}

/// 只读核算 Event 载荷盒字节（不释放）。
pub fn event_native_size(obj: &JsObject) -> u64 {
    let Some(ptr) = event_payload_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    mem::size_of::<EventInner>() as u64
}

/// 释放 Event 载荷盒（`Box<EventInner>`），返回释放字节数。
///
/// 盒 drop 时 type / target / current_target 的 JsValue 边随盒释放（GC 已
/// 判定死对象）；槽置空保证重复调用零释放（幂等）。
pub fn drop_event_native(obj: &mut JsObject) -> u64 {
    let bytes = event_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = event_payload_ptr(obj) else {
        return 0;
    };
    // SAFETY: ptr 非空（event_native_size 已验证），Box::from_raw 恰好释放一次。
    unsafe {
        drop(Box::from_raw(ptr));
    }
    obj.set_native_fn(None);
    bytes
}

/// 读事件类型串（GC 字符串边）。非事件系对象或无载荷盒返回 None。
///
/// 供派发路径（`dispatchEvent`）取事件类型串、intern 后查监听器。
pub fn event_type_value(obj: &JsObject) -> Option<JsValue> {
    let ptr = event_family_base_ptr(obj)?;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: ptr 指向有效载荷盒（构造期写入、释放时置空），只读字段。
    Some(unsafe { &*ptr }.r#type)
}

/// 设派发目标与阶段：`target` / `current_target` 置 `target`、`event_phase` 置
/// `phase`。非事件系对象或无载荷盒时 no-op。
///
/// 供派发路径（`dispatchEvent`）在调用监听器前设目标与 AT_TARGET 阶段。
pub fn event_set_dispatch(obj: &mut JsObject, target: JsValue, phase: u32) {
    let Some(ptr) = event_family_base_ptr(obj) else {
        return;
    };
    if ptr.is_null() {
        return;
    }
    // SAFETY: ptr 指向有效载荷盒，写入派发字段（无 GC 窗口）。
    let inner = unsafe { &mut *ptr };
    inner.target = target;
    inner.current_target = target;
    inner.event_phase = phase;
}

/// 读 `defaultPrevented` 标志。非事件系对象或无载荷盒返回 None。
pub fn event_default_prevented(obj: &JsObject) -> Option<bool> {
    let ptr = event_family_base_ptr(obj)?;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: ptr 指向有效载荷盒，只读字段。
    Some(unsafe { &*ptr }.default_prevented)
}

/// 读 `stopImmediatePropagation` 标志。非事件系对象或无载荷盒返回 None。
pub fn event_immediate_stopped(obj: &JsObject) -> Option<bool> {
    let ptr = event_family_base_ptr(obj)?;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: ptr 指向有效载荷盒，只读字段。
    Some(unsafe { &*ptr }.immediate_stopped)
}

/// 解析 this 为事件系基类字段共享引用：非事件系对象或无载荷盒返回 None。
///
/// # 注意事项
/// 返回引用按 `'a` 声称为存活，实际存活至载荷盒被 GC 释放；调用方须在无 GC
/// 窗口的单 native 函数内消费，不得跨用户调用窗口持有。
fn event_inner_of<'a>(this_val: JsValue) -> Option<&'a EventInner> {
    if !this_val.is_object() {
        return None;
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return None;
    }
    // SAFETY: this_val 已确认是对象值，obj_ptr 指向合法 JsObject。
    let obj = unsafe { &*obj_ptr };
    let box_ptr = event_family_base_ptr(obj)?;
    if box_ptr.is_null() {
        return None;
    }
    // SAFETY: box_ptr 指向有效载荷盒（构造期写入、释放时置空）；'a 存活由
    // 调用方「无 GC 窗口内消费」约定保证。
    Some(unsafe { &*box_ptr })
}

/// 解析 this 为事件系基类字段可变引用：非事件系对象或无载荷盒返回 None。
///
/// # 注意事项
/// 返回引用按 `'a` 声称为存活，实际存活至载荷盒被 GC 释放；调用方须在无 GC
/// 窗口的单 native 函数内消费，不得跨用户调用窗口持有。
fn event_inner_of_mut<'a>(this_val: JsValue) -> Option<&'a mut EventInner> {
    if !this_val.is_object() {
        return None;
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return None;
    }
    // SAFETY: this_val 已确认是对象值，obj_ptr 指向合法 JsObject。
    let obj = unsafe { &mut *obj_ptr };
    let box_ptr = event_family_base_ptr(obj)?;
    if box_ptr.is_null() {
        return None;
    }
    // SAFETY: box_ptr 指向有效载荷盒（构造期写入、释放时置空）；'a 存活由
    // 调用方「无 GC 窗口内消费」约定保证。
    Some(unsafe { &mut *box_ptr })
}

/// Event 构造器：`new Event(type, init)`。
///
/// # 步骤
/// 1. 非构造形态（`!constructing_native`）抛 TypeError（HTML 规范只有
///    [[Construct]]，无 [[Call]]）
/// 2. type 经完整 ToString 转串；空串抛 SyntaxError
/// 3. init 字典读 bubbles / cancelable / composed（缺省 false）
/// 4. 建载荷盒入 receiver 的 `native_fn` 槽，置 type_tag
/// 5. 返回 receiver
///
/// # 边界与前提
/// - receiver（`args[0]`）是 VM 按构造器 prototype 新分配的对象，proto 已
///   指向 %Event.prototype%；
/// - init 缺省（undefined）或非对象时对应标志取 false。
///
/// # 副作用
/// - receiver 的 `native_fn` 槽写入载荷盒、`type_tag` 置 Event 标签。
pub fn event_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if !vm.constructing_native() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Event constructor requires new"));
    }

    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "Event constructor requires new"));
    }

    // type 经完整 ToString 转串（对象经 ToPrimitive，Symbol 抛 TypeError）。
    let type_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let type_str = match oxide_runtime_api::to_string_full(type_val, vm) {
        Ok(s) => s,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    if type_str.is_empty() {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "The provided type is empty"));
    }

    // init 字典读 bubbles / cancelable / composed（缺省或非对象时取 false）。
    let mut bubbles = false;
    let mut cancelable = false;
    let mut composed = false;
    if args.len() > 2 {
        let init_val = vm.reg(args[2]);
        if init_val.is_object() {
            let init_ptr = init_val.as_js_object_ptr();
            // SAFETY: init_val 已确认是对象值，借用即时消费。
            let init_obj = unsafe { &*init_ptr };
            for (name, slot) in
                [("bubbles", &mut bubbles), ("cancelable", &mut cancelable), ("composed", &mut composed)]
            {
                let si = vm.perm_intern(name);
                match vm.ordinary_get(init_obj, si, init_val) {
                    Ok(v) => *slot = oxide_runtime_api::to_boolean(v),
                    Err(_) => {
                        if let Some(exc) = vm.take_uncaught_value() {
                            return NativeResult::Err(exc);
                        }
                        return NativeResult::Err(crate::error::create_type_error(
                            vm,
                            "Event init property getter failed",
                        ));
                    }
                }
            }
        }
    }

    // 建载荷盒入 receiver 的 native_fn 槽，置 Event 标签。
    let type_js = vm.new_string(&type_str);
    let inner = Box::into_raw(Box::new(EventInner {
        r#type: type_js,
        target: JsValue::null(),
        current_target: JsValue::null(),
        event_phase: 0,
        is_trusted: false,
        bubbles,
        cancelable,
        default_prevented: false,
        propagation_stopped: false,
        immediate_stopped: false,
        composed,
    }));
    // SAFETY: receiver 不可调用，native_fn 存不透明 `Box<EventInner>` 指针，
    // 与 ArrayBuffer / RegExp 的类型化存储模式一致。
    let obj = unsafe { &mut *this_val.as_js_object_ptr() };
    obj.type_tag = JsObject::OBJ_TYPE_EVENT;
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(inner as *const ()) }));
    NativeResult::Ok(this_val)
}

/// 建一个 Event 载荷盒对象（type 为 `type_str`），返回对象指针。
///
/// 供交付路径（消息事件 / 错误事件）建真实 Event 盒，使 `dispatchEvent` 品牌守卫
/// 通过。盒的 `target` / `currentTarget` 初始 null、`eventPhase` 初始 0（派发时设）；
/// `[[Prototype]]` 指向 `Event.prototype`（`e instanceof Event` 成立）。
///
/// # 副作用
/// - 新建一个 Event 盒对象（经 `alloc_object` 入对象表）。
pub fn create_event_box<H: VmHost>(vm: &mut H, type_str: &str) -> *mut JsObject {
    let type_js = vm.new_string(type_str);
    let inner = Box::into_raw(Box::new(EventInner {
        r#type: type_js,
        target: JsValue::null(),
        current_target: JsValue::null(),
        event_phase: 0,
        is_trusted: false,
        bubbles: false,
        cancelable: false,
        default_prevented: false,
        propagation_stopped: false,
        immediate_stopped: false,
        composed: false,
    }));
    let event_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::EventProto));
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, event_proto);
    let ptr = vm.alloc_object(obj);
    // SAFETY: ptr 是本函数刚 alloc_object 的对象，存活且本段无别名；native_fn 存
    // 不透明 `Box<EventInner>` 指针，与 ArrayBuffer / RegExp 的类型化存储模式一致。
    let event_obj = unsafe { &mut *ptr };
    event_obj.type_tag = JsObject::OBJ_TYPE_EVENT;
    event_obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(inner as *const ()) }));
    ptr
}

/// `Event.prototype.type`：返回事件类型串。
pub fn event_type_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.type getter called on non-Event object",
        ));
    };
    NativeResult::Ok(inner.r#type)
}

/// `Event.prototype.target`：返回事件目标对象（未派发时 null）。
pub fn event_target_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.target getter called on non-Event object",
        ));
    };
    NativeResult::Ok(inner.target)
}

/// `Event.prototype.currentTarget`：返回当前派发目标（未派发时 null）。
pub fn event_current_target_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.currentTarget getter called on non-Event object",
        ));
    };
    NativeResult::Ok(inner.current_target)
}

/// `Event.prototype.eventPhase`：返回派发阶段（0 未派发 / 1 捕获 / 2 目标 / 3 冒泡）。
pub fn event_phase_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.eventPhase getter called on non-Event object",
        ));
    };
    NativeResult::Ok(JsValue::int(inner.event_phase as i32))
}

/// `Event.prototype.isTrusted`：返回是否由用户操作产生（引擎恒 false）。
pub fn event_is_trusted_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.isTrusted getter called on non-Event object",
        ));
    };
    NativeResult::Ok(JsValue::bool(inner.is_trusted))
}

/// `Event.prototype.bubbles`：返回是否冒泡。
pub fn event_bubbles_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.bubbles getter called on non-Event object",
        ));
    };
    NativeResult::Ok(JsValue::bool(inner.bubbles))
}

/// `Event.prototype.cancelable`：返回是否可取消。
pub fn event_cancelable_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.cancelable getter called on non-Event object",
        ));
    };
    NativeResult::Ok(JsValue::bool(inner.cancelable))
}

/// `Event.prototype.defaultPrevented`：返回默认行为是否已被阻止。
pub fn event_default_prevented_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.defaultPrevented getter called on non-Event object",
        ));
    };
    NativeResult::Ok(JsValue::bool(inner.default_prevented))
}

/// `Event.prototype.composed`：返回是否跨 Shadow DOM 边界。
pub fn event_composed_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.composed getter called on non-Event object",
        ));
    };
    NativeResult::Ok(JsValue::bool(inner.composed))
}

/// `Event.prototype.preventDefault`：cancelable 时置 defaultPrevented。
pub fn event_prevent_default<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of_mut(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.preventDefault called on non-Event object",
        ));
    };
    if inner.cancelable {
        inner.default_prevented = true;
    }
    NativeResult::Ok(JsValue::undefined())
}

/// `Event.prototype.stopPropagation`：置传播停止标志。
pub fn event_stop_propagation<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of_mut(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.stopPropagation called on non-Event object",
        ));
    };
    inner.propagation_stopped = true;
    NativeResult::Ok(JsValue::undefined())
}

/// `Event.prototype.stopImmediatePropagation`：置传播停止与立即停止双标志。
pub fn event_stop_immediate_propagation<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = event_inner_of_mut(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.stopImmediatePropagation called on non-Event object",
        ));
    };
    inner.propagation_stopped = true;
    inner.immediate_stopped = true;
    NativeResult::Ok(JsValue::undefined())
}

/// `Event.prototype.composedPath`：返回事件传播路径（首版仅含 this 自身）。
pub fn event_composed_path<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(_) = event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "Event.prototype.composedPath called on non-Event object",
        ));
    };
    // 首版：路径仅含 this 自身（完整传播路径由事件派发任务补全）。
    let array_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::ArrayProto));
    let arr = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, array_proto, 1));
    // SAFETY: arr 是本函数刚分配的对象，存活且本段无别名。
    unsafe {
        (*arr).set_prop_at(0, this_val);
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}

// —— MessageEvent 派生类 ——

/// 取 MessageEvent 载荷盒指针：仅 MessageEvent 标签对象返回 `Some`，其余标签
/// 返回 `None`。
fn message_event_payload_ptr(obj: &JsObject) -> Option<*mut MessageEventInner> {
    if obj.type_tag != JsObject::OBJ_TYPE_MESSAGE_EVENT {
        return None;
    }
    obj.native_fn().map(|ptr| ptr.as_ptr() as *mut MessageEventInner)
}

/// 收集 MessageEvent 的引用边（基类 type / target / current_target 边加
/// data / source / ports 边）。
///
/// `target` / `current_target` 空窗口（构造期回填之前）为 null，边函数自守，
/// 语义自含。
pub fn message_event_native_edges(obj: &JsObject) -> Vec<JsValue> {
    let Some(ptr) = message_event_payload_ptr(obj) else {
        return Vec::new();
    };
    if ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: ptr 指向有效载荷盒（槽位仅构造期写入、释放时置空），只读字段。
    let inner = unsafe { &*ptr };
    let mut edges = vec![inner.base.r#type, inner.base.target, inner.base.current_target];
    edges.push(inner.data);
    edges.push(inner.source);
    edges.extend(inner.ports.iter().copied());
    edges
}

/// 只读核算 MessageEvent 载荷盒字节（不释放）。
pub fn message_event_native_size(obj: &JsObject) -> u64 {
    let Some(ptr) = message_event_payload_ptr(obj) else {
        return 0;
    };
    if ptr.is_null() {
        return 0;
    }
    mem::size_of::<MessageEventInner>() as u64
}

/// 释放 MessageEvent 载荷盒（`Box<MessageEventInner>`），返回释放字节数。
///
/// 盒 drop 时基类字段与 data / origin / source / ports 的 JsValue 边随盒释放
/// （GC 已判定死对象）；槽置空保证重复调用零释放（幂等）。
pub fn drop_message_event_native(obj: &mut JsObject) -> u64 {
    let bytes = message_event_native_size(obj);
    if bytes == 0 {
        return 0;
    }
    let Some(ptr) = message_event_payload_ptr(obj) else {
        return 0;
    };
    // SAFETY: ptr 非空（message_event_native_size 已验证），Box::from_raw 恰好
    // 释放一次。
    unsafe {
        drop(Box::from_raw(ptr));
    }
    obj.set_native_fn(None);
    bytes
}

/// 建一个 MessageEvent 载荷盒对象（`type` 串与 `data` 值），返回对象指针。
///
/// 供交付路径（消息事件）建真实 MessageEvent 盒，使 `dispatchEvent` 品牌守卫
/// 通过。盒的 `origin` 为空串、`lastEpoch` 为 0、`source` 为 null、`ports`
/// 为空数组；`[[Prototype]]` 指向 `MessageEvent.prototype`
/// （`e instanceof MessageEvent` 与 `e instanceof Event` 均成立）。
///
/// # 副作用
/// - 新建一个 MessageEvent 盒对象（经 `alloc_object` 入对象表）。
pub fn create_message_event_box<H: VmHost>(vm: &mut H, data: JsValue, type_str: &str) -> *mut JsObject {
    let type_js = vm.new_string(type_str);
    let origin_js = vm.new_string("");
    let inner = Box::into_raw(Box::new(MessageEventInner {
        base: EventInner {
            r#type: type_js,
            target: JsValue::null(),
            current_target: JsValue::null(),
            event_phase: 0,
            is_trusted: false,
            bubbles: false,
            cancelable: false,
            default_prevented: false,
            propagation_stopped: false,
            immediate_stopped: false,
            composed: false,
        },
        data,
        origin: origin_js,
        last_epoch: 0,
        source: JsValue::null(),
        ports: Vec::new(),
    }));
    let me_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::MessageEventProto));
    let obj = JsObject::new_empty(EMPTY_SHAPE_ID, me_proto);
    let ptr = vm.alloc_object(obj);
    // SAFETY: ptr 是本函数刚 alloc_object 的对象，存活且本段无别名；native_fn 存
    // 不透明 `Box<MessageEventInner>` 指针，与 ArrayBuffer / RegExp 的类型化
    // 存储模式一致。
    let me_obj = unsafe { &mut *ptr };
    me_obj.type_tag = JsObject::OBJ_TYPE_MESSAGE_EVENT;
    me_obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(inner as *const ()) }));
    ptr
}

/// MessageEvent 构造器：`new MessageEvent(type, init)`。
///
/// # 步骤
/// 1. 非构造形态（`!constructing_native`）抛 TypeError（HTML 规范只有
///    [[Construct]]，无 [[Call]]）
/// 2. type 经完整 ToString 转串；空串抛 SyntaxError
/// 3. init 字典读 data / origin / lastEpoch / source / ports（缺省：data 为
///    undefined、origin 为空串、lastEpoch 为 0、source 为 null、ports 为空数组）
/// 4. 建载荷盒入 receiver 的 `native_fn` 槽，置 type_tag
/// 5. 返回 receiver
///
/// # 边界与前提
/// - receiver（`args[0]`）是 VM 按构造器 prototype 新分配的对象，proto 已
///   指向 %MessageEvent.prototype%；
/// - init 缺省（undefined）或非对象时对应字段取缺省。
///
/// # 副作用
/// - receiver 的 `native_fn` 槽写入载荷盒、`type_tag` 置 MessageEvent 标签。
pub fn message_event_constructor<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    if !vm.constructing_native() {
        return NativeResult::Err(crate::error::create_type_error(vm, "MessageEvent constructor requires new"));
    }

    let this_val = vm.reg(args[0]);
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "MessageEvent constructor requires new"));
    }

    // type 经完整 ToString 转串（对象经 ToPrimitive，Symbol 抛 TypeError）。
    let type_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let type_str = match oxide_runtime_api::to_string_full(type_val, vm) {
        Ok(s) => s,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert value to a string"));
        }
    };
    if type_str.is_empty() {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "The provided type is empty"));
    }

    // init 字典读 data / origin / lastEpoch / source / ports（缺省或非对象
    // 时取缺省）。
    let mut data = JsValue::undefined();
    let mut origin = JsValue::undefined();
    let mut last_epoch = 0u32;
    let mut source = JsValue::null();
    let mut ports: Vec<JsValue> = Vec::new();
    if args.len() > 2 {
        let init_val = vm.reg(args[2]);
        if init_val.is_object() {
            let init_ptr = init_val.as_js_object_ptr();
            // SAFETY: init_val 已确认是对象值，借用即时消费。
            let init_obj = unsafe { &*init_ptr };

            // data / origin / source 直读属性（getter 副作用与异常原值传播）。
            for (name, slot) in [("data", &mut data), ("origin", &mut origin), ("source", &mut source)] {
                let si = vm.perm_intern(name);
                match vm.ordinary_get(init_obj, si, init_val) {
                    Ok(v) => *slot = v,
                    Err(_) => {
                        if let Some(exc) = vm.take_uncaught_value() {
                            return NativeResult::Err(exc);
                        }
                        return NativeResult::Err(crate::error::create_type_error(
                            vm,
                            "MessageEvent init property getter failed",
                        ));
                    }
                }
            }

            // lastEpoch 读数值（非有限非负数归 0）。
            let si_epoch = vm.perm_intern("lastEpoch");
            match vm.ordinary_get(init_obj, si_epoch, init_val) {
                Ok(v) => {
                    let n = oxide_runtime_api::to_number(v);
                    last_epoch = if n.is_finite() && n >= 0.0 { n as u32 } else { 0 };
                }
                Err(_) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(
                        vm,
                        "MessageEvent init property getter failed",
                    ));
                }
            }

            // ports 读数组属性并复制元素（非数组取空数组）。
            let si_ports = vm.perm_intern("ports");
            match vm.ordinary_get(init_obj, si_ports, init_val) {
                Ok(v) => {
                    if v.is_object() {
                        let ports_ptr = v.as_js_object_ptr();
                        // SAFETY: v 已确认是对象值，借用即时消费。
                        let ports_obj = unsafe { &*ports_ptr };
                        if ports_obj.is_array() {
                            if let Some(elements) = ports_obj.array_elements_vec() {
                                ports = elements.to_vec();
                            }
                        }
                    }
                }
                Err(_) => {
                    if let Some(exc) = vm.take_uncaught_value() {
                        return NativeResult::Err(exc);
                    }
                    return NativeResult::Err(crate::error::create_type_error(
                        vm,
                        "MessageEvent init property getter failed",
                    ));
                }
            }
        }
    }

    // origin 缺省为空串。
    if origin.is_undefined() {
        origin = vm.new_string("");
    }

    // 建载荷盒入 receiver 的 native_fn 槽，置 MessageEvent 标签。
    let type_js = vm.new_string(&type_str);
    let inner = Box::into_raw(Box::new(MessageEventInner {
        base: EventInner {
            r#type: type_js,
            target: JsValue::null(),
            current_target: JsValue::null(),
            event_phase: 0,
            is_trusted: false,
            bubbles: false,
            cancelable: false,
            default_prevented: false,
            propagation_stopped: false,
            immediate_stopped: false,
            composed: false,
        },
        data,
        origin,
        last_epoch,
        source,
        ports,
    }));
    // SAFETY: receiver 不可调用，native_fn 存不透明 `Box<MessageEventInner>`
    // 指针，与 ArrayBuffer / RegExp 的类型化存储模式一致。
    let obj = unsafe { &mut *this_val.as_js_object_ptr() };
    obj.type_tag = JsObject::OBJ_TYPE_MESSAGE_EVENT;
    obj.set_native_fn(Some(unsafe { NativeFnPtr::from_raw(inner as *const ()) }));
    NativeResult::Ok(this_val)
}

/// 解析 this 为 MessageEvent 载荷盒共享引用：非 MessageEvent 对象或无载荷盒
/// 返回 None。
///
/// # 注意事项
/// 返回引用按 `'a` 声称为存活，实际存活至载荷盒被 GC 释放；调用方须在无 GC
/// 窗口的单 native 函数内消费，不得跨用户调用窗口持有。
fn message_event_inner_of<'a>(this_val: JsValue) -> Option<&'a MessageEventInner> {
    if !this_val.is_object() {
        return None;
    }
    let obj_ptr = this_val.as_js_object_ptr();
    if obj_ptr.is_null() {
        return None;
    }
    // SAFETY: this_val 已确认是对象值，obj_ptr 指向合法 JsObject。
    let obj = unsafe { &*obj_ptr };
    let box_ptr = message_event_payload_ptr(obj)?;
    if box_ptr.is_null() {
        return None;
    }
    // SAFETY: box_ptr 指向有效载荷盒（构造期写入、释放时置空）；'a 存活由
    // 调用方「无 GC 窗口内消费」约定保证。
    Some(unsafe { &*box_ptr })
}

/// `MessageEvent.prototype.data`：返回消息数据。
pub fn message_event_data_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = message_event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "MessageEvent.prototype.data getter called on non-MessageEvent object",
        ));
    };
    NativeResult::Ok(inner.data)
}

/// `MessageEvent.prototype.origin`：返回消息来源源（缺省为空串）。
pub fn message_event_origin_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = message_event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "MessageEvent.prototype.origin getter called on non-MessageEvent object",
        ));
    };
    NativeResult::Ok(inner.origin)
}

/// `MessageEvent.prototype.lastEpoch`：返回最后纪元（引擎恒 0）。
pub fn message_event_last_epoch_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = message_event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "MessageEvent.prototype.lastEpoch getter called on non-MessageEvent object",
        ));
    };
    NativeResult::Ok(JsValue::int(inner.last_epoch as i32))
}

/// `MessageEvent.prototype.source`：返回消息来源对象（首版恒 null）。
pub fn message_event_source_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = message_event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "MessageEvent.prototype.source getter called on non-MessageEvent object",
        ));
    };
    NativeResult::Ok(inner.source)
}

/// `MessageEvent.prototype.ports`：返回消息端口数组（首版恒空数组）。
pub fn message_event_ports_getter<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    let Some(inner) = message_event_inner_of(this_val) else {
        return NativeResult::Err(crate::error::create_type_error(
            vm,
            "MessageEvent.prototype.ports getter called on non-MessageEvent object",
        ));
    };
    let array_proto = JsValue::from_js_object(vm.builtin_proto(ProtoKind::ArrayProto));
    let arr = vm.alloc_object(JsObject::new_array(EMPTY_SHAPE_ID, array_proto, inner.ports.len()));
    // SAFETY: arr 是本函数刚分配的对象，存活且本段无别名。
    unsafe {
        for (i, value) in inner.ports.iter().enumerate() {
            (*arr).set_prop_at(i, *value);
        }
    }
    NativeResult::Ok(JsValue::from_js_object(arr))
}
