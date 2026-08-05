//! OxideJS 统一日志基础设施。
//!
//! 基于 `tracing` / `tracing-subscriber` 实现，日志按子系统（VM / IC / Kernel /
//! Builtins）独立配置级别。入口为 `init`：传入 [`LogConfig`] 后一次性初始化
//! 全局 subscriber；运行期可用 [`set_level`] 动态调整各子系统级别。
//!
//! 环境变量 `OXIDE_LOG` 可在进程启动时覆盖配置，格式与 `RUST_LOG` 类似，
//! 见 `subsystem::apply_env_levels`。

/// 按子系统过滤 `tracing` 事件的订阅者过滤器。
pub mod filter;
/// 日志初始化与运行期级别控制入口。
pub mod init;
/// 日志级别定义。
pub mod level;
/// 内部日志转发宏。
pub mod macros;
/// 子系统标识与 `OXIDE_LOG` 环境变量解析。
pub mod subsystem;

/// 重导出 [`filter::SubsystemFilter`]。
pub use filter::SubsystemFilter;
/// 重导出 `init` 模块的初始化入口与配置类型。
pub use init::{init, set_level, LogConfig, Output};
/// 重导出 [`level::Level`]。
pub use level::Level;
/// 重导出 [`subsystem`] 模块的子系统标识。
pub use subsystem::{SubsystemId, SUBSYSTEM_COUNT};

#[doc(hidden)]
pub use tracing;
