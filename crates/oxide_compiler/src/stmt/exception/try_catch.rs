use crate::compiler::{CompileCtx, Compiler, Label};
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{Statement, VariableDeclarationKind};

impl Compiler {
    pub(crate) fn count_try_statement(&self, stmt: &oxide_parser::TryStatement<'_>, ctx: &mut CompileCtx) {
        let id = ctx.next_label_id();
        let catch_label = Label::CatchBody(id);
        let try_end_label = Label::TryEnd(id);
        let has_catch = stmt.handler.is_some();
        let has_finally = stmt.finalizer.is_some();
        ctx.alloc_reg();
        if has_finally {
            ctx.projected_pc += 1;
        }
        if has_catch {
            ctx.projected_pc += 1;
        }
        for s in &stmt.block.body {
            self.count_statement(s, ctx);
        }
        ctx.projected_pc += 1;
        if has_catch {
            ctx.projected_pc += 1;
        }
        let jmp_needed = has_catch || has_finally;
        if jmp_needed {
            ctx.projected_pc += 1;
        }
        ctx.labels.label_map.insert(catch_label, ctx.projected_pc);
        if let Some(catch) = &stmt.handler {
            ctx.push_scope();
            if let Some(param) = &catch.param {
                ctx.alloc_reg();
                if let oxide_parser::BindingPattern::BindingIdentifier(_) = &param.pattern {
                    ctx.projected_pc += 1;
                }
            }
            for cs in &catch.body.body {
                self.count_statement(cs, ctx);
            }
            ctx.projected_pc += 1;
            ctx.pop_scope();
        }
        if let Some(finally) = &stmt.finalizer {
            let finally_label = Label::FinallyBody(id);
            ctx.labels.label_map.insert(finally_label, ctx.projected_pc);
            for fs in &finally.body {
                self.count_statement(fs, ctx);
            }
            ctx.projected_pc += 1;
            ctx.projected_pc += 1;
        }
        ctx.labels.label_map.insert(try_end_label, ctx.projected_pc);
    }

    pub(crate) fn emit_try_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
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
        if has_finally {
            try_finally_begin_pos = Some(ctx.bytecode.len());
            ctx.emit(opcode::encode_try_finally_begin(0));
        }
        if has_catch {
            ctx.emit_try_begin_labeled(catch_label);
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
        if jmp_needed {
            let target = if has_finally { Label::FinallyBody(id) } else { try_end_label };
            ctx.emit_jmp_labeled(target);
        }
        let catch_label_pc = ctx.bytecode.len();
        ctx.labels.label_map.insert(catch_label, catch_label_pc);
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
            ctx.labels.label_map.insert(finally_label, finally_label_pc);
            if let Some(fb_pos) = try_finally_begin_pos {
                let offset = finally_label_pc as isize - (fb_pos as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.bytecode[fb_pos] = opcode::encode_try_finally_begin(offset);
            }
            let mut last_finally_result: Option<u8> = None;
            for s in &ts.finalizer.as_ref().unwrap().body {
                if let Some(r) = self.emit_statement(s, ctx)? {
                    last_finally_result = Some(r);
                }
            }
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, last_finally_result.unwrap_or(result_reg), 0));
            ctx.emit(opcode::encode(OpCode::TRY_FINALLY_END, 0, 0, 0));
        }
        let try_end_pc = ctx.bytecode.len();
        ctx.labels.label_map.insert(try_end_label, try_end_pc);
        Ok(Some(result_reg))
    }
}
