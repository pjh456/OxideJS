use crate::compiler::{CompileCtx, Compiler};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;

impl Compiler {
    pub(crate) fn emit_this_expression(&self, ctx: &mut CompileCtx) -> Result<u8, String> {
        let r = ctx.alloc_reg();
        let src = ctx.static_block_this_reg.unwrap_or(254);
        let src_op = if src == 254 { Operand::This } else { Operand::Reg(src as u32) };
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(r as u32), src_op, Operand::None));
        Ok(r)
    }
}
