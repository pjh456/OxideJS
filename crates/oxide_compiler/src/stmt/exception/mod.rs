use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Statement;

mod throw;
mod try_catch;

impl Compiler {
    pub(crate) fn emit_exception_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::ThrowStatement(_) => self.emit_throw_statement(stmt, ctx),
            Statement::TryStatement(_) => self.emit_try_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
