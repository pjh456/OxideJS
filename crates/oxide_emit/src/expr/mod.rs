//! 表达式编译域：表达式分发 + sequence/unsupported。
//!
//! `emit_expression` 按 `Expression` 变体分发到各子模块；本文件还处理
//! 序列表达式（逐项求值取末值）与不支持表达式的报错。

use crate::{CompileCtx, Emitter};
use oxide_parser::Expression;

pub mod assignment;
pub mod await_expr;
pub mod call;
pub mod function;
pub mod identifier;
pub mod literal;
pub mod member;
pub mod meta_property;
pub mod object;
pub mod operator;
pub mod parenthesized;
pub mod template;
pub mod this;
pub mod yield_expr;

impl Emitter {
    pub(crate) fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::NumericLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::RegExpLiteral(_) => self.emit_literal(expr, ctx),
            Expression::BinaryExpression(_)
            | Expression::PrivateInExpression(_)
            | Expression::UnaryExpression(_)
            | Expression::ConditionalExpression(_)
            | Expression::LogicalExpression(_)
            | Expression::UpdateExpression(_) => self.emit_operator(expr, ctx),
            Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_)
            | Expression::ChainExpression(_) => self.emit_member_domain(expr, ctx),
            Expression::ObjectExpression(_) | Expression::ArrayExpression(_) => self.emit_object_domain(expr, ctx),
            Expression::AssignmentExpression(assign) => self.emit_assignment_expression(assign, ctx),
            Expression::TemplateLiteral(_) | Expression::TaggedTemplateExpression(_) => {
                self.emit_template_domain(expr, ctx)
            }
            Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ClassExpression(_)
            | Expression::NewExpression(_) => self.emit_function_domain(expr, ctx),
            Expression::Identifier(ident) => self.emit_identifier_expression(ident, ctx),
            Expression::YieldExpression(ye) => self.emit_yield_expression(ye, ctx),
            Expression::AwaitExpression(ae) => self.emit_await_expression(ae, ctx),
            Expression::CallExpression(_) => self.emit_call_domain(expr, ctx),
            Expression::ThisExpression(_) => self.emit_this_expression(ctx),
            Expression::SequenceExpression(seq) => self.emit_sequence_expression(seq, ctx),
            Expression::ParenthesizedExpression(p) => self.emit_parenthesized_expression(p, ctx),
            Expression::MetaProperty(mp) => self.emit_meta_property_expression(mp, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }

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
