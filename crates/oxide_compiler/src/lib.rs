//! oxide_compiler：AST → bytecode 编译入口（编译管线收尾）。
//!
//! `Compiler::compile` 串联 `oxide_emit`（AST → IR）与 `oxide_ir::lower`
//! （IR → bytecode），产出 `CompiledModule`。另有 AST 结构哈希
//! （`structural_hash` / `compiled_module_hash`）用于编译缓存键。

pub mod compiler;
pub mod compiler_log;
pub mod flatten;
pub mod hash;
