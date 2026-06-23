use super::*;

impl Compiler {
    pub(in crate::emitter) fn emit_throw_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::ThrowStatement(ts) = stmt else {
            return Ok(None);
        };
        let exc_reg = self.emit_expression(&ts.argument, ctx)?;
        ctx.emit(opcode::encode(OpCode::THROW, exc_reg, 0, 0));
        Ok(None)
    }

    pub(in crate::emitter) fn emit_try_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::TryStatement(ts) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let catch_label = Label::CatchBody(id);
        let try_end_label = Label::TryEnd(id);
        let has_catch = ts.handler.is_some();
        let has_finally = ts.finalizer.is_some();

        let result_reg = ctx.alloc_reg();
        let mut try_finally_begin_pos: Option<usize> = None;
        let mut try_begin_pos: Option<usize> = None;

        if has_finally {
            try_finally_begin_pos = Some(ctx.bytecode.len());
            ctx.emit(opcode::encode_try_finally_begin(0));
        }

        if has_catch {
            try_begin_pos = Some(ctx.bytecode.len());
            ctx.emit(opcode::encode_try_begin(0));
        }

        let mut last_try_result: Option<u8> = None;
        for s in &ts.block.body {
            if let Some(r) = self.emit_statement(s, ctx)? {
                last_try_result = Some(r);
            }
        }
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, last_try_result.unwrap_or(result_reg), 0));

        if has_catch {
            ctx.emit(opcode::encode(OpCode::TRY_END, 0, 0, 0));
        }

        let jmp_needed = has_catch || has_finally;
        let jmp_skip_pos = if jmp_needed {
            let pos = ctx.bytecode.len();
            ctx.emit(opcode::encode_jmp(0));
            Some(pos)
        } else {
            None
        };

        let catch_label_pc = ctx.bytecode.len();
        ctx.label_map.insert(catch_label, catch_label_pc);

        if let Some(try_begin_pc) = try_begin_pos {
            let offset = catch_label_pc as isize - (try_begin_pc as isize);
            let offset = ctx.checked_jump_offset(offset);
            ctx.bytecode[try_begin_pc] = opcode::encode_try_begin(offset);
        }

        if let Some(catch) = &ts.handler {
            ctx.push_scope();
            if let Some(param) = &catch.param {
                let catch_reg = ctx.alloc_reg();
                if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                    ctx.declare_initialized(bi.name.as_str(), catch_reg, VariableDeclarationKind::Let, false)?;
                    ctx.emit(opcode::encode(OpCode::STORE_VAR, catch_reg, 0, 0));
                }
            }
            let mut last_catch_result: Option<u8> = None;
            for s in &catch.body.body {
                if let Some(r) = self.emit_statement(s, ctx)? {
                    last_catch_result = Some(r);
                }
            }
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, last_catch_result.unwrap_or(result_reg), 0));
            ctx.pop_scope();
        }

        if has_finally {
            let finally_label = Label::FinallyBody(id);
            let finally_label_pc = ctx.bytecode.len();
            ctx.label_map.insert(finally_label, finally_label_pc);
            if let Some(fb_pos) = try_finally_begin_pos {
                let offset = finally_label_pc as isize - (fb_pos as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.bytecode[fb_pos] = opcode::encode_try_finally_begin(offset);
            }

            if let Some(jmp_pos) = jmp_skip_pos {
                let offset = finally_label_pc as isize - (jmp_pos as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.bytecode[jmp_pos] = opcode::encode_jmp(offset);
            }

            let mut last_finally_result: Option<u8> = None;
            for s in &ts.finalizer.as_ref().unwrap().body {
                if let Some(r) = self.emit_statement(s, ctx)? {
                    last_finally_result = Some(r);
                }
            }
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, last_finally_result.unwrap_or(result_reg), 0));
            ctx.emit(opcode::encode(OpCode::TRY_FINALLY_END, 0, 0, 0));
        } else if let Some(jmp_pos) = jmp_skip_pos {
            let offset = ctx.bytecode.len() as isize - (jmp_pos as isize);
            let offset = ctx.checked_jump_offset(offset);
            ctx.bytecode[jmp_pos] = opcode::encode_jmp(offset);
        }

        let try_end_pc = ctx.bytecode.len();
        ctx.label_map.insert(try_end_label, try_end_pc);

        Ok(Some(result_reg))
    }
}
