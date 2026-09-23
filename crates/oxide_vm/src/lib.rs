#![doc = "OxideJS - Register-based VM with epoch arena memory"]
//!
//! 字节码执行与运行时内核层：基于寄存器的 VM 负责执行 `CompiledModule` 的
//! bytecode，采用 epoch arena 管理 session 内对象内存，并提供 session 级
//! mark-sweep GC、VM 池、native 函数绑定与内置对象初始化。
//!
//! 核心入口为 [`vm::Vm`]；`VmPool` 负责 VM 实例的复用；`session_gc` 提供
//! session 内存回收；`bindings` 把内置对象绑定到每个 session 的 global 上。

mod async_disposable;
mod async_from_sync;
mod async_func;
mod async_generator;
mod atomics;
/// 内置对象绑定模块：向 session 的 global 对象安装 Object/Array/... 及各原型。
pub mod bindings;
mod dispatch;
mod generator;
mod ic_helper;
/// native 函数签名类型（[`native::NativeFn`]），builtin 绑定与 VM 调用约定依赖它。
pub mod native;
/// Promise 运行时（状态盒 + 微任务队列 + 构造器/方法实现）。
pub mod promise;
mod session_arena;
/// session 级 mark-sweep GC：标记-清扫 session arena 对象与 session 字符串。
pub mod session_gc;
mod suspended;
/// test262 宿主对象 `$262` 的最小实现（非标准全局，供 test262 套件使用）。
pub mod test262_host;
/// 寄存器 VM 主类型（[`vm::Vm`]）与其执行状态定义。
pub mod vm;
mod vm_dispatch_ctrl;
mod vm_dispatch_misc;
/// VM 层日志宏。
pub mod vm_log;
/// VM 池：多个 VM 实例的创建、复用与归还（[`vm_pool::VmPool`] / [`vm_pool::VmGuard`]）。
pub mod vm_pool;
mod vm_props;
mod vm_runtime;
mod vm_state;
mod vm_support;
/// JS 值类型再导出（来自 `oxide_types`），作为本 crate 公共 API 的一部分。
pub use oxide_types::value::JsValue;
