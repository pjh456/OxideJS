use super::*;

impl Compiler {
    pub(in crate::counter) fn count_if_statement(&self, stmt: &oxide_parser::IfStatement<'_>, ctx: &mut CompileCtx) {
        let id = ctx.next_label_id();
        let else_label = Label::IfElse(id);
        let end_label = Label::IfEnd(id);

        self.count_expression(&stmt.test, ctx);
        ctx.projected_pc += 1; // JMP_IF_FALSE
        self.count_statement(&stmt.consequent, ctx);
        ctx.alloc_reg(); // result register
        ctx.projected_pc += 1; // LOAD_VAR result <- consequent
        if stmt.alternate.is_some() {
            ctx.projected_pc += 1; // JMP (skip else)
        }
        ctx.labels.label_map.insert(else_label, ctx.projected_pc);
        if let Some(alt_stmt) = &stmt.alternate {
            self.count_statement(alt_stmt, ctx);
            ctx.projected_pc += 1; // LOAD_VAR result <- alternate
        }
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }
}
