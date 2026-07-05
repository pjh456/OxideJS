use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Expression;

mod array;
mod object_literal;

impl Compiler {
    pub(crate) fn count_object_domain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::ObjectExpression(_) => self.count_object_expression(expr, ctx),
            Expression::ArrayExpression(_) => self.count_array_expression(expr, ctx),
            _ => {}
        }
    }

    pub(crate) fn emit_object_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::ObjectExpression(obj) => self.emit_object_expression(obj, ctx),
            Expression::ArrayExpression(arr) => self.emit_array_expression(arr, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
