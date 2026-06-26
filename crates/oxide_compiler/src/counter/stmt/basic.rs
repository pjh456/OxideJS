use super::*;

impl Compiler {
    fn count_expression_statement(&self, stmt: &oxide_parser::ExpressionStatement<'_>, ctx: &mut CompileCtx) {
        self.count_expression(&stmt.expression, ctx);
    }

    fn count_return_statement(&self, stmt: &oxide_parser::ReturnStatement<'_>, ctx: &mut CompileCtx) {
        if let Some(arg) = &stmt.argument {
            self.count_expression(arg, ctx);
        }
        ctx.projected_pc += 1; // RETURN
    }

    fn count_break_statement(&self, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1; // JMP
    }

    fn count_continue_statement(&self, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1; // JMP
    }

    fn count_labeled_statement(&self, stmt: &oxide_parser::LabeledStatement<'_>, ctx: &mut CompileCtx) {
        let body_is_loop = matches!(
            stmt.body,
            Statement::WhileStatement(_)
                | Statement::DoWhileStatement(_)
                | Statement::ForStatement(_)
                | Statement::ForInStatement(_)
                | Statement::ForOfStatement(_)
        );
        if body_is_loop {
            self.count_statement(&stmt.body, ctx);
        } else {
            let id = ctx.next_label_id();
            self.count_statement(&stmt.body, ctx);
            ctx.labels.label_map.insert(Label::LabeledEnd(id), ctx.projected_pc);
        }
    }

    pub(in crate::counter) fn count_basic(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => self.count_expression_statement(es, ctx),
            Statement::ReturnStatement(ret) => self.count_return_statement(ret, ctx),
            Statement::BreakStatement(_) => self.count_break_statement(ctx),
            Statement::ContinueStatement(_) => self.count_continue_statement(ctx),
            Statement::LabeledStatement(ls) => self.count_labeled_statement(ls, ctx),
            _ => {}
        }
    }
}
