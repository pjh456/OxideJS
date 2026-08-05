//! 编译入口：`Compiler::compile` 串联 emit 与 lowering。

use oxide_bytecode::module::CompiledModule;
use oxide_emit::Emitter;
use oxide_ir::lower::lower;

/// 结构哈希 / 编译模块哈希 re-export（供编译缓存作键）。
pub use crate::hash::{compiled_module_hash, structural_hash};

/// 编译入口（marker 类型）：无状态，编译状态在 emit/lower 内部。
pub struct Compiler;

impl Compiler {
    /// 构造 `Compiler`（无状态，恒返回空实例）。
    pub fn new() -> Self {
        Self
    }

    /// 编译整个 program：AST → IR（`Emitter::emit_program`）→ bytecode（`lower`）。
    pub fn compile(&self, program: &oxide_parser::Program) -> Result<CompiledModule, String> {
        crate::compiler_debug!("compile: starting...");
        let ir = Emitter::new().emit_program(program)?;
        crate::compiler_debug!("compile: done, {} instructions", ir.insts.len());
        lower(&ir)
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
