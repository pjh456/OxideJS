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
            // finally 域跨整个语句（try 体 + catch 体 + finally 体）：期间 break/continue
            // 逃出语句时须穿越本 finally，词法上记录域深度供跨越计数。
            ctx.push_finally_domain();
            ctx.inst(Inst::try_finally_begin(finally_label));
            ctx.push_open_finally_handler();
        }
        if has_catch {
            ctx.inst(Inst::try_begin(catch_label));
            ctx.push_open_catch_handler();
        }
        let mut last_try_result: Option<u32> = None;
        // try block 是独立块作用域：lexical 声明（let/const/class）限定于 try 内，
        // 与外层同名绑定互不干扰；块级预声明使声明点前读取编译为 TDZ 抛错。
        ctx.push_scope();
        self.predeclare_lexical_declarations(&ts.block.body, ctx);
        for s in &ts.block.body {
            if let Some(r) = self.emit_statement(s, ctx)? {
                last_try_result = Some(r);
            }
        }
        ctx.pop_scope();
        ctx.inst(Inst::new(
            OpCode::LOAD_VAR,
            Operand::Reg(result_reg),
            Operand::Reg(last_try_result.unwrap_or(result_reg)),
            Operand::None,
        ));
        if has_catch {
            ctx.inst(Inst::new(OpCode::TRY_END, Operand::None, Operand::None, Operand::None));
            ctx.pop_open_try_handler();
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
                let src_reg = ctx.alloc_reg();
                // 异常值在 VM 物理 regs[0]（unwind 展开处写入），STORE_VAR a=None 读回。
                ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(src_reg), Operand::None, Operand::None));
                self.emit_binding_pattern(&param.pattern, src_reg, VariableDeclarationKind::Let, false, false, ctx)?;
            }
            self.predeclare_lexical_declarations(&catch.body.body, ctx);
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
            // finally 体入口标记：运行时置位栈顶 handler 的 finally_active。所有进入
            // 路径（try/catch 体 JMP、catch 体直落、unwind、完成穿越、挂起恢复）都
            // 落到 finally_label 起始的这条标记，统一完成置位；catch 体直落路径此前
            // 无任何置位，finally 内 break/continue/return/throw 会重入 finally 体。
            ctx.inst(Inst::try_finally_enter());
            let mut last_finally_result: Option<u32> = None;
            // finally block 同为独立块作用域，lexical 声明互不泄漏。
            ctx.push_scope();
            self.predeclare_lexical_declarations(&ts.finalizer.as_ref().unwrap().body, ctx);
            for s in &ts.finalizer.as_ref().unwrap().body {
                if let Some(r) = self.emit_statement(s, ctx)? {
                    last_finally_result = Some(r);
                }
            }
            ctx.pop_scope();
            ctx.inst(Inst::new(
                OpCode::LOAD_VAR,
                Operand::Reg(result_reg),
                Operand::Reg(last_finally_result.unwrap_or(result_reg)),
                Operand::None,
            ));
            ctx.inst(Inst::new(OpCode::TRY_FINALLY_END, Operand::None, Operand::None, Operand::None));
            ctx.pop_open_try_handler();
            ctx.pop_finally_domain();
        }
        ctx.labels.set_label_pos(try_end_label, ctx.insts.len());
        Ok(Some(result_reg))
    }
}
