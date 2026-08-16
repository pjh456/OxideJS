//! `await` 表达式 emit：求值被等待的值后发 `AWAIT` 指令，结果经 reg 0 交付。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::AwaitExpression;

impl Emitter {
    pub(crate) fn emit_await_expression(&self, ae: &AwaitExpression, ctx: &mut CompileCtx) -> Result<u32, String> {
        // 求值被等待的值（`await undefined` 时求值 undefined）。
        let value_reg = self.emit_expression(&ae.argument, ctx)?;
        ctx.inst(Inst::await_expr(Operand::Reg(value_reg)));
        // promise settle 后恢复值经 reg 0 交付（与 YIELD/CALL 同协议）。
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::None, Operand::None));
        Ok(result_reg)
    }
}
