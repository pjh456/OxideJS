//! 模板字面量 emit：`emit_template_literal_expression` 生成 TEMPLATE_STR 指令
//! 拼接字面量段与内插表达式。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;

impl Emitter {
    pub(crate) fn emit_template_literal_expression(
        &self, tl: &oxide_parser::TemplateLiteral, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let r = ctx.alloc_reg();
        let quasis = &tl.quasis;
        let expressions = &tl.expressions;
        let segment_count = quasis.len() + expressions.len();

        let expr_regs: Vec<u32> = expressions
            .iter()
            .map(|e| self.emit_expression(e, ctx))
            .collect::<Result<Vec<_>, _>>()?;

        let mut quasi_keys: Vec<String> = Vec::with_capacity(quasis.len());
        for q in quasis {
            quasi_keys.push(match &q.value.cooked {
                Some(c) => crate::shared::string_pool::pool_key_marker(c, q.lone_surrogates),
                None => String::new(),
            });
        }
        let quasi_const_idxs: Vec<u16> = quasi_keys
            .iter()
            .map(|s| ctx.add_constant(Constant::String(s.clone())))
            .collect();

        // 容量提示按单元数口径（键文本经物化解码还原单元序列）。
        let total_len_hint: usize = quasi_keys.iter().map(|k| oxide_kernel::string_forge::decode_key(k).len()).sum();

        let mut parts = Vec::with_capacity(quasi_const_idxs.len() * 2);
        let mut expr_iter = expr_regs.iter();
        for const_idx in quasi_const_idxs.iter() {
            parts.push(*const_idx as u32 & 0x7FFF_FFFF);
            if let Some(expr_reg) = expr_iter.next() {
                parts.push(0x8000_0000u32 | (*expr_reg));
            }
        }

        ctx.inst(Inst::template_str(Operand::Reg(r), segment_count as u32, total_len_hint as u16, &parts));

        Ok(r)
    }
}
