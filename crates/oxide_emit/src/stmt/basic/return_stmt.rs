//! return 语句 emit：`emit_return_statement` 求值返回值并生成返回指令。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::Statement;

impl Emitter {
    fn emit_return_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let Statement::ReturnStatement(ret) = stmt else {
            return Ok(None);
        };
        match &ret.argument {
            Some(expr) => {
                let r = self.emit_expression(expr, ctx)?;
                // return 逃出本函数全部 try 域：值求值后、发 RETURN 前，先从栈顶
                // 弹出连续纯 catch handler（finally handler 走运行时完成穿越逐个
                // 执行）。否则 return 跳过 TRY_END 会让 handler 残留在 try_stack，
                // 后续异常 unwind 会跳回已返回函数的 catch 形成死循环。
                self.emit_return_try_cleanup(ctx);
                ctx.inst(Inst::new(OpCode::RETURN, Operand::Reg(r), Operand::None, Operand::None));
            }
            None => {
                self.emit_return_try_cleanup(ctx);
                ctx.inst(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));
            }
        }
        Ok(None)
    }

    /// 在 RETURN 前弹出栈顶连续打开的纯 catch handler。
    ///
    /// # 边界与前提
    /// - 只处理位于栈顶的连续纯 catch handler；finally handler 及其下方的 catch
    ///   handler 不由 TRY_END 弹出（LIFO 无法跨 finally 弹出），由运行时
    ///   dispatch_return 扫描兜底清理。
    fn emit_return_try_cleanup(&self, ctx: &mut CompileCtx) {
        let open_catches = ctx.top_open_catch_handlers();
        for _ in 0..open_catches {
            ctx.inst(Inst::new(OpCode::TRY_END, Operand::None, Operand::None, Operand::None));
        }
    }

    pub(crate) fn emit_basic_return(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        self.emit_return_statement(stmt, ctx)
    }
}
