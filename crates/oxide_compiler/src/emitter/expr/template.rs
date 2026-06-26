use super::*;

impl Compiler {
    fn emit_template_literal_expression(
        &self, tl: &oxide_parser::TemplateLiteral, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let r = ctx.alloc_reg();
        let quasis = &tl.quasis;
        let expressions = &tl.expressions;
        let segment_count = quasis.len() + expressions.len();

        // Evaluate each expression, collecting registers
        let expr_regs: Vec<u8> = expressions
            .iter()
            .map(|e| self.emit_expression(e, ctx))
            .collect::<Result<Vec<_>, _>>()?;

        // Add each quasi string to constant pool
        let quasi_const_idxs: Vec<u16> = quasis
            .iter()
            .map(|q| {
                let s = q.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
                ctx.add_constant(Constant::String(s))
            })
            .collect();

        // Compute total length hint
        let total_len_hint: usize = quasis
            .iter()
            .map(|q| q.value.cooked.as_ref().map(|c| c.len()).unwrap_or(0))
            .sum();

        // Emit TEMPLATE_STR rd, 0, 0
        ctx.emit(opcode::encode(OpCode::TEMPLATE_STR, r, 0, 0));

        // Ext word 0: (segment_count << 16) | (total_len_hint & 0xFFFF)
        ctx.emit(((segment_count as u32) << 16) | (total_len_hint as u32 & 0xFFFF));

        // Interleave segments: quasi[0], expr[0], quasi[1], expr[1], ...
        let mut expr_iter = expr_regs.iter();
        for const_idx in quasi_const_idxs.iter() {
            // Quasi: is_expression=0, bits 0-15 = const_idx
            ctx.emit(*const_idx as u32 & 0x7FFF_FFFF);

            // Expression (if any remaining)
            if let Some(expr_reg) = expr_iter.next() {
                // Expression: is_expression=1, bits 0-15 = reg
                ctx.emit(0x8000_0000u32 | (*expr_reg as u32));
            }
        }

        Ok(r)
    }

    fn emit_tagged_template_expression(
        &self, tt: &oxide_parser::TaggedTemplateExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let quasis = &tt.quasi.quasis;
        let expressions = &tt.quasi.expressions;

        // 1. Evaluate tag expression
        let tag_reg = self.emit_expression(&tt.tag, ctx)?;

        // 2. Build cooked strings array (into a temp register)
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

        // 3. Build raw strings array (into a temp register)
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

        // 4. Evaluate expression arguments (into temp registers)
        let mut expr_temps = Vec::new();
        for expr in expressions {
            expr_temps.push(self.emit_expression(expr, ctx)?);
        }

        // 5. Allocate consecutive argument slots: cooked, raw, expr[0], expr[1], ...
        let cooked_slot = ctx.alloc_reg();
        let raw_slot = ctx.alloc_reg();
        let mut expr_slots = Vec::new();
        for _ in expressions {
            expr_slots.push(ctx.alloc_reg());
        }

        // Copy temps to consecutive slots using LOAD_VAR (register-to-register move)
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, cooked_slot, cooked_temp, 0));
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, raw_slot, raw_temp, 0));
        for (slot, temp) in expr_slots.iter().zip(expr_temps.iter()) {
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, *slot, *temp, 0));
        }

        // 6. Emit undefined as this_arg
        let undef_idx = ctx.add_constant(Constant::Undefined);
        let undef_reg = ctx.alloc_reg();
        ctx.emit_load_const(undef_reg, undef_idx);

        // 7. Emit CALL(tag, undefined, cooked_slot)
        let arg_count = 2 + expressions.len();
        ctx.emit(opcode::encode(OpCode::CALL, tag_reg, undef_reg, cooked_slot));
        ctx.emit(arg_count as u32);

        // 8. Result from regs[0]
        let result_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, 0, 0));
        Ok(result_reg)
    }

    pub(in crate::emitter) fn emit_template_domain(
        &self, expr: &Expression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        match expr {
            Expression::TemplateLiteral(tl) => self.emit_template_literal_expression(tl, ctx),
            Expression::TaggedTemplateExpression(tt) => self.emit_tagged_template_expression(tt, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
