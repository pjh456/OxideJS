#[macro_export]
macro_rules! code_cache_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}
