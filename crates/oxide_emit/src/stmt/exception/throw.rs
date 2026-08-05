//! throw 语句 emit：`emit_throw_statement` 求值异常值并生成 THROW。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::Statement;

impl Emitter {
    pub(crate) fn emit_throw_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ThrowStatement(ts) = stmt else {
            return Ok(None);
        };
        let exc_reg = self.emit_expression(&ts.argument, ctx)?;
        ctx.inst(Inst::new(OpCode::THROW, Operand::Reg(exc_reg as u32), Operand::None, Operand::None));
        Ok(None)
    }
}
