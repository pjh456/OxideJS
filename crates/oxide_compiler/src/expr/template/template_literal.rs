use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::{
    module::Constant,
    opcode::{self, OpCode},
};
impl Compiler {
    pub(crate) fn emit_template_literal_expression(
        &self, tl: &oxide_parser::TemplateLiteral, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let r = ctx.alloc_reg();
        let quasis = &tl.quasis;
        let expressions = &tl.expressions;
        let segment_count = quasis.len() + expressions.len();

        let expr_regs: Vec<u8> = expressions
            .iter()
            .map(|e| self.emit_expression(e, ctx))
            .collect::<Result<Vec<_>, _>>()?;

        let quasi_const_idxs: Vec<u16> = quasis
            .iter()
            .map(|q| {
                let s = q.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
                ctx.add_constant(Constant::String(s))
            })
            .collect();

        let total_len_hint: usize = quasis
            .iter()
            .map(|q| q.value.cooked.as_ref().map(|c| c.len()).unwrap_or(0))
            .sum();

        ctx.emit(opcode::encode(OpCode::TEMPLATE_STR, r, 0, 0));
        ctx.emit(((segment_count as u32) << 16) | (total_len_hint as u32 & 0xFFFF));

        let mut expr_iter = expr_regs.iter();
        for const_idx in quasi_const_idxs.iter() {
            ctx.emit(*const_idx as u32 & 0x7FFF_FFFF);
            if let Some(expr_reg) = expr_iter.next() {
                ctx.emit(0x8000_0000u32 | (*expr_reg as u32));
            }
        }

        Ok(r)
    }
}
