#[macro_export]
macro_rules! __log_event {
    ($target:expr, $level:expr, $($arg:tt)*) => {
        tracing::event!(target: $target, $level, $($arg)*)
    };
}
