use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Statement;

mod break_continue;
mod expression;
mod labeled;
mod return_stmt;

impl Compiler {
    pub(crate) fn emit_basic_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::ExpressionStatement(_) => self.emit_basic_expression(stmt, ctx),
            Statement::ReturnStatement(_) => self.emit_basic_return(stmt, ctx),
            Statement::EmptyStatement(_) => Ok(None),
            _ => Ok(None),
        }
    }

    pub(crate) fn count_basic(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => self.count_expression_statement(es, ctx),
            Statement::ReturnStatement(ret) => self.count_return_statement(ret, ctx),
            Statement::BreakStatement(_) => self.count_break_statement(ctx),
            Statement::ContinueStatement(_) => self.count_continue_statement(ctx),
            Statement::LabeledStatement(ls) => self.count_labeled_statement(ls, ctx),
            _ => {}
        }
    }
}
