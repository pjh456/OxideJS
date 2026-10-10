//! EventTarget 监听器注册表的数据类型：`ListenerEntry`（单条监听器登记）与
//! `EventTargetState`（一个目标的监听器集合，注册表值）。
//!
//! 本类型落 `oxide_runtime_api`（`VmHost` trait 所在层）：`et_register` /
//! `et_lookup` 的签名引用 `ListenerEntry`，而 trait 不依赖 `oxide_builtins`
//! （依赖方向 `runtime_api ← builtins`），故类型须落在 trait 可引用的层。

use oxide_types::value::JsValue;

/// 单条监听器登记：事件类型 si、回调函数、捕获标志、once 标志、属性 handler 标志。
///
/// `type_si` 是事件类型串的 perm intern 下标（同型同回调同 capture 的重复登记为
/// no-op）。`callback` 是回调函数（GC 对象边）。`is_attribute` 区分属性 handler
/// （`onmessage` 等）与 `addEventListener` 登记（首版恒 false，属性 handler 由后续
/// 任务接入）。
#[derive(Clone, Debug)]
pub struct ListenerEntry {
    pub type_si: u32,
    pub callback: JsValue,
    pub capture: bool,
    pub once: bool,
    pub is_attribute: bool,
}

/// 一个 EventTarget 的监听器集合（per-realm 弱引用注册表的值）。
#[derive(Clone, Debug, Default)]
pub struct EventTargetState {
    pub listeners: Vec<ListenerEntry>,
}
