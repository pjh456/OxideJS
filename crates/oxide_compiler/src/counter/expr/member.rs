use super::*;

impl Compiler {
    pub(in crate::counter) fn count_static_member_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::StaticMemberExpression(member) = expr else {
            return;
        };
        if matches!(&member.object, Expression::Super(_)) {
            ctx.count_load_const(); // key
            ctx.count_load_var(); // this
            ctx.alloc_reg(); // result
            ctx.count_instr(); // SUPER_GET_PROP
            return;
        }
        self.count_expression(&member.object, ctx);
        ctx.count_load_const(); // key
        ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext words
    }

    pub(in crate::counter) fn count_computed_member_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ComputedMemberExpression(member) = expr else {
            return;
        };
        self.count_expression(&member.object, ctx);
        self.count_expression(&member.expression, ctx);
        ctx.alloc_reg();
        ctx.projected_pc += 1; // GET_PROP_DYNAMIC
    }

    pub(in crate::counter) fn count_private_field_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::PrivateFieldExpression(member) = expr else {
            return;
        };
        self.count_expression(&member.object, ctx);
        ctx.count_private_access();
    }

    pub(in crate::counter) fn count_parenthesized_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ParenthesizedExpression(p) = expr else {
            return;
        };
        self.count_expression(&p.expression, ctx);
    }
}
