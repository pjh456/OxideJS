//! 日志初始化。
//!
//! `init` 一次性初始化全局 `tracing` subscriber，输出可定向到 stderr / stdout /
//! 按日滚动的文件；[`set_level`] 与 `get_subsystem` 提供运行期级别控制。
//! 重复调用 `init` 幂等（内部 `Once`），后续调用不生效。

use std::io;
use std::path::PathBuf;
use std::sync::{Once, OnceLock};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

use crate::filter::SubsystemFilter;
use crate::level::Level;
use crate::subsystem::{self, SubsystemId, SUBSYSTEM_COUNT};

/// 日志输出目标。
#[derive(Debug, Clone)]
pub enum Output {
    /// 写到标准错误（默认）。
    Stderr,
    /// 写到标准输出。
    Stdout,
    /// 按日滚动写入 `dir` 目录下的 `oxide.log`。
    File(PathBuf),
}

/// 日志初始化配置。
///
/// `levels` 按 [`SubsystemId`] 的下标顺序给出各子系统初始级别；
/// 默认值为输出到 stderr 且全部子系统关闭。
pub struct LogConfig {
    pub output: Output,
    pub levels: [Level; SUBSYSTEM_COUNT],
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            output: Output::Stderr,
            levels: [Level::Off; SUBSYSTEM_COUNT],
        }
    }
}

static INIT: Once = Once::new();
static SUBSYSTEM: OnceLock<SubsystemFilter> = OnceLock::new();
static FILE_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

/// 获取全局 [`SubsystemFilter`]，供日志宏在检查级别时使用。
///
/// # Panics
///
/// 若在调用 [`init`] 之前调用会 panic——日志宏必须发生在初始化之后。
pub fn get_subsystem() -> &'static SubsystemFilter {
    SUBSYSTEM.get().expect("oxide_log::init() must be called before logging")
}

/// 运行期调整指定子系统的日志级别。
///
/// 直接写全局过滤器，无需重新初始化 subscriber。
pub fn set_level(id: SubsystemId, level: Level) {
    get_subsystem().set_level(id, level);
}

/// 初始化全局日志 subscriber。
///
/// 按 [`LogConfig::levels`] 设置各子系统初始级别，再读取 `OXIDE_LOG` 环境变量
/// 覆盖，最后按 `output` 组装 subscriber。内部以 `Once` 保证只执行一次。
pub fn init(config: &LogConfig) {
    INIT.call_once(|| {
        let filter = SubsystemFilter::new();
        for (i, level) in config.levels.iter().enumerate() {
            let id = match i {
                0 => SubsystemId::Vm,
                1 => SubsystemId::Ic,
                2 => SubsystemId::Kernel,
                3 => SubsystemId::Builtins,
                _ => continue,
            };
            filter.set_level(id, *level);
        }
        SUBSYSTEM.set(filter.clone()).ok();

        let env_filter = EnvFilter::try_from_env("OXIDE_LOG").unwrap_or_else(|_| EnvFilter::new("oxide=off"));

        subsystem::apply_env_levels();

        let stderr_layer = tracing_subscriber::fmt::Layer::default()
            .with_writer(io::stderr)
            .with_ansi(false)
            .without_time()
            .compact();

        match &config.output {
            Output::Stderr => {
                let subscriber = tracing_subscriber::Registry::default()
                    .with(env_filter)
                    .with(stderr_layer.with_filter(filter));
                subscriber.init();
            }
            Output::Stdout => {
                let stdout_layer = tracing_subscriber::fmt::Layer::default()
                    .with_writer(io::stdout)
                    .with_ansi(false)
                    .without_time()
                    .compact();
                let subscriber = tracing_subscriber::Registry::default()
                    .with(env_filter)
                    .with(stdout_layer.with_filter(filter));
                subscriber.init();
            }
            Output::File(dir) => {
                let file_appender = tracing_appender::rolling::daily(dir, "oxide.log");
                let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
                FILE_GUARD.set(guard).ok();

                let file_layer = tracing_subscriber::fmt::Layer::default()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .without_time()
                    .compact();

                let subscriber = tracing_subscriber::Registry::default()
                    .with(env_filter)
                    .with(stderr_layer.with_filter(filter.clone()))
                    .with(file_layer.with_filter(filter));
                subscriber.init();
            }
        }
    });
}
