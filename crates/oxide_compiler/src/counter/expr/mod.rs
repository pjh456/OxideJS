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
            Expression::BinaryExpression(_) => self.count_binary_expression(expr, ctx),
            Expression::PrivateInExpression(_) => self.count_private_in_expression(expr, ctx),
            Expression::UnaryExpression(_) => self.count_unary_expression(expr, ctx),
            Expression::CallExpression(_) => self.count_call_expression(expr, ctx),
            Expression::AssignmentExpression(_) => self.count_assignment_expression(expr, ctx),
            Expression::ConditionalExpression(_) => self.count_conditional_expression(expr, ctx),
            Expression::SequenceExpression(_) => self.count_sequence_expression(expr, ctx),
            Expression::LogicalExpression(_) => self.count_logical_expression(expr, ctx),
            Expression::ChainExpression(_) => self.count_chain_expression(expr, ctx),
            Expression::ObjectExpression(_) => self.count_object_expression(expr, ctx),
            Expression::TemplateLiteral(_) => self.count_template_literal(expr, ctx),
            Expression::TaggedTemplateExpression(_) => self.count_tagged_template_expression(expr, ctx),
            Expression::ArrowFunctionExpression(_) => self.count_arrow_function_expression(ctx),
            Expression::FunctionExpression(_) => self.count_function_expression(ctx),
            Expression::ClassExpression(_) => self.count_class_expression(expr, ctx),
            Expression::NewExpression(_) => self.count_new_expression(expr, ctx),
            Expression::ArrayExpression(_) => self.count_array_expression(expr, ctx),
            Expression::StaticMemberExpression(_) => self.count_static_member_expression(expr, ctx),
            Expression::ComputedMemberExpression(_) => self.count_computed_member_expression(expr, ctx),
            Expression::PrivateFieldExpression(_) => self.count_private_field_expression(expr, ctx),
            Expression::ParenthesizedExpression(_) => self.count_parenthesized_expression(expr, ctx),
            Expression::ThisExpression(_) => self.count_this_expression(ctx),
            Expression::Identifier(_) => self.count_identifier_expression(expr, ctx),
            Expression::UpdateExpression(_) => self.count_update_expression(expr, ctx),
            Expression::RegExpLiteral(_) => self.count_regexp_literal(ctx),
            _ => self.count_default_expression(ctx),
        }
    }
}
