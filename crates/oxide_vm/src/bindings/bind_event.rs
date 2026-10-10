//! Event 体系绑定：本文件是内核接线占位（零行为变化）。九枚 P 字段
//! （Event / MessageEvent / ErrorEvent / CustomEvent 四枚原型与构造器对加
//! EventTarget 原型）当前是空对象，本函数为它们的绑定入口预留，真实构造器 /
//! 原型方法 / 原型链连线由后续任务就地填充（不新建对象——P 字段本身就是槽位，
//! 快照 / 脏检查 / 收尾枚举已覆盖）。

use std::sync::Arc;

use oxide_kernel::kernel::{KernelCore, KernelSession};
use oxide_types::object::JsObject;

/// Event 体系绑定入口（当前为空占位）。
///
/// # 幂等
/// 本函数从三条路径到达：`init_kernel_builtins`（占位是空对象）、
/// `rebind_dirty_builtins` 的 `event` 分支（占位是重建后的空对象）、
/// global-dirty 路径（占位可能已填满）。本函数当前不写任何槽位，空操作天然
/// 幂等；后续任务就地填充时，各槽写须沿用 `lookup_position` 前置守卫
/// （有槽即跳过），保持与 `bind_broadcast_channel` 同型的幂等契约。
///
/// # 边界
/// 九枚 P 字段是空对象占位，本函数不读不写，行为与未调用一致；
/// EventTarget 原型不占 ProtoKind（builtins 不读它），绑定层后续直接访问
/// `world.event_target_proto` 槽。
///
/// `core` / `session` / `global` / `realm_id` 当前未使用，保留签名与其他
/// 绑定函数一致，供后续任务就地填充时直接取用。
pub fn bind_event(_core: &Arc<KernelCore>, _session: &KernelSession, _global: &mut JsObject, _realm_id: u32) {}
