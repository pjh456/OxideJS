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

    pub(in crate::emitter) fn emit_break_statement(
        &self, stmt: &oxide_parser::BreakStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let break_label = if let Some(label) = &stmt.label {
            let name = label.name.as_str();
            let scope = ctx
                .find_label(name)
                .ok_or_else(|| format!("SyntaxError: Undefined label '{name}'"))?;
            scope.break_label
        } else if let Some(sw_label) = ctx.current_switch() {
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

    pub(in crate::emitter) fn emit_continue_statement(
        &self, stmt: &oxide_parser::ContinueStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let continue_label = if let Some(label) = &stmt.label {
            let name = label.name.as_str();
            let scope = ctx
                .find_label(name)
                .ok_or_else(|| format!("SyntaxError: Undefined label '{name}'"))?;
            scope.continue_label.ok_or_else(|| {
                format!("SyntaxError: Illegal continue statement: '{name}' does not denote an iteration statement")
            })?
        } else {
            let (_, cl) = ctx.current_loop().ok_or("continue outside loop".to_string())?;
            *cl
        };
        let continue_pos = ctx.resolve_label(continue_label)?;
        let offset = (continue_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp(offset));
        Ok(None)
    }

    pub(in crate::emitter) fn emit_labeled_statement(
        &self, stmt: &oxide_parser::LabeledStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let name = stmt.label.name.as_str();
        if Self::is_iteration_statement(&stmt.body) {
            // Bind the label to the loop's own continue/break targets (the loop
            // drains the queue and pops the scope after its body).
            ctx.queue_loop_label(name)?;
            self.emit_statement(&stmt.body, ctx)?;
        } else {
            // Non-loop labeled statement: only `break label` is valid; it targets
            // the LabeledEnd placed (by the count pass) right after the body.
            let id = ctx.next_label_id();
            ctx.push_label_scope(name, Label::LabeledEnd(id), None)?;
            self.emit_statement(&stmt.body, ctx)?;
            ctx.pop_label_scope();
        }
        Ok(None)
    }

    pub(in crate::emitter) fn is_iteration_statement(stmt: &Statement) -> bool {
        matches!(
            stmt,
            Statement::WhileStatement(_)
                | Statement::DoWhileStatement(_)
                | Statement::ForStatement(_)
                | Statement::ForInStatement(_)
                | Statement::ForOfStatement(_)
        )
    }
}
