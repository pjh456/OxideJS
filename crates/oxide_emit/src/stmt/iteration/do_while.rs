//! do-while 语句 emit：`emit_do_while_statement` 先执行体后测条件。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
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
        // continue 目标是条件位置：体先执行，continue 须绕过体直接去求值条件，
        // 不能指回体首（否则条件永不被求值）。开环先于循环头标签落位：出口
        // 结果寄存器的 undefined 初始化落在回边外（入口执行一次）。
        let v_reg = ctx.push_loop(end_label, cont_label, crate::emit_ctx::LoopKind::Plain);
        let n_labeled = ctx.take_pending_loop_labels(end_label, cont_label, v_reg);
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        let body_result = self.emit_statement(&dw.body, ctx)?;
        // 体正常完成回写（回写点在 `cont_label` 落位前：体内 continue 绕过本点
        // 直达条件位置，不覆写本迭代累积值）。
        if let Some(r) = body_result {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(v_reg), Operand::Reg(r), Operand::None));
        }
        ctx.labels.set_label_pos(cont_label, ctx.insts.len());
        let test_reg = self.emit_expression(&dw.test, ctx)?;
        ctx.inst(Inst::jmp_if_true(test_reg, start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        // 循环完成值 = 出口结果寄存器：入口 undefined、体正常完成每迭代覆写、
        // break/continue 携值写入（空携值不写，保持既有累积值）。
        Ok(Some(v_reg))
    }
}
