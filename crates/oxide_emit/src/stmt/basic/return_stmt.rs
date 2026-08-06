//! return 语句 emit：`emit_return_statement` 求值返回值并生成返回指令。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::Statement;

impl Emitter {
    fn emit_return_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::ReturnStatement(ret) = stmt else {
            return Ok(None);
        };
        match &ret.argument {
            Some(expr) => {
                let r = self.emit_expression(expr, ctx)?;
                ctx.inst(Inst::new(OpCode::RETURN, Operand::Reg(r), Operand::None, Operand::None));
            }
            None => {
                ctx.inst(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));
            }
        }
        Ok(None)
    }

    pub(crate) fn emit_basic_return(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        self.emit_return_statement(stmt, ctx)
    }
}
