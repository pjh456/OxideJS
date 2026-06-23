use super::*;

impl Compiler {
    pub(in crate::emitter) fn emit_expression_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::ExpressionStatement(es) = stmt else {
            return Ok(None);
        };
        Ok(Some(self.emit_expression(&es.expression, ctx)?))
    }

    pub(in crate::emitter) fn emit_return_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::ReturnStatement(ret) = stmt else {
            return Ok(None);
        };
        match &ret.argument {
            Some(expr) => {
                let r = self.emit_expression(expr, ctx)?;
                ctx.emit(opcode::encode(OpCode::RETURN, r, 0, 0));
            }
            None => {
                ctx.emit(opcode::encode(OpCode::RETURN, 0, 0, 0));
            }
        }
        Ok(None)
    }

    pub(in crate::emitter) fn emit_break_statement(&self, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let break_label = if let Some(sw_label) = ctx.current_switch() {
            *sw_label
        } else {
            let (bl, _) = ctx.current_loop().ok_or("break outside switch or loop".to_string())?;
            *bl
        };
        let break_pos = ctx.resolve_label(break_label)?;
        let offset = (break_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));
        Ok(None)
    }

    pub(in crate::emitter) fn emit_continue_statement(&self, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let (_, continue_label) = ctx.current_loop().ok_or("continue outside loop".to_string())?;
        let continue_pos = ctx.resolve_label(*continue_label)?;
        let offset = (continue_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));
        Ok(None)
    }
}
