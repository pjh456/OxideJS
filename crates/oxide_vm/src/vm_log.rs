#[macro_export]
macro_rules! vm_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::vm", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

#[macro_export]
macro_rules! vm_warn {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::vm", oxide_log::tracing::Level::WARN, $($arg)*)
    };
}

#[macro_export]
macro_rules! vm_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::vm", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

#[macro_export]
macro_rules! vm_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::vm", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}

#[macro_export]
macro_rules! vm_trace {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::vm", oxide_log::tracing::Level::TRACE, $($arg)*)
    };
}

#[macro_export]
macro_rules! ic_error {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::ic", oxide_log::tracing::Level::ERROR, $($arg)*)
    };
}

#[macro_export]
macro_rules! ic_warn {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::ic", oxide_log::tracing::Level::WARN, $($arg)*)
    };
}

#[macro_export]
macro_rules! ic_info {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::ic", oxide_log::tracing::Level::INFO, $($arg)*)
    };
}

#[macro_export]
macro_rules! ic_debug {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::ic", oxide_log::tracing::Level::DEBUG, $($arg)*)
    };
}

#[macro_export]
macro_rules! ic_trace {
    ($($arg:tt)*) => {
        oxide_log::__log_event!("oxide::ic", oxide_log::tracing::Level::TRACE, $($arg)*)
    };
}
