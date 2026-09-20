//! break/continue 语句 emit：解析标签与最近循环的跳转目标，发射前按完成值帧
//! 栈解析携值并写入出口目标寄存器。
//!
//! 计算 break/continue 逃出的 finally 域数（目标打开时的 finally 深度与当前深度之差）
//! 与逃出的迭代器层数（for-of/for-in 深度之差，目标循环自身的关闭由 CLOSE /
//! FOR_IN_CLEANUP 处理，不计入）：逃出 ≥1 个 finally 域或 ≥1 层迭代器 → 发
//! BREAK/CONTINUE（运行时穿越 finally、关闭逃出迭代器后再跳转）；否则发普通 JMP。

use crate::emit_ctx::CompletionCarry;
use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;

impl Emitter {
    /// break/continue 的携值写入：自完成值帧栈顶解析携值，非空携值在跳转前
    /// 写入出口目标寄存器——`Value(r)` 拷寄存器、`Undefined` 物化 undefined、
    /// `Empty` 不写（目标寄存器保持既有累积值，规范
    /// `UpdateEmpty(break, iterationResult)` 等义）。
    fn emit_completion_carry(&self, ctx: &mut CompileCtx, target_v_reg: u32) {
        match ctx.resolve_completion_carry(target_v_reg) {
            CompletionCarry::Value(r) => {
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(target_v_reg), Operand::Reg(r), Operand::None));
            }
            CompletionCarry::Undefined => {
                let u = self.emit_undefined(ctx);
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(target_v_reg), Operand::Reg(u), Operand::None));
            }
            CompletionCarry::Empty => {}
        }
    }

    pub(crate) fn emit_break_statement(
        &self, stmt: &oxide_parser::BreakStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let (break_label, fd_at_open, fod_at_open, fid_at_open, target_v_reg) = if let Some(label) = &stmt.label {
            let name = label.name.as_str();
            let scope = ctx
                .find_label(name)
                .ok_or_else(|| format!("SyntaxError: Undefined label '{name}'"))?;
            (
                scope.break_label,
                scope.finally_depth_at_open,
                scope.for_of_depth_at_open,
                scope.for_in_depth_at_open,
                scope.completion_reg,
            )
        } else if let Some(sw) = ctx.current_switch() {
            // switch 内 break：目标为 switch 出口。逃出计数以 switch 打开时的
            // 深度快照为基准——switch 之前已打开的循环不属于本次逃出，仅当
            // case 内新打开了迭代循环（并逃出）时才需关闭。
            (
                sw.break_label,
                sw.finally_depth_at_open,
                sw.for_of_depth_at_open,
                sw.for_in_depth_at_open,
                Some(sw.result_reg),
            )
        } else {
            let entry = ctx.current_loop().ok_or("break outside switch or loop".to_string())?;
            (
                entry.break_label,
                entry.finally_depth_at_open,
                entry.for_of_depth_at_open,
                entry.for_in_depth_at_open,
                Some(entry.v_reg),
            )
        };
        if let Some(target_v_reg) = target_v_reg {
            self.emit_completion_carry(ctx, target_v_reg);
        }
        let crossed = ctx.labels.finally_depth.saturating_sub(fd_at_open) as u16;
        let (for_of_count, for_in_count) = escape_iter_counts(ctx, fod_at_open, fid_at_open);
        if crossed > 0 || for_of_count > 0 || for_in_count > 0 {
            ctx.inst(Inst::brk(break_label, crossed, for_of_count, for_in_count));
        } else {
            ctx.inst(Inst::jmp(break_label));
        }
        Ok(None)
    }

    pub(crate) fn emit_continue_statement(
        &self, stmt: &oxide_parser::ContinueStatement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let (continue_label, fd_at_open, fod_at_open, fid_at_open, target_v_reg) = if let Some(label) = &stmt.label {
            let name = label.name.as_str();
            let scope = ctx
                .find_label(name)
                .ok_or_else(|| format!("SyntaxError: Undefined label '{name}'"))?;
            (
                scope.continue_label.ok_or_else(|| {
                    format!("SyntaxError: Illegal continue statement: '{name}' does not denote an iteration statement")
                })?,
                scope.finally_depth_at_open,
                scope.for_of_depth_at_open,
                scope.for_in_depth_at_open,
                scope.completion_reg,
            )
        } else {
            let entry = ctx.current_loop().ok_or("continue outside loop".to_string())?;
            (
                entry.continue_label,
                entry.finally_depth_at_open,
                entry.for_of_depth_at_open,
                entry.for_in_depth_at_open,
                Some(entry.v_reg),
            )
        };
        if let Some(target_v_reg) = target_v_reg {
            self.emit_completion_carry(ctx, target_v_reg);
        }
        let crossed = ctx.labels.finally_depth.saturating_sub(fd_at_open) as u16;
        let (for_of_count, for_in_count) = escape_iter_counts(ctx, fod_at_open, fid_at_open);
        if crossed > 0 || for_of_count > 0 || for_in_count > 0 {
            ctx.inst(Inst::cont(continue_label, crossed, for_of_count, for_in_count));
        } else {
            ctx.inst(Inst::jmp(continue_label));
        }
        Ok(None)
    }
}

/// 计算逃出目标循环需要关闭的 for-of / for-in 迭代器层数。
///
/// 深度快照（`fod_at_open`/`fid_at_open`）取目标循环打开后（含自身）的深度：
/// 当前深度与之差 = 被逃出的中间层数。目标循环自身的关闭由循环收尾指令
/// （FOR_OF_CLOSE / FOR_IN_CLEANUP）处理，不计入——break 的跳转目标在收尾指令
/// 之后（含其 CLOSE），continue 则目标循环继续迭代。
fn escape_iter_counts(ctx: &CompileCtx, fod_at_open: usize, fid_at_open: usize) -> (usize, usize) {
    let for_of_count = ctx.labels.for_of_depth.saturating_sub(fod_at_open);
    let for_in_count = ctx.labels.for_in_depth.saturating_sub(fid_at_open);
    (for_of_count, for_in_count)
}
