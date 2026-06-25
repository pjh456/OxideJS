use super::*;

impl Compiler {
    fn count_arrow_function_expression(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_CONST
    }

    fn count_function_expression(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_CONST
    }

    fn count_class_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ClassExpression(class) = expr else {
            return;
        };
        self.count_class(class, ctx);
    }

    pub(in crate::counter) fn count_function_domain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::ArrowFunctionExpression(_) => self.count_arrow_function_expression(ctx),
            Expression::FunctionExpression(_) => self.count_function_expression(ctx),
            Expression::ClassExpression(_) => self.count_class_expression(expr, ctx),
            _ => {}
        }
    }
}
