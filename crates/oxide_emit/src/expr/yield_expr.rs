//! `yield` 表达式 emit：求值被让出的值后发 `YIELD` 指令，结果经 reg 0 交付。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::YieldExpression;

impl Emitter {
    pub(crate) fn emit_yield_expression(&self, ye: &YieldExpression, ctx: &mut CompileCtx) -> Result<u32, String> {
        // 未支持：`yield*` 委托迭代，后续按 IteratorYield 协议实现。
        if ye.delegate {
            return Err("yield* not yet supported".into());
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
