use crate::compiler::{CompileCtx, Compiler, Label};
use oxide_bytecode::opcode;
use oxide_parser::Statement;

impl Compiler {
    pub(crate) fn count_while_statement(&self, stmt: &oxide_parser::WhileStatement<'_>, ctx: &mut CompileCtx) {
        let id = ctx.next_label_id();
        let start_label = Label::WhileStart(id);
        let end_label = Label::WhileEnd(id);
        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        self.count_expression(&stmt.test, ctx);
        ctx.projected_pc += 1;
        self.count_statement(&stmt.body, ctx);
        ctx.projected_pc += 1;
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }

    pub(crate) fn emit_while_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
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
}
