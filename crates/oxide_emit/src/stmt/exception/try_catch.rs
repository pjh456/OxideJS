//! try/catch/finally 语句 emit：`emit_try_statement` 生成 TRY_BEGIN 与处理入口。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::{Statement, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn emit_try_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::TryStatement(ts) = stmt else {
            return Ok(None);
        };
        let catch_label = ctx.next_label_id();
        let finally_label = ctx.next_label_id();
        let try_end_label = ctx.next_label_id();
        let has_catch = ts.handler.is_some();
        let has_finally = ts.finalizer.is_some();
        let result_reg = ctx.alloc_reg();
        if has_finally {
            ctx.inst(Inst::try_finally_begin(finally_label));
        }
        if has_catch {
            ctx.inst(Inst::try_begin(catch_label));
        }
        let mut last_try_result: Option<u32> = None;
        for s in &ts.block.body {
            if let Some(r) = self.emit_statement(s, ctx)? {
                last_try_result = Some(r);
            }
        }
        ctx.inst(Inst::new(
            OpCode::LOAD_VAR,
            Operand::Reg(result_reg),
            Operand::Reg(last_try_result.unwrap_or(result_reg)),
            Operand::None,
        ));
        if has_catch {
            ctx.inst(Inst::new(OpCode::TRY_END, Operand::None, Operand::None, Operand::None));
        }
        let jmp_needed = has_catch || has_finally;
        if jmp_needed {
            let target = if has_finally { finally_label } else { try_end_label };
            ctx.inst(Inst::jmp(target));
        }
        ctx.labels.set_label_pos(catch_label, ctx.insts.len());
        if let Some(catch) = &ts.handler {
            ctx.push_scope();
            if let Some(param) = &catch.param {
                let catch_reg = ctx.alloc_reg();
                if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                    ctx.declare_initialized(bi.name.as_str(), catch_reg, VariableDeclarationKind::Let, false)?;
                    ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(catch_reg), Operand::None, Operand::None));
                }
            }
            let mut last_catch_result: Option<u32> = None;
            for s in &catch.body.body {
                if let Some(r) = self.emit_statement(s, ctx)? {
                    last_catch_result = Some(r);
                }
            }
            ctx.inst(Inst::new(
                OpCode::LOAD_VAR,
                Operand::Reg(result_reg),
                Operand::Reg(last_catch_result.unwrap_or(result_reg)),
                Operand::None,
            ));
            ctx.pop_scope();
        }
        if has_finally {
            ctx.labels.set_label_pos(finally_label, ctx.insts.len());
            let mut last_finally_result: Option<u32> = None;
            for s in &ts.finalizer.as_ref().unwrap().body {
                if let Some(r) = self.emit_statement(s, ctx)? {
                    last_finally_result = Some(r);
                }
            }
            ctx.inst(Inst::new(
                OpCode::LOAD_VAR,
                Operand::Reg(result_reg),
                Operand::Reg(last_finally_result.unwrap_or(result_reg)),
                Operand::None,
            ));
            ctx.inst(Inst::new(OpCode::TRY_FINALLY_END, Operand::None, Operand::None, Operand::None));
        }
        ctx.labels.set_label_pos(try_end_label, ctx.insts.len());
        Ok(Some(result_reg))
    }
}
