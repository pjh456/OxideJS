use crate::compiler::{CompileCtx, Compiler, Label};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::Statement;

impl Compiler {
    fn emit_if_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::IfStatement(ifs) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let else_label = Label::IfElse(id);
        let end_label = Label::IfEnd(id);

        let test_reg = self.emit_expression(&ifs.test, ctx)?;

        ctx.emit_jmp_if_false_labeled(test_reg, else_label);

        let cons_reg = self.emit_statement(&ifs.consequent, ctx)?;
        let result_reg = ctx.alloc_reg();
        if let Some(r) = cons_reg {
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.emit_load_const(result_reg, undef_idx);
        }

        if ifs.alternate.is_some() {
            ctx.emit_jmp_labeled(end_label);
        }

        ctx.labels.label_map.insert(else_label, ctx.bytecode.len());
        if let Some(alt) = &ifs.alternate {
            let alt_reg = self.emit_statement(alt, ctx)?;
            if let Some(r) = alt_reg {
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
            } else {
                let undef_idx = ctx.add_constant(Constant::Undefined);
                ctx.emit_load_const(result_reg, undef_idx);
            }
        }

        ctx.labels.label_map.insert(end_label, ctx.bytecode.len());

        Ok(Some(result_reg))
    }

    pub(crate) fn emit_control_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::IfStatement(_) => self.emit_if_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
