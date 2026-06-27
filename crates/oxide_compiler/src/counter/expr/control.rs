use super::*;

impl Compiler {
    fn count_conditional_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
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
        ctx.labels.label_map.insert(else_label, ctx.projected_pc);
        self.count_expression(&cond.alternate, ctx);
        ctx.projected_pc += 1; // LOAD_VAR result <- alternate
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }

    fn count_sequence_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::SequenceExpression(seq) = expr else {
            return;
        };
        for e in &seq.expressions {
            self.count_expression(e, ctx);
        }
    }

    fn count_logical_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::LogicalExpression(log) = expr else {
            return;
        };
        self.count_expression(&log.left, ctx);

        let is_simple = is_side_effect_free(&log.left) && is_side_effect_free(&log.right);

        if is_simple {
            self.count_expression(&log.right, ctx);
            ctx.alloc_reg();
            ctx.projected_pc += 1; // AND/OR
        } else if matches!(log.operator, LogicalOperator::Coalesce) {
            // Mirror emit_logical_expression's Coalesce path: it patches raw bytecode
            // positions and does NOT consume a label id. Consuming one here would shift
            // every later construct's label id and break resolve_label at emit time.
            ctx.alloc_reg(); // dup register
            ctx.projected_pc += 1; // LOAD_VAR (DUP)
            ctx.projected_pc += 1; // JMP_IF_NULLISH
            ctx.projected_pc += 1; // JMP over RHS on non-nullish
            self.count_expression(&log.right, ctx);
            ctx.projected_pc += 1; // LOAD_VAR (overwrite)
        } else {
            let id = ctx.next_label_id();
            ctx.alloc_reg(); // dup register
            ctx.projected_pc += 1; // LOAD_VAR (DUP)
            ctx.projected_pc += 1; // JMP_IF_FALSE or JMP_IF_TRUE
            self.count_expression(&log.right, ctx);
            ctx.projected_pc += 1; // LOAD_VAR (overwrite)
            let skip_label = match log.operator {
                LogicalOperator::And => Label::TernaryEnd(id),
                LogicalOperator::Or => Label::TernaryElse(id),
                LogicalOperator::Coalesce => unreachable!("coalesce handled above"),
            };
            ctx.labels.label_map.insert(skip_label, ctx.projected_pc);
        }
    }

    pub(in crate::counter) fn count_conditional_chain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::ConditionalExpression(_) => self.count_conditional_expression(expr, ctx),
            Expression::SequenceExpression(_) => self.count_sequence_expression(expr, ctx),
            Expression::LogicalExpression(_) => self.count_logical_expression(expr, ctx),
            _ => {}
        }
    }
}
