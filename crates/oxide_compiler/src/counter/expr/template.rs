use super::*;

impl Compiler {
    fn count_template_literal(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::TemplateLiteral(tl) = expr else {
            return;
        };
        for expr in &tl.expressions {
            self.count_expression(expr, ctx);
        }
        let segment_count = tl.quasis.len() + tl.expressions.len();
        ctx.count_template_str(segment_count);
    }

    fn count_tagged_template_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::TaggedTemplateExpression(tt) = expr else {
            return;
        };
        self.count_expression(&tt.tag, ctx);
        let quasi_count = tt.quasi.quasis.len();

        for _ in 0..quasi_count {
            ctx.count_load_const(); // string
            ctx.count_load_const(); // index
            ctx.count_instr(); // SET_ELEM (cooked)
        }
        ctx.alloc_reg(); // cooked_temp
        ctx.count_instr(); // NEW_ARRAY

        for _ in 0..quasi_count {
            ctx.count_load_const(); // string
            ctx.count_load_const(); // index
            ctx.count_instr(); // SET_ELEM (raw)
        }
        ctx.alloc_reg(); // raw_temp
        ctx.count_instr(); // NEW_ARRAY

        for expr in &tt.quasi.expressions {
            self.count_expression(expr, ctx);
        }

        ctx.alloc_reg(); // cooked_slot
        ctx.alloc_reg(); // raw_slot
        for _ in &tt.quasi.expressions {
            ctx.alloc_reg(); // expr_slot
            ctx.count_instr(); // LOAD_VAR
        }
        ctx.count_words(2); // LOAD_VAR for cooked and raw
        ctx.count_load_const(); // undefined this arg
        ctx.count_call_instr_with_arg_ext();
        ctx.count_load_var(); // result <- regs[0]
    }

    pub(in crate::counter) fn count_template_domain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::TemplateLiteral(_) => self.count_template_literal(expr, ctx),
            Expression::TaggedTemplateExpression(_) => self.count_tagged_template_expression(expr, ctx),
            _ => {}
        }
    }
}
