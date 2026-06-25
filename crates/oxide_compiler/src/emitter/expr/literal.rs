use super::*;

impl Compiler {
    /// Domain entry point: emit any literal expression. The central
    /// `emit_expression` match delegates all literal variants here in one arm;
    /// adding a new literal kind only touches this file plus that one arm.
    pub(in crate::emitter) fn emit_literal(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::NumericLiteral(n) => self.emit_numeric_literal_expression(n, ctx),
            Expression::StringLiteral(s) => self.emit_string_literal_expression(s, ctx),
            Expression::BooleanLiteral(b) => self.emit_boolean_literal_expression(b, ctx),
            Expression::NullLiteral(_) => self.emit_null_literal_expression(ctx),
            Expression::RegExpLiteral(lit) => self.emit_reg_exp_literal_expression(lit, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }

    fn emit_numeric_literal_expression(
        &self, n: &oxide_parser::NumericLiteral, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let idx = if is_int_literal(n.value) {
            ctx.add_constant(Constant::Int(n.value as i32))
        } else {
            ctx.add_constant(Constant::Number(n.value))
        };
        let r = ctx.alloc_reg();
        ctx.emit_load_const(r, idx);
        Ok(r)
    }

    fn emit_string_literal_expression(
        &self, s: &oxide_parser::StringLiteral, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let idx = ctx.add_constant(Constant::String(s.value.to_string()));
        let r = ctx.alloc_reg();
        ctx.emit_load_const(r, idx);
        Ok(r)
    }

    fn emit_boolean_literal_expression(
        &self, b: &oxide_parser::BooleanLiteral, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let idx = ctx.add_constant(Constant::Boolean(b.value));
        let r = ctx.alloc_reg();
        ctx.emit_load_const(r, idx);
        Ok(r)
    }

    fn emit_null_literal_expression(&self, ctx: &mut CompileCtx) -> Result<u8, String> {
        let idx = ctx.add_constant(Constant::Null);
        let r = ctx.alloc_reg();
        ctx.emit_load_const(r, idx);
        Ok(r)
    }

    pub(in crate::emitter) fn emit_identifier_expression(
        &self, ident: &oxide_parser::IdentifierReference, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let var_reg = ctx.lookup_or_builtin(ident.name.as_str())?;
        let r = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, var_reg, 0));
        Ok(r)
    }

    pub(in crate::emitter) fn emit_parenthesized_expression(
        &self, p: &oxide_parser::ParenthesizedExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        self.emit_expression(&p.expression, ctx)
    }

    pub(in crate::emitter) fn emit_this_expression(&self, ctx: &mut CompileCtx) -> Result<u8, String> {
        let r = ctx.alloc_reg();
        let src = ctx.static_block_this_reg.unwrap_or(254);
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, src, 0));
        Ok(r)
    }

    fn emit_reg_exp_literal_expression(
        &self, lit: &oxide_parser::RegExpLiteral, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        if let Some(raw) = &lit.raw {
            let raw_str = raw.to_string();
            if raw_str.len() >= 2 && raw_str.starts_with('/') {
                let last_slash = raw_str.rfind('/').unwrap_or(raw_str.len() - 1);
                let pattern = raw_str[1..last_slash].to_string();
                let flags = raw_str[last_slash + 1..].to_string();
                let pat_ci = ctx.add_constant(Constant::String(pattern));
                let pat_reg = ctx.alloc_reg();
                ctx.emit_load_const(pat_reg, pat_ci);
                let flags_ci = ctx.add_constant(Constant::String(flags));
                let flags_reg = ctx.alloc_reg();
                ctx.emit_load_const(flags_reg, flags_ci);
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::CREATE_REGEXP, r, pat_reg, flags_reg));
                Ok(r)
            } else {
                Err(format!("unsupported regexp literal: {:?}", lit))
            }
        } else {
            Err(format!("unsupported regexp literal: {:?}", lit))
        }
    }

    pub(in crate::emitter) fn emit_sequence_expression(
        &self, seq: &oxide_parser::SequenceExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        // Comma operator: evaluate each expression in order, value is the last.
        // Emits exactly one expression per element (no extra instruction), so the
        // projected-PC count in count_sequence_expression stays aligned.
        let mut last = 0u8;
        for e in &seq.expressions {
            last = self.emit_expression(e, ctx)?;
        }
        Ok(last)
    }

    pub(in crate::emitter) fn emit_unsupported_expression(
        &self, expr: &Expression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let _ = ctx;
        Err(format!("unsupported expression type: {:?}", expr))
    }
}
