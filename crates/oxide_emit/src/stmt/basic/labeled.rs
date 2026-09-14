//! 标签语句 emit：`emit_labeled_statement` 登记 break/continue 标签作用域。

use crate::{CompileCtx, Emitter};
use oxide_parser::Statement;

impl Emitter {
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
    ) -> Result<Option<u32>, String> {
        let name = stmt.label.name.as_str();
        // 标签语句完成值 = 体完成值（透传）：迭代体返回其循环值，块/表达式体返回其
        // 收敛值；标签本身不改变完成值。
        let body_result = if Self::is_iteration_statement(&stmt.body) {
            ctx.queue_loop_label(name)?;
            self.emit_statement(&stmt.body, ctx)?
        } else {
            let id = ctx.next_label_id();
            ctx.push_label_scope(name, id, None)?;
            let r = self.emit_statement(&stmt.body, ctx)?;
            ctx.labels.set_label_pos(id, ctx.insts.len());
            ctx.pop_label_scope();
            r
        };
        Ok(body_result)
    }
}
