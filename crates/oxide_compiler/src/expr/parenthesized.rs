use crate::compiler::{CompileCtx, Compiler};

impl Compiler {
    pub(crate) fn emit_parenthesized_expression(
        &self, p: &oxide_parser::ParenthesizedExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        self.emit_expression(&p.expression, ctx)
    }
}
