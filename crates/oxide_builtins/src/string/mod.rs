//! String 内置对象实现（constructor 与全部 String.prototype 方法）。
//!
//! 按职责分 `common` 文本通道、`basic` 基础方法族、`regex` 正则与匹配族，
//! 三族经本模块 glob 重导出，供绑定层（`oxide_builtins::string::`）与 `regexp` 模块（`crate::string::`）消费。

mod basic;
mod common;
mod regex;

pub use basic::*;
pub(crate) use common::*;
pub use regex::*;
