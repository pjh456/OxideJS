use crate::compiler::{CompileCtx, Compiler};
use oxide_parser::Statement;

impl Compiler {
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
            ctx.push_label_scope(name, id, None)?;
            self.emit_statement(&stmt.body, ctx)?;
            ctx.labels.set_label_pos(id, ctx.insts.len());
            ctx.pop_label_scope();
        }
        Ok(None)
    }
}
