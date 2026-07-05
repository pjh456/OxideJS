use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Expression;

impl Compiler {
    pub(crate) fn count_parenthesized_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        if let Expression::ParenthesizedExpression(p) = expr {
            self.count_expression(&p.expression, ctx);
        }
    }

    pub(crate) fn emit_parenthesized_expression(
        &self, p: &oxide_parser::ParenthesizedExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        self.emit_expression(&p.expression, ctx)
    }
}
