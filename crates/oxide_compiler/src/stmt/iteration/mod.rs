use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Statement;

mod do_while;
mod for_;
mod for_in;
mod for_of;
mod while_loop;

impl Compiler {
    pub(crate) fn count_iteration_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::WhileStatement(wh) => self.count_while_statement(wh, ctx),
            Statement::DoWhileStatement(dw) => self.count_do_while_statement(dw, ctx),
            Statement::ForStatement(fr) => self.count_for_statement(fr, ctx),
            Statement::ForInStatement(fi) => self.count_for_in_statement(fi, ctx),
            Statement::ForOfStatement(fo) => self.count_for_of_statement(fo, ctx),
            _ => {}
        }
    }

    pub(crate) fn emit_iteration_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::WhileStatement(_) => self.emit_while_statement(stmt, ctx),
            Statement::DoWhileStatement(_) => self.emit_do_while_statement(stmt, ctx),
            Statement::ForStatement(_) => self.emit_for_statement(stmt, ctx),
            Statement::ForInStatement(_) => self.emit_for_in_statement(stmt, ctx),
            Statement::ForOfStatement(_) => self.emit_for_of_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
