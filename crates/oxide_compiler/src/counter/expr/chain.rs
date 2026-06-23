use super::*;

impl Compiler {
    pub(in crate::counter) fn count_chain_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ChainExpression(chain) = expr else {
            return;
        };
        let id = ctx.next_label_id();
        self.count_chain_element(&chain.expression, true, ctx);
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_VAR result <- chain value
        ctx.projected_pc += 1; // JMP over short-circuit writer
        ctx.label_map.insert(Label::TernaryElse(id), ctx.projected_pc);
        ctx.projected_pc += 1; // LOAD_CONST undefined
        ctx.label_map.insert(Label::TernaryEnd(id), ctx.projected_pc);
    }
}
