//! 缓存日志宏：`code_cache_debug!`，经由 oxide_log 上报。

#[macro_export]
macro_rules! code_cache_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}
