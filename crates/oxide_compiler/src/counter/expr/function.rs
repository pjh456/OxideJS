use super::*;

impl Compiler {
    pub(in crate::counter) fn count_arrow_function_expression(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_CONST
    }

    pub(in crate::counter) fn count_function_expression(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_CONST
    }

    pub(in crate::counter) fn count_class_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ClassExpression(class) = expr else {
            return;
        };
        self.count_class(class, ctx);
    }
}
