use super::*;

impl Compiler {
    pub(in crate::counter) fn count_conditional_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ConditionalExpression(cond) = expr else {
            return;
        };
        let id = ctx.next_label_id();
        let else_label = Label::TernaryElse(id);
        let end_label = Label::TernaryEnd(id);

        self.count_expression(&cond.test, ctx);
        ctx.projected_pc += 1; // JMP_IF_FALSE
        self.count_expression(&cond.consequent, ctx);
        ctx.alloc_reg(); // result register
        ctx.projected_pc += 1; // LOAD_VAR result <- consequent
        ctx.projected_pc += 1; // JMP to end
        ctx.label_map.insert(else_label, ctx.projected_pc);
        self.count_expression(&cond.alternate, ctx);
        ctx.projected_pc += 1; // LOAD_VAR result <- alternate
        ctx.label_map.insert(end_label, ctx.projected_pc);
    }

    pub(in crate::counter) fn count_sequence_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::SequenceExpression(seq) = expr else {
            return;
        };
        for e in &seq.expressions {
            self.count_expression(e, ctx);
        }
    }

    pub(in crate::counter) fn count_logical_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::LogicalExpression(log) = expr else {
            return;
        };
        self.count_expression(&log.left, ctx);

        let is_simple = is_side_effect_free(&log.left) && is_side_effect_free(&log.right);

        if is_simple {
            self.count_expression(&log.right, ctx);
            ctx.alloc_reg();
            ctx.projected_pc += 1; // AND/OR
        } else {
            let id = ctx.next_label_id();
            ctx.alloc_reg(); // dup register
            ctx.projected_pc += 1; // LOAD_VAR (DUP)
            ctx.projected_pc += 1; // JMP_IF_FALSE, JMP_IF_TRUE, or JMP_IF_NULLISH
            if matches!(log.operator, LogicalOperator::Coalesce) {
                ctx.projected_pc += 1; // JMP over RHS on non-nullish
                ctx.label_map.insert(Label::TernaryElse(id), ctx.projected_pc);
            }
            self.count_expression(&log.right, ctx);
            ctx.projected_pc += 1; // LOAD_VAR (overwrite)
            let skip_label = match log.operator {
                LogicalOperator::And => Label::TernaryEnd(id),
                LogicalOperator::Or => Label::TernaryElse(id),
                LogicalOperator::Coalesce => Label::TernaryEnd(id),
            };
            ctx.label_map.insert(skip_label, ctx.projected_pc);
        }
    }
}
