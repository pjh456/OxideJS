#[macro_export]
macro_rules! __log_event {
    ($target:expr, $level:expr, $($arg:tt)*) => {
        $crate::tracing::event!(target: $target, $level, $($arg)*)
    };
}
