//! 控制流语句 emit：`if` 条件分支（含 else）与域分发 `emit_control_domain`。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::Statement;

impl Emitter {
    fn emit_if_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::IfStatement(ifs) = stmt else {
            return Ok(None);
        };
        let else_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();

        let test_reg = self.emit_expression(&ifs.test, ctx)?;

        ctx.inst(Inst::jmp_if_false(test_reg, else_label));

        let cons_reg = self.emit_statement(&ifs.consequent, ctx)?;
        let result_reg = ctx.alloc_reg();
        if let Some(r) = cons_reg {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::Reg(r as u32), Operand::None));
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.inst(Inst::load_const(Operand::Reg(result_reg as u32), undef_idx));
        }

        if ifs.alternate.is_some() {
            ctx.inst(Inst::jmp(end_label));
        }

        ctx.labels.set_label_pos(else_label, ctx.insts.len());
        if let Some(alt) = &ifs.alternate {
            let alt_reg = self.emit_statement(alt, ctx)?;
            if let Some(r) = alt_reg {
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::Reg(r as u32), Operand::None));
            } else {
                let undef_idx = ctx.add_constant(Constant::Undefined);
                ctx.inst(Inst::load_const(Operand::Reg(result_reg as u32), undef_idx));
            }
        }

        ctx.labels.set_label_pos(end_label, ctx.insts.len());

        Ok(Some(result_reg))
    }

    pub(crate) fn emit_control_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::IfStatement(_) => self.emit_if_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
