//! 表达式语句 emit：`emit_expression_statement` 求值并（按需）丢弃结果。

use crate::{CompileCtx, Emitter};
use oxide_parser::Statement;

impl Emitter {
    fn emit_expression_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::ExpressionStatement(es) = stmt else {
            return Ok(None);
        };
        Ok(Some(self.emit_expression(&es.expression, ctx)?))
    }

    pub(crate) fn emit_basic_expression(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        self.emit_expression_statement(stmt, ctx)
    }
}
