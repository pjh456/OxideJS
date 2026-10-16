//! `this` 表达式 emit：`emit_this_expression` 解析当前函数的 this 寄存器。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;

impl Emitter {
    pub(crate) fn emit_this_expression(&self, ctx: &mut CompileCtx) -> Result<u32, String> {
        let r = ctx.alloc_reg();
        // None 是常规 this 槽；Some(reg) 是类构造器 vreg（vreg 号可越过 254，
        // 按寄存器处理，不与 this 槽特判混淆）。
        let src_op = match ctx.static_block_this_reg {
            None => Operand::This,
            Some(reg) => Operand::Reg(reg),
        };
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(r), src_op, Operand::None));
        Ok(r)
    }
}
