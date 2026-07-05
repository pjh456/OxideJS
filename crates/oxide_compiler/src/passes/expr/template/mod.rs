use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Expression;

mod tagged;
mod template_literal;

impl Compiler {
    pub(crate) fn count_template_domain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::TemplateLiteral(_) => self.count_template_literal(expr, ctx),
            Expression::TaggedTemplateExpression(_) => self.count_tagged_template_expression(expr, ctx),
            _ => {}
        }
    }

    pub(crate) fn emit_template_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::TemplateLiteral(tl) => self.emit_template_literal_expression(tl, ctx),
            Expression::TaggedTemplateExpression(tt) => self.emit_tagged_template_expression(tt, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
