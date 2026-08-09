//! 表达式编译域：表达式分发 + sequence/unsupported。
//!
//! `emit_expression` 按 `Expression` 变体分发到各子模块；本文件还处理
//! 序列表达式（逐项求值取末值）与不支持表达式的报错。

use crate::{CompileCtx, Emitter};
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
pub mod yield_expr;

impl Emitter {
    pub(crate) fn emit_unsupported_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        let _ = ctx;
        Err(format!("unsupported expression type: {:?}", expr))
    }

    pub(crate) fn emit_sequence_expression(
        &self, seq: &oxide_parser::SequenceExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let mut last = 0u32;
        for e in &seq.expressions {
            last = self.emit_expression(e, ctx)?;
        }
        Ok(last)
    }
}
