#![doc = "OxideJS 共享核心类型：ECMAScript 值、对象模型、形状存储、内存抽象与运行时错误类型。"]

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
/// 永久字符串 intern 表与键编码：属性名/方法名稳定 id 化、interner 键
/// 编码、动态编译源编码与有界永久 `JsString` 指针表。
pub mod string_forge;
/// ECMAScript 值（NaN-boxing 的 `JsValue`）。
pub mod value;

/// 类型层日志宏（`types_error`/`types_info` 等），target 为 `oxide::kernel`。
mod types_log;

/// 重导出 [`error`] 模块的运行时错误类型。
pub use error::{JsError, JsErrorKind};

/// 模块命名空间再导出的绑定身份哨兵。
///
/// `export * as ns from mod` 与 `import * as ns from mod; export { ns }` 两路
/// 转发的是同一个命名空间对象，来源身份须用同一绑定名才能判为同一绑定；该字面量
/// 含 NUL，不可能与任何静态导出名相同。编译器与运行时共享此常量，禁止另写字面量。
pub const MODULE_NAMESPACE_BINDING: &str = "\u{0}namespace";
