use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::Statement;

impl Compiler {
    pub(super) fn count_return_statement(&self, stmt: &oxide_parser::ReturnStatement<'_>, ctx: &mut CompileCtx) {
        if let Some(arg) = &stmt.argument {
            self.count_expression(arg, ctx);
        }
        ctx.projected_pc += 1;
    }

    fn emit_return_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ReturnStatement(ret) = stmt else {
            return Ok(None);
        };
        match &ret.argument {
            Some(expr) => {
                let r = self.emit_expression(expr, ctx)?;
                ctx.emit(opcode::encode(OpCode::RETURN, r, 0, 0));
            }
            None => {
                ctx.emit(opcode::encode(OpCode::RETURN, 0, 0, 0));
            }
        }
        Ok(None)
    }

    pub(crate) fn emit_basic_return(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        self.emit_return_statement(stmt, ctx)
    }
}
