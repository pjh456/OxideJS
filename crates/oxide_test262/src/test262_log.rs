#[macro_export]
macro_rules! test262_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::test262", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

#[macro_export]
macro_rules! test262_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::test262", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

#[macro_export]
macro_rules! test262_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::test262", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}
