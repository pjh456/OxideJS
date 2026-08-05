//! 日志级别定义。
//!
//! [`Level`] 是引擎内部使用的日志级别枚举，与 `tracing` 的级别一一对应，
//! 通过 [`Level::as_tracing_level`] 转换。值以 `u8` 存储（`repr(u8)`），
//! 可直接写入 `SubsystemFilter` 的原子级别槽位。

/// 日志级别。
///
/// 由低到高：`Off` < `Error` < `Warn` < `Info` < `Debug` < `Trace`。
/// `Off` 表示完全关闭，仅由 `SubsystemFilter` 判读，不映射到 `tracing` 级别。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    /// 转换为 `tracing` 的 [`tracing::Level`]。
    ///
    /// `Off` 与 `Error` 都映射到 `ERROR`——关闭语义由
    /// [`SubsystemFilter`](crate::SubsystemFilter) 的 `Off` 分支单独处理，
    /// 这里仅保证其余级别 1:1 对齐。
    pub const fn as_tracing_level(self) -> tracing::Level {
        match self {
            Level::Off | Level::Error => tracing::Level::ERROR,
            Level::Warn => tracing::Level::WARN,
            Level::Info => tracing::Level::INFO,
            Level::Debug => tracing::Level::DEBUG,
            Level::Trace => tracing::Level::TRACE,
        }
    }
}
