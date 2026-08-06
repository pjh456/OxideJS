//! 括号表达式 emit：`emit_parenthesized_expression` 透传内层表达式。

use crate::{CompileCtx, Emitter};

impl Emitter {
    pub(crate) fn emit_parenthesized_expression(
        &self, p: &oxide_parser::ParenthesizedExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        self.emit_expression(&p.expression, ctx)
    }
}
