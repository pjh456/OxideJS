#[macro_export]
macro_rules! builtins_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

#[macro_export]
macro_rules! builtins_warn {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::WARN, $($arg)*)
    };
}

#[macro_export]
macro_rules! builtins_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

#[macro_export]
macro_rules! builtins_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}

#[macro_export]
macro_rules! builtins_trace {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::TRACE, $($arg)*)
    };
}
