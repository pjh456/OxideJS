//! 对象表达式域：对象/数组字面量子模块与域分发 `emit_object_domain`。

use crate::{CompileCtx, Emitter};
use oxide_parser::Expression;

mod array;
mod object_literal;

impl Emitter {
    pub(crate) fn emit_object_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::ObjectExpression(obj) => self.emit_object_expression(obj, ctx),
            Expression::ArrayExpression(arr) => self.emit_array_expression(arr, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
