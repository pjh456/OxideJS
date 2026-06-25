use super::*;

mod assignment;
mod call;
mod chain;
mod control;
mod function;
mod literal;
mod member;
mod object;
mod operator;
mod template;

impl Compiler {
    pub(crate) fn count_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::BinaryExpression(_)
            | Expression::PrivateInExpression(_)
            | Expression::UnaryExpression(_)
            | Expression::UpdateExpression(_) => self.count_operator(expr, ctx),
            Expression::CallExpression(_) | Expression::NewExpression(_) => self.count_call_domain(expr, ctx),
            Expression::AssignmentExpression(_) => self.count_assignment(expr, ctx),
            Expression::ConditionalExpression(_)
            | Expression::SequenceExpression(_)
            | Expression::LogicalExpression(_) => self.count_conditional_chain(expr, ctx),
            Expression::ChainExpression(_) => self.count_chain_expression(expr, ctx),
            Expression::ObjectExpression(_) | Expression::ArrayExpression(_) => self.count_object_domain(expr, ctx),
            Expression::TemplateLiteral(_) | Expression::TaggedTemplateExpression(_) => {
                self.count_template_domain(expr, ctx)
            }
            Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ClassExpression(_) => self.count_function_domain(expr, ctx),
            Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_) => self.count_member_domain(expr, ctx),
            Expression::ParenthesizedExpression(_) => self.count_parenthesized_expression(expr, ctx),
            Expression::ThisExpression(_) => self.count_this_expression(ctx),
            Expression::Identifier(_) => self.count_identifier_expression(expr, ctx),
            Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::RegExpLiteral(_) => self.count_literal(expr, ctx),
            _ => self.count_default_expression(ctx),
        }
    }
}
