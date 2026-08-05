use oxide_bytecode::module::CompiledModule;
use oxide_emit::Emitter;
use oxide_ir::lower::lower;

pub use crate::hash::{compiled_module_hash, structural_hash};

pub struct Compiler;

impl Compiler {
    pub fn new() -> Self {
        Self
    }

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
