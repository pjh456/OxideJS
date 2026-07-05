use super::super::*;

impl Compiler {
    pub(super) fn count_labeled_statement(&self, stmt: &oxide_parser::LabeledStatement<'_>, ctx: &mut CompileCtx) {
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

    fn is_iteration_statement(stmt: &Statement) -> bool {
        matches!(
            stmt,
            Statement::WhileStatement(_)
                | Statement::DoWhileStatement(_)
                | Statement::ForStatement(_)
                | Statement::ForInStatement(_)
                | Statement::ForOfStatement(_)
        )
    }

    pub(crate) fn emit_labeled_statement(
        &self, stmt: &oxide_parser::LabeledStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let name = stmt.label.name.as_str();
        if Self::is_iteration_statement(&stmt.body) {
            ctx.queue_loop_label(name)?;
            self.emit_statement(&stmt.body, ctx)?;
        } else {
            let id = ctx.next_label_id();
            ctx.push_label_scope(name, Label::LabeledEnd(id), None)?;
            self.emit_statement(&stmt.body, ctx)?;
            ctx.pop_label_scope();
        }
        Ok(None)
    }
}
