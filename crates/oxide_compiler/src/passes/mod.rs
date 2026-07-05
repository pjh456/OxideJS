#[allow(unused_imports)]
use oxide_bytecode::module::Constant;
#[allow(unused_imports)]
use oxide_bytecode::opcode::{self, OpCode};
#[allow(unused_imports)]
use oxide_parser::{
    AssignmentOperator, AssignmentTarget, AssignmentTargetMaybeDefault, AssignmentTargetProperty, BindingPattern,
    ChainElement, Class, ClassElement, Expression, ForStatementInit, ForStatementLeft, LogicalOperator,
    MethodDefinitionKind, ObjectAssignmentTarget, PropertyKey, PropertyKind, SimpleAssignmentTarget, Statement,
    UnaryOperator, UpdateOperator, VariableDeclarationKind,
};

#[allow(unused_imports)]
use crate::compiler::{
    is_int_literal, is_side_effect_free, BinaryOperator, CompileCtx, Compiler, FunctionBodyContext, Label, ParamSpec,
};

pub mod class;
pub mod expr;
pub mod shared;
pub mod stmt;

impl Compiler {
    pub(crate) fn count_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(_)
            | Statement::ReturnStatement(_)
            | Statement::BreakStatement(_)
            | Statement::ContinueStatement(_)
            | Statement::LabeledStatement(_) => self.count_basic(stmt, ctx),
            Statement::BlockStatement(_) => self.count_block_domain(stmt, ctx),
            Statement::VariableDeclaration(_) | Statement::FunctionDeclaration(_) | Statement::ClassDeclaration(_) => {
                self.count_declaration_domain(stmt, ctx)
            }
            Statement::IfStatement(_) => self.count_control_domain(stmt, ctx),
            Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_) => self.count_iteration_domain(stmt, ctx),
            Statement::SwitchStatement(_) => self.count_switch_domain(stmt, ctx),
            Statement::ThrowStatement(_) | Statement::TryStatement(_) => self.count_exception_domain(stmt, ctx),
            _ => {}
        }
    }

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

    pub(crate) fn emit_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::ExpressionStatement(_) | Statement::ReturnStatement(_) | Statement::EmptyStatement(_) => {
                self.emit_basic_domain(stmt, ctx)
            }
            Statement::BlockStatement(_) => self.emit_block_domain(stmt, ctx),
            Statement::VariableDeclaration(_) | Statement::FunctionDeclaration(_) | Statement::ClassDeclaration(_) => {
                self.emit_declaration_domain(stmt, ctx)
            }
            Statement::IfStatement(_) => self.emit_control_domain(stmt, ctx),
            Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_) => self.emit_iteration_domain(stmt, ctx),
            Statement::SwitchStatement(_) => self.emit_switch_domain(stmt, ctx),
            Statement::ThrowStatement(_) | Statement::TryStatement(_) => self.emit_exception_domain(stmt, ctx),
            Statement::BreakStatement(b) => self.emit_break_statement(b, ctx),
            Statement::ContinueStatement(c) => self.emit_continue_statement(c, ctx),
            Statement::LabeledStatement(ls) => self.emit_labeled_statement(ls, ctx),
            _ => Ok(None),
        }
    }

    pub(crate) fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::NumericLiteral(_)
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
            Expression::CallExpression(_) => self.emit_call_domain(expr, ctx),
            Expression::ThisExpression(_) => self.emit_this_expression(ctx),
            Expression::SequenceExpression(seq) => self.emit_sequence_expression(seq, ctx),
            Expression::ParenthesizedExpression(p) => self.emit_parenthesized_expression(p, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
