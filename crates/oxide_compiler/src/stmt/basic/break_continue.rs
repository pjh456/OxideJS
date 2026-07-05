use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::opcode;

impl Compiler {
    pub(super) fn count_break_statement(&self, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1;
    }

    pub(super) fn count_continue_statement(&self, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1;
    }

    pub(crate) fn emit_break_statement(
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

    pub(crate) fn emit_continue_statement(
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
}
