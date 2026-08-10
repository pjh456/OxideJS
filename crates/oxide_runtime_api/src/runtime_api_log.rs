//! runtime_api 日志宏：`runtime_api_error!/warn!/info!/debug!/trace!`，经由 oxide_log 上报。
//! 接口层供编译管线与运行时共享，日志归 Kernel 子系统
//! （`OXIDE_LOG=oxide::kernel=debug` 控制本 crate 日志）。

#[macro_export]
macro_rules! runtime_api_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

#[macro_export]
macro_rules! runtime_api_warn {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::WARN, $($arg)*)
    };
}

#[macro_export]
macro_rules! runtime_api_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

#[macro_export]
macro_rules! runtime_api_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}

#[macro_export]
macro_rules! runtime_api_trace {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::TRACE, $($arg)*)
    };
}
