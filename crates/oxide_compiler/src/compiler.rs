//! 编译入口：`Compiler::compile` 串联 emit 与 lowering。

use oxide_bytecode::module::CompiledModule;
use oxide_emit::Emitter;
use oxide_ir::lower::lower;

/// 结构哈希 / 编译模块哈希 re-export（供编译缓存作键）。
pub use crate::hash::{compiled_module_hash, structural_hash};

/// 编译入口：状态 = DCE 开关，编译其余状态在 emit/lower 内部。
pub struct Compiler {
    /// 是否在 emit 与 lower 之间运行死代码消除（D-10：默认开启）。
    enable_dce: bool,
}

impl Compiler {
    /// 构造 `Compiler`：默认开启 DCE（D-10）。
    pub fn new() -> Self {
        Self { enable_dce: true }
    }

    /// 显式设置 DCE 开关（`with_dce(false)` 关闭死代码消除）。
    pub fn with_dce(self, enable: bool) -> Self {
        Self { enable_dce: enable }
    }

    /// 编译整个 program：AST → IR（`Emitter::emit_program`）→（可选 DCE）→ bytecode（`lower`）。
    pub fn compile(&self, program: &oxide_parser::Program) -> Result<CompiledModule, String> {
        crate::compiler_debug!("compile: starting...");
        let mut ir = Emitter::new().emit_program(program)?;
        if self.enable_dce {
            oxide_dce::dce(&mut ir); // 顶层函数；nested 不递归（D-03）
        }
        crate::compiler_debug!("compile: done, {} instructions", ir.insts.len());
        lower(&ir)
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
