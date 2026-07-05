use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::{
    module::Constant,
    opcode::{self, OpCode},
};
use oxide_parser::Expression;

impl Compiler {
    pub(crate) fn count_tagged_template_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::TaggedTemplateExpression(tt) = expr else {
            return;
        };
        self.count_expression(&tt.tag, ctx);
        let quasi_count = tt.quasi.quasis.len();

        for _ in 0..quasi_count {
            ctx.count_load_const();
            ctx.count_load_const();
            ctx.count_instr();
        }
        ctx.alloc_reg();
        ctx.count_instr();

        for _ in 0..quasi_count {
            ctx.count_load_const();
            ctx.count_load_const();
            ctx.count_instr();
        }
        ctx.alloc_reg();
        ctx.count_instr();

        for expr in &tt.quasi.expressions {
            self.count_expression(expr, ctx);
        }

        ctx.alloc_reg();
        ctx.alloc_reg();
        for _ in &tt.quasi.expressions {
            ctx.alloc_reg();
            ctx.count_instr();
        }
        ctx.count_words(2);
        ctx.count_load_const();
        ctx.count_call_instr_with_arg_ext();
        ctx.count_load_var();
    }

    pub(crate) fn emit_tagged_template_expression(
        &self, tt: &oxide_parser::TaggedTemplateExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let quasis = &tt.quasi.quasis;
        let expressions = &tt.quasi.expressions;

        let tag_reg = self.emit_expression(&tt.tag, ctx)?;

        let cooked_temp = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::NEW_ARRAY, cooked_temp, quasis.len() as u8, 0));
        for (i, quasi) in quasis.iter().enumerate() {
            let s = quasi.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
            let const_idx = ctx.add_constant(Constant::String(s));
            let str_reg = ctx.alloc_reg();
            ctx.emit_load_const(str_reg, const_idx);
            let idx_const = ctx.add_constant(Constant::Int(i as i32));
            let idx_reg = ctx.alloc_reg();
            ctx.emit_load_const(idx_reg, idx_const);
            ctx.emit(opcode::encode(OpCode::SET_ELEM, cooked_temp, idx_reg, str_reg));
        }

        let raw_temp = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::NEW_ARRAY, raw_temp, quasis.len() as u8, 0));
        for (i, quasi) in quasis.iter().enumerate() {
            let raw = quasi.value.raw.to_string();
            let const_idx = ctx.add_constant(Constant::String(raw));
            let str_reg = ctx.alloc_reg();
            ctx.emit_load_const(str_reg, const_idx);
            let idx_const = ctx.add_constant(Constant::Int(i as i32));
            let idx_reg = ctx.alloc_reg();
            ctx.emit_load_const(idx_reg, idx_const);
            ctx.emit(opcode::encode(OpCode::SET_ELEM, raw_temp, idx_reg, str_reg));
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

        ctx.emit(opcode::encode(OpCode::LOAD_VAR, cooked_slot, cooked_temp, 0));
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, raw_slot, raw_temp, 0));
        for (slot, temp) in expr_slots.iter().zip(expr_temps.iter()) {
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, *slot, *temp, 0));
        }

        let undef_idx = ctx.add_constant(Constant::Undefined);
        let undef_reg = ctx.alloc_reg();
        ctx.emit_load_const(undef_reg, undef_idx);

        let arg_count = 2 + expressions.len();
        ctx.emit(opcode::encode(OpCode::CALL, tag_reg, undef_reg, cooked_slot));
        ctx.emit(arg_count as u32);

        let result_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, 0, 0));
        Ok(result_reg)
    }
}
