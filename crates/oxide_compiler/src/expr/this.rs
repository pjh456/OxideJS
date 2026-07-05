use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::opcode::{self, OpCode};

impl Compiler {
    pub(crate) fn count_this_expression(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        ctx.projected_pc += 1;
    }

    pub(crate) fn emit_this_expression(&self, ctx: &mut CompileCtx) -> Result<u8, String> {
        let r = ctx.alloc_reg();
        let src = ctx.static_block_this_reg.unwrap_or(254);
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, src, 0));
        Ok(r)
    }
}
