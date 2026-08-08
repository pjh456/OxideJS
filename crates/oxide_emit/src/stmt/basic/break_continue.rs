//! break/continue 语句 emit：解析标签与最近循环的跳转目标。
//!
//! 计算 break/continue 逃出的 finally 域数（目标打开时的 finally 深度与当前深度之差）：
//! 逃出 ≥1 个 finally → 发 BREAK/CONTINUE（运行时穿越 finally 后再跳转）；
//! 不逃出（目标在 try 域内，如 switch/循环内部的 break）→ 发普通 JMP。

use crate::{CompileCtx, Emitter};
use oxide_ir::inst::Inst;

impl Emitter {
    pub(crate) fn emit_break_statement(
        &self, stmt: &oxide_parser::BreakStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let (break_label, fd_at_open) = if let Some(label) = &stmt.label {
            let name = label.name.as_str();
            let scope = ctx
                .find_label(name)
                .ok_or_else(|| format!("SyntaxError: Undefined label '{name}'"))?;
            (scope.break_label, scope.finally_depth_at_open)
        } else if let Some((sw_label, fd)) = ctx.current_switch() {
            (*sw_label, *fd)
        } else {
            let (bl, _, fd) = ctx.current_loop().ok_or("break outside switch or loop".to_string())?;
            (*bl, *fd)
        };
        let crossed = ctx.labels.finally_depth.saturating_sub(fd_at_open) as u16;
        if crossed > 0 {
            ctx.inst(Inst::brk(break_label, crossed));
        } else {
            ctx.inst(Inst::jmp(break_label));
        }
        Ok(None)
    }

    pub(crate) fn emit_continue_statement(
        &self, stmt: &oxide_parser::ContinueStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let (continue_label, fd_at_open) = if let Some(label) = &stmt.label {
            let name = label.name.as_str();
            let scope = ctx
                .find_label(name)
                .ok_or_else(|| format!("SyntaxError: Undefined label '{name}'"))?;
            (
                scope.continue_label.ok_or_else(|| {
                    format!("SyntaxError: Illegal continue statement: '{name}' does not denote an iteration statement")
                })?,
                scope.finally_depth_at_open,
            )
        } else {
            let (_, cl, fd) = ctx.current_loop().ok_or("continue outside loop".to_string())?;
            (*cl, *fd)
        };
        let crossed = ctx.labels.finally_depth.saturating_sub(fd_at_open) as u16;
        if crossed > 0 {
            ctx.inst(Inst::cont(continue_label, crossed));
        } else {
            ctx.inst(Inst::jmp(continue_label));
        }
        Ok(None)
    }
}
