//! do-while 语句 emit：`emit_do_while_statement` 先执行体后测条件。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_parser::Statement;

impl Emitter {
    pub(crate) fn emit_do_while_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::DoWhileStatement(dw) = stmt else {
            return Ok(None);
        };
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        self.emit_statement(&dw.body, ctx)?;
        let test_reg = self.emit_expression(&dw.test, ctx)?;
        ctx.inst(Inst::jmp_if_true(test_reg, start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }
}
