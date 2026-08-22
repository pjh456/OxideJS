//! 标签模板表达式 emit：`emit_tagged_template_expression` 构造 template object 并调用标签函数。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
impl Emitter {
    pub(crate) fn emit_tagged_template_expression(
        &self, tt: &oxide_parser::TaggedTemplateExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let quasis = &tt.quasi.quasis;
        let expressions = &tt.quasi.expressions;

        let tag_reg = self.emit_expression(&tt.tag, ctx)?;

        let cooked_temp = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::NEW_ARRAY,
            Operand::Reg(cooked_temp),
            Operand::Imm(quasis.len() as u16),
            Operand::None,
        ));
        for (i, quasi) in quasis.iter().enumerate() {
            let s = quasi.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
            let const_idx = ctx.add_constant(Constant::String(s));
            let str_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(str_reg), const_idx));
            let idx_const = ctx.add_constant(Constant::Int(i as i32));
            let idx_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(idx_reg), idx_const));
            ctx.inst(Inst::new(
                OpCode::SET_ELEM,
                Operand::Reg(cooked_temp),
                Operand::Reg(idx_reg),
                Operand::Reg(str_reg),
            ));
        }

        let raw_temp = ctx.alloc_reg();
        ctx.inst(Inst::new(
            OpCode::NEW_ARRAY,
            Operand::Reg(raw_temp),
            Operand::Imm(quasis.len() as u16),
            Operand::None,
        ));
        for (i, quasi) in quasis.iter().enumerate() {
            let raw = quasi.value.raw.to_string();
            let const_idx = ctx.add_constant(Constant::String(raw));
            let str_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(str_reg), const_idx));
            let idx_const = ctx.add_constant(Constant::Int(i as i32));
            let idx_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(idx_reg), idx_const));
            ctx.inst(Inst::new(
                OpCode::SET_ELEM,
                Operand::Reg(raw_temp),
                Operand::Reg(idx_reg),
                Operand::Reg(str_reg),
            ));
        }

        // 模板对象：按 GetTemplateObject 语义把 raw 数组挂到 cooked 数组的 "raw"
        // 属性，使标签函数首参为标准模板对象（cooked 数组 + raw 属性）。
        // String.raw / 模板标签测试依赖 template.raw 存在。
        let raw_key_idx = ctx.add_constant(Constant::String("raw".to_string()));
        let raw_key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(raw_key_reg), raw_key_idx));
        ctx.inst(Inst::new(
            OpCode::SET_PROP,
            Operand::Reg(cooked_temp),
            Operand::Reg(raw_temp),
            Operand::Reg(raw_key_reg),
        ));

        let mut expr_temps = Vec::new();
        for expr in expressions {
            expr_temps.push(self.emit_expression(expr, ctx)?);
        }

        let cooked_slot = ctx.alloc_reg();
        let mut expr_slots = Vec::new();
        for _ in expressions {
            expr_slots.push(ctx.alloc_reg());
        }

        ctx.inst(Inst::new(
            OpCode::LOAD_VAR,
            Operand::Reg(cooked_slot),
            Operand::Reg(cooked_temp),
            Operand::None,
        ));
        for (slot, temp) in expr_slots.iter().zip(expr_temps.iter()) {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(*slot), Operand::Reg(*temp), Operand::None));
        }

        let undef_idx = ctx.add_constant(Constant::Undefined);
        let undef_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(undef_reg), undef_idx));

        let arg_count = 1 + expressions.len();
        ctx.inst(Inst::call(
            Operand::Reg(tag_reg),
            Operand::Reg(undef_reg),
            Operand::Reg(cooked_slot),
            arg_count as u8,
        ));

        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::None, Operand::None));
        Ok(result_reg)
    }
}
