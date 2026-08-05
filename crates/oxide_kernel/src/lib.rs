#![doc = "OxideJS - Shared kernel (CodeForge, ShapeForge, PropForge, StringForge, BuiltinWorld)"]
//!
//! 运行时共享内核层：所有 VM 实例共享的只读缓存（code cache、hidden class /
//! shape store、属性模板、字符串 intern 表）以及内置对象（builtin world）的
//! 构造与重建逻辑。位于依赖链 `oxide_types ← oxide_kernel ← ...` 中，被
//! `oxide_vm` 与 `oxide_runtime_api` 共同依赖。

/// 内置运行时对象（Object/Array/... 及各自原型）的构造与重建。
pub mod builtin;
/// compiled bytecode cache 的再导出（来自 `oxide_code_cache`）。
pub mod code_forge;
/// kernel 核心：配置、共享缓存入口（[`KernelCore`]）与 per-session 状态。
pub mod kernel;
/// kernel 层日志宏。
pub mod kernel_log;
/// 属性模板缓存：按 shape 缓存属性槽位布局，加速属性查找。
pub mod prop_forge;
/// hidden class（shape）共享存储：把对象结构映射为整数 id。
pub mod shape_forge;
/// 永久字符串 intern 表：属性名/方法名稳定 id 化。
pub mod string_forge;

/// kernel 对外的三个核心类型：配置、共享核心、会话。
pub use kernel::{KernelConfig, KernelCore, KernelSession};
