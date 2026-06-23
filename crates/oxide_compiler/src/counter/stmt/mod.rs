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
            Statement::ExpressionStatement(es) => self.count_expression_statement(es, ctx),
            Statement::VariableDeclaration(decl) => self.count_variable_declaration(decl, ctx),
            Statement::ReturnStatement(ret) => self.count_return_statement(ret, ctx),
            Statement::IfStatement(ifs) => self.count_if_statement(ifs, ctx),
            Statement::WhileStatement(wh) => self.count_while_statement(wh, ctx),
            Statement::DoWhileStatement(dw) => self.count_do_while_statement(dw, ctx),
            Statement::ForStatement(fr) => self.count_for_statement(fr, ctx),
            Statement::ForInStatement(fi) => self.count_for_in_statement(fi, ctx),
            Statement::ForOfStatement(fo) => self.count_for_of_statement(fo, ctx),
            Statement::SwitchStatement(sw) => self.count_switch_statement(sw, ctx),
            Statement::BreakStatement(_) => self.count_break_statement(ctx),
            Statement::ContinueStatement(_) => self.count_continue_statement(ctx),
            Statement::BlockStatement(block) => self.count_block_statement(block, ctx),
            Statement::FunctionDeclaration(fd) => self.count_function_declaration(fd, ctx),
            Statement::ClassDeclaration(class) => self.count_class_declaration(class, ctx),
            Statement::ThrowStatement(ts) => self.count_throw_statement(ts, ctx),
            Statement::TryStatement(ts) => self.count_try_statement(ts, ctx),
            Statement::LabeledStatement(ls) => self.count_labeled_statement(ls, ctx),
            _ => {}
        }
    }
}
