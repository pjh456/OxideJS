//! `yield` 表达式 emit：求值被让出的值后发 `YIELD` 指令，结果经 reg 0 交付。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::YieldExpression;

impl Emitter {
    pub(crate) fn emit_yield_expression(&self, ye: &YieldExpression, ctx: &mut CompileCtx) -> Result<u32, String> {
        // `yield*` 委托：求值内层可迭代对象后发 YIELD_STAR，运行时转发 next/return/throw。
        // 委托完成值（内层 done 的 value）经 reg 0 交付，与普通 `yield` 同协议。
        if ye.delegate {
            let arg = ye.argument.as_ref().ok_or_else(|| "yield* requires an argument".to_string())?;
            let inner_reg = self.emit_expression(arg, ctx)?;
            ctx.inst(Inst::yield_star(Operand::Reg(inner_reg)));
            let result_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::None, Operand::None));
            return Ok(result_reg);
        }
        // 求值被让出的值（`yield` 无参数时让出 undefined）。
        let value_reg = if let Some(arg) = &ye.argument {
            self.emit_expression(arg, ctx)?
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            let r = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(r), undef_idx));
            r
        };
        ctx.inst(Inst::yield_value(Operand::Reg(value_reg)));
        // 恢复时 `next(v)` 的 v 经 reg 0 交付（与 CALL 同协议）。
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::None, Operand::None));
        Ok(result_reg)
    }
}
