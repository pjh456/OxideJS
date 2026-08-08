//! 语句编译域：`emit_statement` 分发到块/控制流/声明/异常/迭代等子模块。

pub mod basic;
pub mod block;
pub mod control;
pub mod declaration;
pub mod exception;
pub mod iteration;
pub mod switch;
pub mod with;
