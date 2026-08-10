#![doc = "OxideJS - Shared core types (JsValue, JsObject, Shape, P, Epoch)"]

/// 运行时错误类型（[`JsError`] / [`JsErrorKind`]）。
pub mod error;
/// 内存抽象：持久指针、arena 分配（epoch）与持久堆。
pub mod mem;
/// 对象模型：字符串、对象头、属性元数据与闭包 cell。
pub mod object;
/// 私有名（`#x`）键编码。
pub mod private_key;
/// 形状（hidden class）存储。
pub mod shape;
/// ECMAScript 值（NaN-boxing 的 `JsValue`）。
pub mod value;

/// 类型层日志宏（`types_error`/`types_info` 等），target 为 `oxide::kernel`。
mod types_log;

/// 重导出 [`error`] 模块的运行时错误类型。
pub use error::{JsError, JsErrorKind};
