//! switch 语句 emit：case 匹配链 + 穿落（fallthrough）与 default，见 `emit_switch_statement`。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Statement;

impl Emitter {
    fn emit_switch_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::SwitchStatement(sw) = stmt else {
            return Ok(None);
        };
        let end_label = ctx.next_label_id();
        ctx.push_switch(end_label);
        let disc_reg = self.emit_expression(&sw.discriminant, ctx)?;
        let cases = &sw.cases;
        let mut case_labels = Vec::with_capacity(cases.len());
        for case in cases.iter() {
            let case_label = ctx.next_label_id();
            case_labels.push(case_label);
            if let Some(test) = &case.test {
                let test_reg = self.emit_expression(test, ctx)?;
                let eq_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::EQ,
                    Operand::Reg(eq_reg),
                    Operand::Reg(disc_reg),
                    Operand::Reg(test_reg),
                ));
                ctx.inst(Inst::jmp_if_true(eq_reg, case_label));
            }
        }
        let has_default = cases.iter().any(|c| c.test.is_none());
        if !has_default {
            ctx.inst(Inst::jmp(end_label));
        }
        let mut last: Option<u32> = None;
        for (case_idx, case) in cases.iter().enumerate() {
            let case_label = case_labels[case_idx];
            ctx.labels.set_label_pos(case_label, ctx.insts.len());
            for s in &case.consequent {
                if let Some(r) = self.emit_statement(s, ctx)? {
                    last = Some(r);
                }
            }
        }
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_switch();
        // switch 完成值 = 已执行 case 链最后一次非空完成值（源序最后非空语句的
        // 收敛寄存器：命中后穿落到 break/尾即为其值，无命中则该寄存器不被写入而
        // 持 undefined）；无命中物化 undefined 作为带值完成返回。
        let result = match last {
            Some(r) => r,
            None => self.emit_undefined(ctx),
        };
        Ok(Some(result))
    }

    pub(crate) fn emit_switch_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        match stmt {
            Statement::SwitchStatement(_) => self.emit_switch_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
