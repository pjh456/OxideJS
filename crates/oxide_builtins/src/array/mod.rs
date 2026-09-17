//! Array 内置对象实现（constructor、静态方法与全部 Array.prototype 方法）。
//!
//! 按职责分 `common` 共享助手（含 `array_ptr` 族宏）、`from` 构造器与静态方法、`element` 元素变更与查找、
//! `iterate` 高阶迭代、`sort_iterator` 排序与迭代协议、`immutable` ES2023 不可变方法族；各族经本模块重导出。

mod common;
pub(crate) use common::*;

mod from;
pub use from::*;

mod element;
pub use element::*;

mod iterate;
pub use iterate::*;

mod sort_iterator;
pub use sort_iterator::*;

mod immutable;
pub use immutable::*;
