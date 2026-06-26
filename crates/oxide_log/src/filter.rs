use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use tracing::Subscriber;
use tracing_subscriber::layer::Filter;

use crate::level::Level;
use crate::subsystem::{SubsystemId, SUBSYSTEM_COUNT};

#[derive(Clone)]
pub struct SubsystemFilter {
    levels: Arc<[AtomicU8; SUBSYSTEM_COUNT]>,
}

impl SubsystemFilter {
    pub fn new() -> Self {
        Self {
            levels: Arc::new([AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0)]),
        }
    }

    pub fn set_level(&self, id: SubsystemId, level: Level) {
        self.levels[id as usize].store(level as u8, Ordering::Relaxed);
    }

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
