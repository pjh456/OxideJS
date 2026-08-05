/// 输出 ERROR 级别日志，target 固定为 `oxide::test262`。
#[macro_export]
macro_rules! test262_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::test262", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

/// 输出 INFO 级别日志，target 固定为 `oxide::test262`。
#[macro_export]
macro_rules! test262_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::test262", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

/// 输出 DEBUG 级别日志，target 固定为 `oxide::test262`。
#[macro_export]
macro_rules! test262_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::test262", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}
