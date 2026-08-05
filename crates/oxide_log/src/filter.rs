//! 按子系统过滤日志事件的 `tracing` 层过滤器。
//!
//! [`SubsystemFilter`] 为每个子系统保存一个独立的原子级别槽位，并实现
//! [`Filter<S>`](tracing_subscriber::layer::Filter)。`tracing` 事件的 target
//! （形如 `oxide::vm::...`）先被映射到子系统，再按该子系统当前级别决定是否放行。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use tracing::Subscriber;
use tracing_subscriber::layer::Filter;

use crate::level::Level;
use crate::subsystem::{SubsystemId, SUBSYSTEM_COUNT};

/// 按子系统维护日志级别的过滤层。
///
/// `Clone` 为浅拷贝（内部 `Arc`），多个 subscriber 层可共享同一份级别状态，
/// 因此 [`set_level`](SubsystemFilter::set_level) 对已初始化的 subscriber 即时生效。
#[derive(Clone)]
pub struct SubsystemFilter {
    levels: Arc<[AtomicU8; SUBSYSTEM_COUNT]>,
}

impl SubsystemFilter {
    /// 创建全部子系统级别为 `Off` 的过滤器。
    pub fn new() -> Self {
        Self {
            levels: Arc::new([AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0)]),
        }
    }

    /// 设置子系统的日志级别（原子写，`Relaxed` 序足够）。
    pub fn set_level(&self, id: SubsystemId, level: Level) {
        self.levels[id as usize].store(level as u8, Ordering::Relaxed);
    }

    /// 读取子系统的当前日志级别。
    pub fn get_level(&self, id: SubsystemId) -> Level {
        let raw = self.levels[id as usize].load(Ordering::Relaxed);
        match raw {
            1 => Level::Error,
            2 => Level::Warn,
            3 => Level::Info,
            4 => Level::Debug,
            5 => Level::Trace,
            _ => Level::Off,
        }
    }
}

impl SubsystemFilter {
    fn is_enabled_for(&self, target: &str, tracing_level: &tracing::Level) -> bool {
        let sid = subsystem_for_target(target);
        let max = self.get_level(sid);
        match max {
            Level::Off => false,
            Level::Error => *tracing_level <= tracing::Level::ERROR,
            Level::Warn => *tracing_level <= tracing::Level::WARN,
            Level::Info => *tracing_level <= tracing::Level::INFO,
            Level::Debug => *tracing_level <= tracing::Level::DEBUG,
            Level::Trace => true,
        }
    }
}

impl Default for SubsystemFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Subscriber> Filter<S> for SubsystemFilter {
    fn enabled(&self, meta: &tracing::Metadata<'_>, _cx: &tracing_subscriber::layer::Context<'_, S>) -> bool {
        self.is_enabled_for(meta.target(), meta.level())
    }
}

fn subsystem_for_target(target: &str) -> SubsystemId {
    if let Some(rest) = target.strip_prefix("oxide::") {
        match rest {
            "vm" => SubsystemId::Vm,
            "ic" => SubsystemId::Ic,
            "kernel" => SubsystemId::Kernel,
            "builtins" => SubsystemId::Builtins,
            _ => {
                if rest.starts_with("vm::") {
                    SubsystemId::Vm
                } else if rest.starts_with("ic::") {
                    SubsystemId::Ic
                } else if rest.starts_with("kernel::") {
                    SubsystemId::Kernel
                } else if rest.starts_with("builtins::") {
                    SubsystemId::Builtins
                } else {
                    SubsystemId::Kernel
                }
            }
        }
    } else {
        SubsystemId::Kernel
    }
}
