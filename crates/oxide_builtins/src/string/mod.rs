//! String 内置对象实现（constructor 与全部 String.prototype 方法）。
//!
//! 按职责分 `common` 文本通道、`basic` 基础方法族、`regex` 正则与匹配族，
//! 三族经本模块 glob 重导出供绑定层与 `regexp` 模块按 `crate::string::` 消费。

mod basic;
mod common;
mod regex;

pub use basic::*;
pub(crate) use common::*;
pub use regex::*;
