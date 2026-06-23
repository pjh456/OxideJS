use super::*;

impl Compiler {
    pub(in crate::counter) fn count_expression_statement(
        &self, stmt: &oxide_parser::ExpressionStatement<'_>, ctx: &mut CompileCtx,
    ) {
        self.count_expression(&stmt.expression, ctx);
    }

    pub(in crate::counter) fn count_return_statement(
        &self, stmt: &oxide_parser::ReturnStatement<'_>, ctx: &mut CompileCtx,
    ) {
        if let Some(arg) = &stmt.argument {
            self.count_expression(arg, ctx);
        }
        ctx.projected_pc += 1; // RETURN
    }

    pub(in crate::counter) fn count_break_statement(&self, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1; // JMP
    }

    pub(in crate::counter) fn count_continue_statement(&self, ctx: &mut CompileCtx) {
        ctx.projected_pc += 1; // JMP
    }
}
