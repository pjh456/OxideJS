use super::*;

impl Compiler {
    pub(in crate::counter) fn count_this_expression(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_VAR from reg 254
    }

    pub(in crate::counter) fn count_identifier_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::Identifier(ident) = expr else {
            return;
        };
        if CompileCtx::is_known_builtin(ident.name.as_str()) {
            let _ = ctx.lookup_or_builtin(ident.name.as_str());
        }
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_VAR
    }

    pub(in crate::counter) fn count_regexp_literal(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg(); // pattern reg (LOAD_CONST)
        ctx.projected_pc += 1;
        ctx.alloc_reg(); // flags reg (LOAD_CONST)
        ctx.projected_pc += 1;
        ctx.alloc_reg(); // result reg (CREATE_REGEXP)
        ctx.projected_pc += 1;
    }

    pub(in crate::counter) fn count_default_expression(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_CONST or LOAD_VAR
    }
}
