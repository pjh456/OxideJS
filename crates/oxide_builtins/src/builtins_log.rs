/// 输出 ERROR 级别日志，target 固定为 `oxide::builtins`。
#[macro_export]
macro_rules! builtins_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

/// 输出 WARN 级别日志，target 固定为 `oxide::builtins`。
#[macro_export]
macro_rules! builtins_warn {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::WARN, $($arg)*)
    };
}

/// 输出 INFO 级别日志，target 固定为 `oxide::builtins`。
#[macro_export]
macro_rules! builtins_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

/// 输出 DEBUG 级别日志，target 固定为 `oxide::builtins`。
#[macro_export]
macro_rules! builtins_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}

/// 输出 TRACE 级别日志，target 固定为 `oxide::builtins`。
#[macro_export]
macro_rules! builtins_trace {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::builtins", oxide_log::tracing::Level::TRACE, $($arg)*)
    };
}
