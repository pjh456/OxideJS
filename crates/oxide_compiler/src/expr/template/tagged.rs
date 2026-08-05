use crate::compiler::{CompileCtx, Compiler};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
impl Compiler {
    pub(crate) fn emit_tagged_template_expression(
        &self, tt: &oxide_parser::TaggedTemplateExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let quasis = &tt.quasi.quasis;
        let expressions = &tt.quasi.expressions;

        let tag_reg = self.emit_expression(&tt.tag, ctx)?;

        let cooked_temp = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(cooked_temp as u32), Operand::Imm(quasis.len() as u16), Operand::None));
        for (i, quasi) in quasis.iter().enumerate() {
            let s = quasi.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
            let const_idx = ctx.add_constant(Constant::String(s));
            let str_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(str_reg as u32), const_idx));
            let idx_const = ctx.add_constant(Constant::Int(i as i32));
            let idx_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(idx_reg as u32), idx_const));
            ctx.inst(Inst::new(OpCode::SET_ELEM, Operand::Reg(cooked_temp as u32), Operand::Reg(idx_reg as u32), Operand::Reg(str_reg as u32)));
        }

        let raw_temp = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(raw_temp as u32), Operand::Imm(quasis.len() as u16), Operand::None));
        for (i, quasi) in quasis.iter().enumerate() {
            let raw = quasi.value.raw.to_string();
            let const_idx = ctx.add_constant(Constant::String(raw));
            let str_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(str_reg as u32), const_idx));
            let idx_const = ctx.add_constant(Constant::Int(i as i32));
            let idx_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(idx_reg as u32), idx_const));
            ctx.inst(Inst::new(OpCode::SET_ELEM, Operand::Reg(raw_temp as u32), Operand::Reg(idx_reg as u32), Operand::Reg(str_reg as u32)));
        }

        let mut expr_temps = Vec::new();
        for expr in expressions {
            expr_temps.push(self.emit_expression(expr, ctx)?);
        }

        let cooked_slot = ctx.alloc_reg();
        let raw_slot = ctx.alloc_reg();
        let mut expr_slots = Vec::new();
        for _ in expressions {
            expr_slots.push(ctx.alloc_reg());
        }

        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(cooked_slot as u32), Operand::Reg(cooked_temp as u32), Operand::None));
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(raw_slot as u32), Operand::Reg(raw_temp as u32), Operand::None));
        for (slot, temp) in expr_slots.iter().zip(expr_temps.iter()) {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(*slot as u32), Operand::Reg(*temp as u32), Operand::None));
        }

        let undef_idx = ctx.add_constant(Constant::Undefined);
        let undef_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(undef_reg as u32), undef_idx));

        let arg_count = 2 + expressions.len();
        ctx.inst(Inst::call(
            Operand::Reg(tag_reg as u32),
            Operand::Reg(undef_reg as u32),
            Operand::Reg(cooked_slot as u32),
            arg_count as u8,
        ));

        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::None, Operand::None));
        Ok(result_reg)
    }
}
