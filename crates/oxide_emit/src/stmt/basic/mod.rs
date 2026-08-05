use crate::{CompileCtx, Emitter};
use oxide_parser::Statement;

mod break_continue;
mod expression;
mod labeled;
mod return_stmt;

impl Emitter {
    pub(crate) fn emit_basic_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::ExpressionStatement(_) => self.emit_basic_expression(stmt, ctx),
            Statement::ReturnStatement(_) => self.emit_basic_return(stmt, ctx),
            Statement::EmptyStatement(_) => Ok(None),
            _ => Ok(None),
        }
    }
}
