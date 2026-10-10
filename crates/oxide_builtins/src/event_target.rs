//! EventTarget 行为面：`addEventListener` / `removeEventListener` / `dispatchEvent`
//! 三原型方法。
//!
//! 监听器注册表是 per-realm 弱引用结构（目标对象裸指针为键、不保活），由 `VmHost`
//! 的 `et_register` / `et_unregister` / `et_lookup` 三方法操作，GC sweep 按 mark 位
//! 剪枝、full_reset 清表。注册表值（`EventTargetState`）持监听器回调的 GC 对象边，
//! mark 期经根枚举标活（存活目标的回调随目标存活）。`ListenerEntry` /
//! `EventTargetState` 类型落 `oxide_runtime_api`（trait 所在层，本 crate 依赖之）。

use oxide_runtime_api::{ListenerEntry, NativeResult, VmHost};
use oxide_types::value::JsValue;

use crate::event;

/// 解析 `addEventListener` / `removeEventListener` 的 options 参数：布尔（capture）
/// 或对象（once / capture，passive / signal 忽略）。返回 (capture, once)。
///
/// # 边界与前提
/// - options 对象属性经 `ordinary_get` 读（getter 副作用与异常原值传播）。
/// - 缺失或非对象时取缺省（capture=false, once=false）。
fn parse_event_options<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<(bool, bool), JsValue> {
    let opt_val = if args.len() > 3 { vm.reg(args[3]) } else { JsValue::undefined() };
    if opt_val.is_bool() {
        return Ok((oxide_runtime_api::to_boolean(opt_val), false));
    }
    if !opt_val.is_object() {
        return Ok((false, false));
    }
    let opt_ptr = opt_val.as_js_object_ptr();
    // SAFETY: opt_val 已确认是对象值，借用即时消费。
    let opt_obj = unsafe { &*opt_ptr };
    let mut capture = false;
    let mut once = false;
    for (name, slot) in [("capture", &mut capture), ("once", &mut once)] {
        let si = vm.perm_intern(name);
        match vm.ordinary_get(opt_obj, si, opt_val) {
            Ok(v) => *slot = oxide_runtime_api::to_boolean(v),
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return Err(exc);
                }
                return Err(crate::error::create_type_error(vm, "Event options property getter failed"));
            }
        }
    }
    Ok((capture, once))
}

/// `EventTarget.prototype.addEventListener(type, callback, options)`。
///
/// # 步骤
/// 1. this 解析为目标对象（非对象抛 TypeError）。
/// 2. type 经完整 ToString 转串、intern 得 si；空串抛 SyntaxError。
/// 3. callback 须为函数（首版不支持对象形 handleEvent，非函数抛 TypeError）。
/// 4. options 是布尔（capture）或对象（once / capture，passive / signal 忽略）。
/// 5. 重复登记（同 type + callback + capture）为 no-op；否则追加进注册表。
///
/// # 副作用
/// - 写注册表（`et_register`）。
pub fn event_target_add_event_listener<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "addEventListener called on non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();

    // type 经完整 ToString 转串（对象经 ToPrimitive，Symbol 抛 TypeError）。
    let type_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let type_str = match oxide_runtime_api::to_string_full(type_val, vm) {
        Ok(s) => s,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert event type to a string"));
        }
    };
    if type_str.is_empty() {
        return NativeResult::Err(crate::error::create_syntax_error(vm, "The provided type is empty"));
    }
    let type_si = vm.perm_intern(&type_str);

    // callback 须为函数（首版不支持对象形 handleEvent）。
    let callback = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    if !crate::iterator::is_callable(callback) {
        return NativeResult::Err(crate::error::create_type_error(vm, "Event listener callback must be a function"));
    }

    // options：布尔（capture）或对象（once / capture）。
    let (capture, once) = match parse_event_options(vm, args) {
        Ok(co) => co,
        Err(e) => return NativeResult::Err(e),
    };

    // 重复登记（同 type + callback + capture）为 no-op；否则追加。
    let entry = ListenerEntry {
        type_si,
        callback,
        capture,
        once,
        is_attribute: false,
    };
    let existing = vm.et_lookup(this_ptr);
    if existing
        .iter()
        .any(|e| e.type_si == type_si && e.capture == capture && e.callback == callback)
    {
        return NativeResult::Ok(JsValue::undefined());
    }
    vm.et_register(this_ptr, entry);
    NativeResult::Ok(JsValue::undefined())
}

/// `EventTarget.prototype.removeEventListener(type, callback, options)`。
///
/// # 步骤
/// 1. this 解析为目标对象（非对象抛 TypeError）。
/// 2. type 经完整 ToString 转串、intern 得 si。
/// 3. 按同 type + callback + capture 查条目移除（不存在为 no-op）。
///
/// # 副作用
/// - 写注册表（`et_unregister`）。
pub fn event_target_remove_event_listener<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "removeEventListener called on non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();

    // type 经完整 ToString 转串（与 addEventListener 同一 intern 口径）。
    let type_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    let type_str = match oxide_runtime_api::to_string_full(type_val, vm) {
        Ok(s) => s,
        Err(_) => {
            if let Some(exc) = vm.take_uncaught_value() {
                return NativeResult::Err(exc);
            }
            return NativeResult::Err(crate::error::create_type_error(vm, "Cannot convert event type to a string"));
        }
    };
    let type_si = vm.perm_intern(&type_str);

    let callback = if args.len() > 2 { vm.reg(args[2]) } else { JsValue::undefined() };
    let (capture, _) = match parse_event_options(vm, args) {
        Ok(co) => co,
        Err(e) => return NativeResult::Err(e),
    };
    vm.et_unregister(this_ptr, type_si, callback, capture);
    NativeResult::Ok(JsValue::undefined())
}

/// `EventTarget.prototype.dispatchEvent(event)`：同步派发事件，返回
/// `!event.defaultPrevented`。
///
/// # 步骤
/// 1. this 解析为目标对象（非对象抛 TypeError）。
/// 2. event 品牌守卫（须为 Event 系对象），设 target / currentTarget 为 this、
///    eventPhase 为 AT_TARGET（2）。
/// 3. 取事件类型串、intern 得 si，查注册表取本对象该类型的监听器快照。
/// 4. 逐个监听器调用（GC 根纪律：调用前钉事件与回调进寄存器，调用后重读载荷指针）；
///    `once` 条目调用后移除；`stopImmediatePropagation` 截断循环。
/// 5. 返回 `!event.defaultPrevented`。
///
/// # 边界与前提
/// - 首版派发只在目标（无冒泡、无捕获阶段），监听器按登记序调用。
/// - 监听器回调的异常原值上抛（不吞）。
///
/// # 副作用
/// - 写事件载荷盒（target / currentTarget / eventPhase）；移除 `once` 条目。
pub fn event_target_dispatch_event<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let this_val = vm.reg(if args.is_empty() { 0 } else { args[0] });
    if !this_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "dispatchEvent called on non-object"));
    }
    let this_ptr = this_val.as_js_object_ptr();

    // event 品牌守卫：须为 Event 系对象（首版仅 Event 标签，派生类由后续任务接入）。
    let event_val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    if !event_val.is_object() {
        return NativeResult::Err(crate::error::create_type_error(vm, "dispatchEvent requires an Event"));
    }
    let event_ptr = event_val.as_js_object_ptr();
    if event_ptr.is_null() {
        return NativeResult::Err(crate::error::create_type_error(vm, "dispatchEvent requires an Event"));
    }
    // SAFETY: event_val 已确认是对象值，event_ptr 指向合法 JsObject。
    let event_obj = unsafe { &*event_ptr };
    if !event_obj.is_event_obj() {
        return NativeResult::Err(crate::error::create_type_error(vm, "dispatchEvent requires an Event"));
    }

    // 设派发目标与阶段（AT_TARGET = 2）。
    // SAFETY: event_obj 存活，无 GC 窗口。
    unsafe {
        event::event_set_dispatch(&mut *event_ptr, this_val, 2);
    }

    // 取事件类型串、intern 得 si，查注册表取监听器快照。
    let type_val = match event::event_type_value(event_obj) {
        Some(v) => v,
        None => return NativeResult::Ok(JsValue::bool(true)),
    };
    let type_text = match vm.lookup_str(type_val) {
        Some(t) => t,
        None => return NativeResult::Ok(JsValue::bool(true)),
    };
    let type_si = vm.perm_intern(&type_text);
    // 注册表按目标对象存全部监听器，此处按事件类型过滤（同型才派发）。
    let listeners: Vec<ListenerEntry> = vm.et_lookup(this_ptr).into_iter().filter(|e| e.type_si == type_si).collect();
    if listeners.is_empty() {
        return NativeResult::Ok(JsValue::bool(true));
    }

    // 逐个监听器调用（GC 根纪律：调用前钉事件与回调进寄存器，调用后重读载荷指针）。
    for entry in &listeners {
        // 钉事件对象与回调进返回寄存器（GC 根，防调用窗口内被回收）。
        vm.set_reg(0, event_val);
        vm.set_reg(1, entry.callback);
        // 调用监听器（this = 目标，实参 = 事件）。异常原值上抛。
        match vm.call_function_sync(entry.callback, this_val, &[event_val]) {
            Ok(_) => {}
            Err(_) => {
                if let Some(exc) = vm.take_uncaught_value() {
                    return NativeResult::Err(exc);
                }
                return NativeResult::Err(crate::error::create_type_error(vm, "Event listener threw"));
            }
        }
        // 调用窗口后重读事件对象（窗口内可发生 GC，原地不搬移但重读守纪律）。
        let event_val = vm.reg(0);
        let event_ptr = event_val.as_js_object_ptr();
        // SAFETY: event_val 是 Event 对象（品牌守卫已过），存活。
        let event_obj = unsafe { &*event_ptr };
        // once 条目调用后移除。
        if entry.once {
            vm.et_unregister(this_ptr, entry.type_si, entry.callback, entry.capture);
        }
        // stopImmediatePropagation 截断循环。
        if event::event_immediate_stopped(event_obj) == Some(true) {
            break;
        }
    }

    // 返回 !event.defaultPrevented（末次调用窗口后重读载荷指针）。
    let event_val = vm.reg(0);
    let event_ptr = event_val.as_js_object_ptr();
    // SAFETY: event_val 是 Event 对象（品牌守卫已过），存活。
    let event_obj = unsafe { &*event_ptr };
    let default_prevented = event::event_default_prevented(event_obj).unwrap_or(false);
    NativeResult::Ok(JsValue::bool(!default_prevented))
}
