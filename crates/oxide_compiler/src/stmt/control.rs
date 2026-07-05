use crate::compiler::{CompileCtx, Compiler, Label};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::Statement;

impl Compiler {
    fn count_if_statement(&self, stmt: &oxide_parser::IfStatement<'_>, ctx: &mut CompileCtx) {
        let id = ctx.next_label_id();
        let else_label = Label::IfElse(id);
        let end_label = Label::IfEnd(id);

        self.count_expression(&stmt.test, ctx);
        ctx.projected_pc += 1;
        self.count_statement(&stmt.consequent, ctx);
        ctx.alloc_reg();
        ctx.projected_pc += 1;
        if stmt.alternate.is_some() {
            ctx.projected_pc += 1;
        }
        ctx.labels.label_map.insert(else_label, ctx.projected_pc);
        if let Some(alt_stmt) = &stmt.alternate {
            self.count_statement(alt_stmt, ctx);
            ctx.projected_pc += 1;
        }
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }

    fn emit_if_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::IfStatement(ifs) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let else_label = Label::IfElse(id);
        let end_label = Label::IfEnd(id);

        let test_reg = self.emit_expression(&ifs.test, ctx)?;

        let else_pos = ctx.resolve_label(else_label)?;
        let offset = (else_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp_if_false(test_reg, offset));

        let cons_reg = self.emit_statement(&ifs.consequent, ctx)?;
        let result_reg = ctx.alloc_reg();
        if let Some(r) = cons_reg {
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.emit_load_const(result_reg, undef_idx);
        }

        if ifs.alternate.is_some() {
            let end_pos = ctx.resolve_label(end_label)?;
            let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
            let offset = ctx.checked_jump_offset(offset);
            ctx.emit(opcode::encode_jmp(offset));
        }

        if let Some(alt) = &ifs.alternate {
            let alt_reg = self.emit_statement(alt, ctx)?;
            if let Some(r) = alt_reg {
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
            } else {
                let undef_idx = ctx.add_constant(Constant::Undefined);
                ctx.emit_load_const(result_reg, undef_idx);
            }
        }

        Ok(Some(result_reg))
    }

    pub(crate) fn count_control_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        if let Statement::IfStatement(ifs) = stmt {
            self.count_if_statement(ifs, ctx);
        }
    }

    pub(crate) fn emit_control_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::IfStatement(_) => self.emit_if_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
