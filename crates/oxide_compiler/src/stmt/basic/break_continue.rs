use crate::compiler::{CompileCtx, Compiler};
use crate::ir::inst::Inst;

impl Compiler {
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
        ctx.inst(Inst::jmp(break_label));
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
        ctx.inst(Inst::jmp(continue_label));
        Ok(None)
    }
}
