use super::*;

impl Compiler {
    pub(in crate::emitter) fn emit_while_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::WhileStatement(wh) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let start_label = Label::WhileStart(id);
        let end_label = Label::WhileEnd(id);

        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);

        let test_reg = self.emit_expression(&wh.test, ctx)?;
        let end_pos = ctx.resolve_label(end_label)?;
        let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp_if_false(test_reg, offset));

        self.emit_statement(&wh.body, ctx)?;

        let start_pos = ctx.resolve_label(start_label)?;
        let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));

        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }

    pub(in crate::emitter) fn emit_do_while_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::DoWhileStatement(dw) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let start_label = Label::DoWhileStart(id);
        let end_label = Label::DoWhileEnd(id);

        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);
        self.emit_statement(&dw.body, ctx)?;
        let test_reg = self.emit_expression(&dw.test, ctx)?;

        let start_pos = ctx.resolve_label(start_label)?;
        let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp_if_true(test_reg, offset));

        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }

    pub(in crate::emitter) fn emit_for_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::ForStatement(fr) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let start_label = Label::ForStart(id);
        let update_label = Label::ForUpdate(id);
        let end_label = Label::ForEnd(id);

        ctx.push_loop(end_label, update_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, update_label);

        if let Some(init) = &fr.init {
            if let Some(expr) = init.as_expression() {
                self.emit_expression(expr, ctx)?;
            } else if let ForStatementInit::VariableDeclaration(decl) = init {
                for d in &decl.declarations {
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    if let Some(init_expr) = &d.init {
                        let val_reg = self.emit_expression(init_expr, ctx)?;
                        self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, ctx)?;
                    } else if let BindingPattern::BindingIdentifier(bi) = &d.id {
                        let var_reg = ctx.alloc_reg();
                        ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const)?;
                    }
                }
            }
        }

        if let Some(test) = &fr.test {
            let test_reg = self.emit_expression(test, ctx)?;
            let end_pos = ctx.resolve_label(end_label)?;
            let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
            let offset = ctx.checked_jump_offset(offset);
            ctx.emit(opcode::encode_jmp_if_false(test_reg, offset));
        }

        self.emit_statement(&fr.body, ctx)?;

        if let Some(update) = &fr.update {
            self.emit_expression(update, ctx)?;
        }

        let start_pos = ctx.resolve_label(start_label)?;
        let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));

        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();

        Ok(None)
    }

    pub(in crate::emitter) fn emit_for_in_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::ForInStatement(fi) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let start_label = Label::ForInStart(id);
        let end_label = Label::ForInEnd(id);

        let obj_reg = self.emit_expression(&fi.right, ctx)?;
        ctx.emit(opcode::encode(OpCode::FOR_IN_INIT, 0, obj_reg, 0));

        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);

        let done_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_IN_DONE, done_reg, 0, 0));

        let end_pos = ctx.resolve_label(end_label)?;
        let cleanup_jmp_offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
        ctx.emit(opcode::encode_jmp_if_false(done_reg, 2));
        let cleanup_jmp_offset = ctx.checked_jump_offset(cleanup_jmp_offset);
        ctx.emit(opcode::encode_jmp(cleanup_jmp_offset));

        let key_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_IN_NEXT, key_reg, 0, 0));

        match &fi.left {
            ForStatementLeft::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    let name = match &d.id {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => bi.name.as_str(),
                        _ => return Err("destructuring not supported".into()),
                    };
                    let var_reg = ctx.alloc_reg();
                    ctx.declare(name, var_reg, decl.kind, matches!(decl.kind, VariableDeclarationKind::Const))?;
                    ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
                    ctx.init_var(name);
                }
            }
            ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                let name = id_ref.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
            }
            _ => return Err("unsupported for-in left-hand side".into()),
        }

        self.emit_statement(&fi.body, ctx)?;

        let start_pos = ctx.resolve_label(start_label)?;
        let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));

        ctx.emit(opcode::encode(OpCode::FOR_IN_CLEANUP, 0, 0, 0));

        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }

    pub(in crate::emitter) fn emit_for_of_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::ForOfStatement(fo) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let start_label = Label::ForOfStart(id);
        let end_label = Label::ForOfEnd(id);

        let iter_src_reg = self.emit_expression(&fo.right, ctx)?;
        ctx.emit(opcode::encode(OpCode::FOR_OF_INIT, 0, iter_src_reg, 0));
        ctx.push_loop(end_label, start_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, start_label);

        let has_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_OF_DONE, has_reg, 0, 0));
        let end_pos = ctx.resolve_label(end_label)?;
        let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp_if_false(has_reg, offset));

        let val_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_OF_NEXT, val_reg, 0, 0));
        match &fo.left {
            ForStatementLeft::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    self.emit_binding_pattern(&d.id, val_reg, decl.kind, false, ctx)?;
                }
            }
            ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                let name = id_ref.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, 0));
            }
            ForStatementLeft::ArrayAssignmentTarget(ap) => {
                self.emit_array_assignment(ap, val_reg, ctx)?;
            }
            ForStatementLeft::ObjectAssignmentTarget(op) => {
                self.emit_object_assignment(op, val_reg, ctx)?;
            }
            _ => return Err("unsupported for-of left-hand side".into()),
        }

        self.emit_statement(&fo.body, ctx)?;
        let start_pos = ctx.resolve_label(start_label)?;
        let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));
        ctx.emit(opcode::encode(OpCode::FOR_OF_CLOSE, 0, 0, 0));
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }
}
