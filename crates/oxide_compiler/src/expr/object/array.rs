use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::{
    module::Constant,
    opcode::{self, OpCode},
};
impl Compiler {
    pub(crate) fn emit_array_expression(
        &self, arr: &oxide_parser::ArrayExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let arr_reg = ctx.alloc_reg();
        let n = arr.elements.len() as u16;
        ctx.emit(opcode::encode(OpCode::NEW_ARRAY, arr_reg, (n & 0xFF) as u8, ((n >> 8) & 0xFF) as u8));
        let elem_checkpoint = ctx.reg_checkpoint();
        for (i, elem) in arr.elements.iter().enumerate() {
            let Some(e) = elem.as_expression() else {
                return Err("spread not supported".into());
            };
            let val_reg = self.emit_expression(e, ctx)?;
            let idx_reg = ctx.alloc_reg();
            let idx = ctx.add_constant(Constant::Int(i as i32));
            ctx.emit_load_const(idx_reg, idx);
            ctx.emit(opcode::encode(OpCode::SET_ELEM, arr_reg, idx_reg, val_reg));
            ctx.restore_reg_checkpoint(elem_checkpoint);
        }
        Ok(arr_reg)
    }
}
