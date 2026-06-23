use super::*;

impl Compiler {
    pub(in crate::counter) fn count_expression_statement(
        &self, stmt: &oxide_parser::ExpressionStatement<'_>, ctx: &mut CompileCtx,
    ) {
        self.count_expression(&stmt.expression, ctx);
    }

    pub(in crate::counter) fn count_return_statement(
        &self, stmt: &oxide_parser::ReturnStatement<'_>, ctx: &mut CompileCtx,
    ) {
        if let Some(arg) = &stmt.argument {
            self.count_expression(arg, ctx);
        }
        ctx.projected_pc += 1; // RETURN
    }

    pub(in crate::counter) fn count_break_statement(&self, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1; // JMP
    }

    pub(in crate::counter) fn count_continue_statement(&self, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1; // JMP
    }

    pub(in crate::counter) fn count_labeled_statement(
        &self, stmt: &oxide_parser::LabeledStatement<'_>, ctx: &mut CompileCtx,
    ) {
        let body_is_loop = matches!(
            stmt.body,
            Statement::WhileStatement(_)
                | Statement::DoWhileStatement(_)
                | Statement::ForStatement(_)
                | Statement::ForInStatement(_)
                | Statement::ForOfStatement(_)
        );
        if body_is_loop {
            // Loop body owns its labels; labeled break/continue reuse them.
            self.count_statement(&stmt.body, ctx);
        } else {
            // Non-loop label: consume one id (parity with emit) and place the
            // LabeledEnd target right after the body for `break label`.
            let id = ctx.next_label_id();
            self.count_statement(&stmt.body, ctx);
            ctx.label_map.insert(Label::LabeledEnd(id), ctx.projected_pc);
        }
    }
}
