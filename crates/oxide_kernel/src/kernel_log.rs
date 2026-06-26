#[macro_export]
macro_rules! kernel_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

#[macro_export]
macro_rules! kernel_warn {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::WARN, $($arg)*)
    };
}

#[macro_export]
macro_rules! kernel_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

#[macro_export]
macro_rules! kernel_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}

#[macro_export]
macro_rules! kernel_trace {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::kernel", oxide_log::tracing::Level::TRACE, $($arg)*)
    };
}
