//! switch 语句 emit：case 匹配链 + 穿落（fallthrough）与 default，见 `emit_switch_statement`。
//! CaseBlock 有独立块作用域：case 选择表达式与全部 case 体共享同一环境，case 内
//! 函数/词法声明不泄漏到 switch 外。

use std::collections::HashMap;

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
        // CaseBlock 出口结果寄存器：入口初始 undefined（覆盖无命中/空块路径），
        // 每条子句的非空语句完成值逐语句覆写（与规范逐子句 `UpdateEmpty`
        // 累积终态等价：空子句不覆写前值）；abrupt 出口不覆写，保留前子句
        // 累积值。
        let result_reg = ctx.alloc_reg();
        let undef = self.emit_undefined(ctx);
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::Reg(undef), Operand::None));
        ctx.push_switch(end_label, result_reg);
        ctx.push_completion_target(result_reg);
        // 判别式在 switch 外层词法环境求值，CaseBlock 环境尚未建立。
        let disc_reg = self.emit_expression(&sw.discriminant, ctx)?;
        let cases = &sw.cases;
        // CaseBlock 环境：case 选择表达式与全部 case 体同处一个块作用域。
        // 块函数预声明先于 lexical，使 `case 0: let g; function g(){}` 的 lexical
        // 占位命中已存在的函数绑定，报重复声明错。
        ctx.push_scope();
        for case in cases.iter() {
            self.predeclare_block_function_declarations(&case.consequent, ctx, false);
            self.predeclare_lexical_declarations(&case.consequent, ctx, false)?;
        }
        // case 内函数声明的块入口初始化：同名重复声明按源序物化，末次声明成为
        // 入口值；声明点复用块槽写回外层 var（见 emit_function_declaration）。
        ctx.block_fn_entry_mats.push(HashMap::new());
        for case in cases.iter() {
            for s in &case.consequent {
                self.emit_block_fn_entry_init_stmt(s, ctx)?;
            }
        }
        let mut case_labels = Vec::with_capacity(cases.len());
        for case in cases.iter() {
            let case_label = ctx.next_label_id();
            case_labels.push(case_label);
            if let Some(test) = &case.test {
                let test_reg = self.emit_expression(test, ctx)?;
                let eq_reg = ctx.alloc_reg();
                // case 选择式与判别式按严格相等比较（不做类型转换）。
                ctx.inst(Inst::new(
                    OpCode::STRICT_EQ,
                    Operand::Reg(eq_reg),
                    Operand::Reg(disc_reg),
                    Operand::Reg(test_reg),
                ));
                ctx.inst(Inst::jmp_if_true(eq_reg, case_label));
            }
        }
        // 无 case 命中时跳向 default 体，无 default 则跳到 switch 末尾。default 不在
        // 源序首位时不能依赖穿落，否则会错误进入首个 case 体。
        let default_label = cases.iter().position(|c| c.test.is_none()).map(|idx| case_labels[idx]);
        match default_label {
            Some(label) => ctx.inst(Inst::jmp(label)),
            None => ctx.inst(Inst::jmp(end_label)),
        }
        for (case_idx, case) in cases.iter().enumerate() {
            let case_label = case_labels[case_idx];
            ctx.labels.set_label_pos(case_label, ctx.insts.len());
            // 子句体不经 `emit_block_statement`，其语句列表须自压列表帧：
            // 非空语句记累积并逐语句覆写出口寄存器（运行期 resultValue）。
            ctx.push_completion_list();
            for s in &case.consequent {
                if let Some(r) = self.emit_statement(s, ctx)? {
                    ctx.set_completion_last(r);
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::Reg(r), Operand::None));
                }
            }
            ctx.pop_completion_list();
        }
        ctx.block_fn_entry_mats.pop();
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_scope();
        ctx.pop_completion_target();
        ctx.pop_switch();
        Ok(Some(result_reg))
    }

    pub(crate) fn emit_switch_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        match stmt {
            Statement::SwitchStatement(_) => self.emit_switch_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
