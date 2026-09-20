//! while 语句 emit：`emit_while_statement` 先测条件后执行体。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Statement;

impl Emitter {
    pub(crate) fn emit_while_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::WhileStatement(wh) = stmt else {
            return Ok(None);
        };
        let start_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        // 开环先于循环头标签落位：出口结果寄存器的 undefined 初始化须落在回边
        // 外（入口执行一次），否则每迭代重置、末迭代出口读 undefined。
        let v_reg = ctx.push_loop(end_label, start_label, crate::emit_ctx::LoopKind::Plain);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label, v_reg);
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        let test_reg = self.emit_expression(&wh.test, ctx)?;
        ctx.inst(Inst::jmp_if_false(test_reg, end_label));
        let body_result = self.emit_statement(&wh.body, ctx)?;
        // 体正常完成回写（回写点在 `jmp start` 前：体内 continue 绕过本点直达
        // 循环头，不覆写本迭代累积值）。
        if let Some(r) = body_result {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(v_reg), Operand::Reg(r), Operand::None));
        }
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        // 循环完成值 = 出口结果寄存器：入口 undefined、体正常完成每迭代覆写、
        // break/continue 携值写入（空携值不写，保持既有累积值）。
        Ok(Some(v_reg))
    }
}
