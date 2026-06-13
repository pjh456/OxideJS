use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Once;

use tracing_subscriber::EnvFilter;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

#[repr(usize)]
#[derive(Clone, Copy, Debug)]
pub enum Subsystem {
    Vm = 0,
    Ic = 1,
    Kernel = 2,
    Builtins = 3,
}

pub const SUBSYSTEM_COUNT: usize = 4;

pub struct SubsystemLevel(AtomicU8);

impl SubsystemLevel {
    pub const fn new() -> Self {
        Self(AtomicU8::new(LogLevel::Off as u8))
    }

    pub fn set(&self, level: LogLevel) {
        self.0.store(level as u8, Ordering::Relaxed);
    }

    pub fn get(&self) -> LogLevel {
        match self.0.load(Ordering::Relaxed) {
            1 => LogLevel::Error,
            2 => LogLevel::Warn,
            3 => LogLevel::Info,
            4 => LogLevel::Debug,
            5 => LogLevel::Trace,
            _ => LogLevel::Off,
        }
    }

    pub fn is_enabled(&self, level: LogLevel) -> bool {
        self.0.load(Ordering::Relaxed) >= level as u8
    }
}

impl Default for SubsystemLevel {
    fn default() -> Self {
        Self::new()
    }
}

pub static VM_LEVEL: SubsystemLevel = SubsystemLevel::new();
pub static IC_LEVEL: SubsystemLevel = SubsystemLevel::new();
pub static KERNEL_LEVEL: SubsystemLevel = SubsystemLevel::new();
pub static BUILTINS_LEVEL: SubsystemLevel = SubsystemLevel::new();

pub fn apply_log_levels(levels: &[LogLevel; SUBSYSTEM_COUNT]) {
    VM_LEVEL.set(levels[Subsystem::Vm as usize]);
    IC_LEVEL.set(levels[Subsystem::Ic as usize]);
    KERNEL_LEVEL.set(levels[Subsystem::Kernel as usize]);
    BUILTINS_LEVEL.set(levels[Subsystem::Builtins as usize]);
    apply_env_log_levels();
}

pub fn init_logging(log_levels: &[LogLevel; SUBSYSTEM_COUNT]) {
    static LOGGING_INIT: Once = Once::new();

    LOGGING_INIT.call_once(|| {
        let filter = EnvFilter::try_from_env("OXIDE_LOG").unwrap_or_else(|_| EnvFilter::new("oxide=off"));
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .without_time()
            .compact()
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    });

    apply_log_levels(log_levels);
}

fn apply_env_log_levels() {
    let Ok(spec) = std::env::var("OXIDE_LOG") else {
        return;
    };

    for directive in spec.split(',') {
        let Some((target, level)) = directive.split_once('=') else {
            continue;
        };
        let Some(level) = parse_level(level.trim()) else {
            continue;
        };
        match target.trim() {
            "oxide" => {
                VM_LEVEL.set(level);
                IC_LEVEL.set(level);
                KERNEL_LEVEL.set(level);
                BUILTINS_LEVEL.set(level);
            }
            "oxide::vm" => VM_LEVEL.set(level),
            "oxide::ic" => IC_LEVEL.set(level),
            "oxide::kernel" => KERNEL_LEVEL.set(level),
            "oxide::builtins" => BUILTINS_LEVEL.set(level),
            _ => {}
        }
    }
}

fn parse_level(level: &str) -> Option<LogLevel> {
    if level.eq_ignore_ascii_case("off") {
        Some(LogLevel::Off)
    } else if level.eq_ignore_ascii_case("error") {
        Some(LogLevel::Error)
    } else if level.eq_ignore_ascii_case("warn") {
        Some(LogLevel::Warn)
    } else if level.eq_ignore_ascii_case("info") {
        Some(LogLevel::Info)
    } else if level.eq_ignore_ascii_case("debug") {
        Some(LogLevel::Debug)
    } else if level.eq_ignore_ascii_case("trace") {
        Some(LogLevel::Trace)
    } else {
        None
    }
}

#[macro_export]
macro_rules! vm_debug {
    ($($arg:tt)*) => {
        if $crate::logging::VM_LEVEL.is_enabled($crate::logging::LogLevel::Debug) {
            tracing::event!(target: "oxide::vm", tracing::Level::DEBUG, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! vm_trace {
    ($($arg:tt)*) => {
        if $crate::logging::VM_LEVEL.is_enabled($crate::logging::LogLevel::Trace) {
            tracing::event!(target: "oxide::vm", tracing::Level::TRACE, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! ic_debug {
    ($($arg:tt)*) => {
        if $crate::logging::IC_LEVEL.is_enabled($crate::logging::LogLevel::Debug) {
            tracing::event!(target: "oxide::ic", tracing::Level::DEBUG, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! ic_trace {
    ($($arg:tt)*) => {
        if $crate::logging::IC_LEVEL.is_enabled($crate::logging::LogLevel::Trace) {
            tracing::event!(target: "oxide::ic", tracing::Level::TRACE, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! kernel_info {
    ($($arg:tt)*) => {
        if $crate::logging::KERNEL_LEVEL.is_enabled($crate::logging::LogLevel::Info) {
            tracing::event!(target: "oxide::kernel", tracing::Level::INFO, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! kernel_debug {
    ($($arg:tt)*) => {
        if $crate::logging::KERNEL_LEVEL.is_enabled($crate::logging::LogLevel::Debug) {
            tracing::event!(target: "oxide::kernel", tracing::Level::DEBUG, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! kernel_trace {
    ($($arg:tt)*) => {
        if $crate::logging::KERNEL_LEVEL.is_enabled($crate::logging::LogLevel::Trace) {
            tracing::event!(target: "oxide::kernel", tracing::Level::TRACE, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! builtins_debug {
    ($($arg:tt)*) => {
        if $crate::logging::BUILTINS_LEVEL.is_enabled($crate::logging::LogLevel::Debug) {
            tracing::event!(target: "oxide::builtins", tracing::Level::DEBUG, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! builtins_trace {
    ($($arg:tt)*) => {
        if $crate::logging::BUILTINS_LEVEL.is_enabled($crate::logging::LogLevel::Trace) {
            tracing::event!(target: "oxide::builtins", tracing::Level::TRACE, $($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{LogLevel, SubsystemLevel};

    #[test]
    fn log_level_ordinals_match_gate_contract() {
        assert_eq!(LogLevel::Off as u8, 0);
        assert_eq!(LogLevel::Trace as u8, 5);
    }

    #[test]
    fn subsystem_level_defaults_off() {
        let level = SubsystemLevel::new();
        assert_eq!(level.get(), LogLevel::Off);
        assert!(!level.is_enabled(LogLevel::Error));
    }
}
