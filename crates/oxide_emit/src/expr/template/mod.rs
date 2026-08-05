use crate::{CompileCtx, Emitter};
use oxide_parser::Expression;

mod tagged;
mod template_literal;

impl Emitter {
    pub(crate) fn emit_template_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::TemplateLiteral(tl) => self.emit_template_literal_expression(tl, ctx),
            Expression::TaggedTemplateExpression(tt) => self.emit_tagged_template_expression(tt, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
