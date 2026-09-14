//! while 语句 emit：`emit_while_statement` 先测条件后执行体。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_parser::Statement;

impl Emitter {
    pub(crate) fn emit_while_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::WhileStatement(wh) = stmt else {
            return Ok(None);
        };
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        ctx.push_loop(end_label, start_label, crate::emit_ctx::LoopKind::Plain);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        let test_reg = self.emit_expression(&wh.test, ctx)?;
        ctx.inst(Inst::jmp_if_false(test_reg, end_label));
        let body_result = self.emit_statement(&wh.body, ctx)?;
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        // 循环完成值 = 循环体最后一次非空完成值：体收敛寄存器每迭代覆写、退出时
        // 持最近值；空体无寄存器可沿用，物化 undefined 作为带值完成返回，不沿用
        // 循环前的值。
        let result = match body_result {
            Some(r) => r,
            None => self.emit_undefined(ctx),
        };
        Ok(Some(result))
    }
}
