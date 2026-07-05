use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Expression;

pub mod assignment;
pub mod call;
pub mod function;
pub mod identifier;
pub mod literal;
pub mod member;
pub mod object;
pub mod operator;
pub mod parenthesized;
pub mod template;
pub mod this;

impl Compiler {
    pub(crate) fn emit_unsupported_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        let _ = ctx;
        Err(format!("unsupported expression type: {:?}", expr))
    }

    pub(crate) fn emit_sequence_expression(
        &self, seq: &oxide_parser::SequenceExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let mut last = 0u8;
        for e in &seq.expressions {
            last = self.emit_expression(e, ctx)?;
        }
        Ok(last)
    }
}
