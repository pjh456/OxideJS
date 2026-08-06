//! 数组字面量 emit：`emit_array_expression` 逐元素求值并建数组（含 holes）。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
impl Emitter {
    pub(crate) fn emit_array_expression(
        &self, arr: &oxide_parser::ArrayExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let arr_reg = ctx.alloc_reg();
        let n = arr.elements.len() as u16;
        ctx.inst(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(arr_reg), Operand::Imm(n), Operand::None));
        for (i, elem) in arr.elements.iter().enumerate() {
            let Some(e) = elem.as_expression() else {
                return Err("spread not supported".into());
            };
            let val_reg = self.emit_expression(e, ctx)?;
            let idx_reg = ctx.alloc_reg();
            let idx = ctx.add_constant(Constant::Int(i as i32));
            ctx.inst(Inst::load_const(Operand::Reg(idx_reg), idx));
            ctx.inst(Inst::new(
                OpCode::SET_ELEM,
                Operand::Reg(arr_reg),
                Operand::Reg(idx_reg),
                Operand::Reg(val_reg),
            ));
        }
        Ok(arr_reg)
    }
}
