use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Statement;

impl Compiler {
    pub(super) fn count_expression_statement(
        &self, stmt: &oxide_parser::ExpressionStatement<'_>, ctx: &mut CompileCtx,
    ) {
        self.count_expression(&stmt.expression, ctx);
    }

    fn emit_expression_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ExpressionStatement(es) = stmt else {
            return Ok(None);
        };
        Ok(Some(self.emit_expression(&es.expression, ctx)?))
    }

    pub(crate) fn emit_basic_expression(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        self.emit_expression_statement(stmt, ctx)
    }
}
