//! liveness 日志宏：`liveness_error!/warn!/info!/debug!/trace!`，经由 oxide_log 上报。
//! 编译管线属 Kernel 子系统，`OXIDE_LOG=oxide::kernel=debug` 控制本 crate 日志。

#[macro_export]
macro_rules! liveness_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

#[macro_export]
macro_rules! liveness_warn {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::WARN, $($arg)*)
    };
}

#[macro_export]
macro_rules! liveness_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

#[macro_export]
macro_rules! liveness_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}

#[macro_export]
macro_rules! liveness_trace {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::TRACE, $($arg)*)
    };
}
