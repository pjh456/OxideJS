//! do-while 语句 emit：`emit_do_while_statement` 先执行体后测条件。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_parser::Statement;

impl Emitter {
    pub(crate) fn emit_do_while_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let Statement::DoWhileStatement(dw) = stmt else {
            return Ok(None);
        };
        let start_label = ctx.next_label_id();
        let cont_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        // continue 目标是条件位置：体先执行，continue 须绕过体直接去求值条件，
        // 不能指回体首（否则条件永不被求值）。
        ctx.push_loop(end_label, cont_label, crate::emit_ctx::LoopKind::Plain);
        let n_labeled = ctx.take_pending_loop_labels(end_label, cont_label);
        let body_result = self.emit_statement(&dw.body, ctx)?;
        ctx.labels.set_label_pos(cont_label, ctx.insts.len());
        let test_reg = self.emit_expression(&dw.test, ctx)?;
        ctx.inst(Inst::jmp_if_true(test_reg, start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        // 循环完成值 = 循环体最后一次非空完成值（体收敛寄存器每迭代覆写）；空体
        // 物化 undefined 作为带值完成返回，不沿用循环前的值。
        let result = match body_result {
            Some(r) => r,
            None => self.emit_undefined(ctx),
        };
        Ok(Some(result))
    }
}
