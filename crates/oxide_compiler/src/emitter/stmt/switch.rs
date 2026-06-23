use super::*;

impl Compiler {
    pub(in crate::emitter) fn emit_switch_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::SwitchStatement(sw) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let end_label = Label::SwitchEnd(id);
        ctx.push_switch(end_label);

        let disc_reg = self.emit_expression(&sw.discriminant, ctx)?;
        let compare_reg_checkpoint = ctx.reg_checkpoint();
        let cases = &sw.cases;

        for (case_idx, case) in cases.iter().enumerate() {
            if let Some(test) = &case.test {
                let test_reg = self.emit_expression(test, ctx)?;
                let eq_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::EQ, eq_reg, disc_reg, test_reg));

                let case_label = Label::SwitchCase(id, case_idx as u32);
                let body_pos = ctx.resolve_label(case_label)?;
                let offset = (body_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_true(eq_reg, offset));
                ctx.restore_reg_checkpoint(compare_reg_checkpoint);
            }
        }

        let has_default = cases.iter().any(|c| c.test.is_none());
        if !has_default {
            let end_pos = ctx.resolve_label(end_label)?;
            let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
            let offset = ctx.checked_jump_offset(offset);
            ctx.emit(opcode::encode_jmp(offset));
        }

        for case in cases.iter() {
            for s in &case.consequent {
                self.emit_statement(s, ctx)?;
            }
        }

        ctx.pop_switch();
        Ok(None)
    }
}
