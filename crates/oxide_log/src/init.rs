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

#[derive(Debug, Clone)]
pub enum Output {
    Stderr,
    Stdout,
    File(PathBuf),
}

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

pub fn get_subsystem() -> &'static SubsystemFilter {
    SUBSYSTEM.get().expect("oxide_log::init() must be called before logging")
}

pub fn set_level(id: SubsystemId, level: Level) {
    get_subsystem().set_level(id, level);
}

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
