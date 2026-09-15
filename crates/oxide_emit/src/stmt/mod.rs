//! 语句编译域：`emit_statement` 分发到块/控制流/声明/异常/迭代等子模块。

use crate::{CompileCtx, Emitter};
use oxide_parser::Statement;

pub mod basic;
pub mod block;
pub mod control;
pub mod declaration;
pub mod exception;
pub mod iteration;
pub mod switch;
pub mod with;

impl Emitter {
    pub(crate) fn emit_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
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
            Statement::WithStatement(_) => self.emit_with_domain(stmt, ctx),
            Statement::ExportNamedDeclaration(_)
            | Statement::ExportDefaultDeclaration(_)
            | Statement::ExportAllDeclaration(_) => self.emit_module_export_domain(stmt, ctx),
            _ => Ok(None),
        }
    }
}
