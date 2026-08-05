//! 内部日志转发宏。
//!
//! [`__log_event`] 是 `$crate::tracing::event!` 的薄封装，供外部 crate 统一
//! 转发日志事件，避免各处直接依赖 `tracing` 的路径。

#[macro_export]
macro_rules! __log_event {
    ($target:expr, $level:expr, $($arg:tt)*) => {
        $crate::tracing::event!(target: $target, $level, $($arg)*)
    };
}
