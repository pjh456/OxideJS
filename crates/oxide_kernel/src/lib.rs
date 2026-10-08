#![doc = "OxideJS 共享内核：所有 VM 实例共享的只读缓存（CodeForge/ShapeForge/PropForge/StringForge）以及内置对象 world（BuiltinWorld）的构造与重建。"]
//!
//! 运行时共享内核层：所有 VM 实例共享的只读缓存（code cache、hidden class /
//! shape store、属性模板、append-only 字符串 intern 表）以及内置对象（builtin world）的
//! 构造与重建逻辑。位于依赖链 `oxide_types ← oxide_kernel ← ...` 中，被
//! `oxide_vm` 与 `oxide_runtime_api` 共同依赖。

/// 内置运行时对象（Object/Array/... 及各自原型）的构造与重建。
pub mod builtin;
/// 字节码缓存的再导出（来自 `oxide_code_cache`）。
pub mod code_forge;
/// kernel 核心：配置、共享缓存入口（[`KernelCore`]）与 per-session 状态。
pub mod kernel;
/// kernel 层日志宏。
pub mod kernel_log;
/// 属性模板缓存：按 shape 缓存属性槽位布局，加速属性查找。
pub mod prop_forge;
/// hidden class（shape）共享存储：把对象结构映射为整数 id。
pub mod shape_forge;
/// 共享字节缓冲：SharedArrayBuffer 的跨线程共享存储（预分配至真实上限，活长原子推进）。
pub mod shared_buffer;

/// kernel 对外的三个核心类型：配置、共享核心、会话。
pub use kernel::{KernelConfig, KernelCore, KernelSession};
/// 永久字符串 intern 表与键编码（本体在 `oxide_types`，此处再导出保持既有路径有效）。
pub use oxide_types::string_forge;
/// 模块命名空间再导出的绑定身份哨兵（本体在 `oxide_types`，此处再导出）。
pub use oxide_types::MODULE_NAMESPACE_BINDING;
/// 跨线程共享字节缓冲（SharedArrayBuffer 的共享存储）。
pub use shared_buffer::SharedBuffer;
