use super::*;

mod basic;
mod block;
mod control;
mod declaration;
mod exception;
mod iteration;
mod switch;

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
}
