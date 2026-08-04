use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::Statement;

impl Compiler {
    pub(crate) fn emit_throw_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ThrowStatement(ts) = stmt else {
            return Ok(None);
        };
        let exc_reg = self.emit_expression(&ts.argument, ctx)?;
        ctx.emit(opcode::encode(OpCode::THROW, exc_reg, 0, 0));
        Ok(None)
    }
}
