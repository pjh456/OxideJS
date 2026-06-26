use super::*;

impl Compiler {
    fn count_object_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ObjectExpression(obj) = expr else {
            return;
        };
        ctx.alloc_reg();
        ctx.projected_pc += 1; // NEW_OBJECT
        let prop_checkpoint = ctx.reg_checkpoint();
        for prop in &obj.properties {
            if let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop {
                if matches!(p.kind, oxide_parser::PropertyKind::Get | oxide_parser::PropertyKind::Set) {
                    self.count_expression(&p.value, ctx);
                    ctx.count_load_const(); // undefined getter/setter placeholder
                    ctx.count_define_accessor(); // DEFINE_ACCESSOR + ext
                } else {
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // LOAD_CONST key
                    self.count_expression(&p.value, ctx);
                    ctx.projected_pc += 1; // SET_PROP
                }
                ctx.restore_reg_checkpoint(prop_checkpoint);
            }
        }
    }

    fn count_array_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ArrayExpression(arr) = expr else {
            return;
        };
        ctx.alloc_reg();
        ctx.projected_pc += 1; // NEW_ARRAY
        let elem_checkpoint = ctx.reg_checkpoint();
        for elem in &arr.elements {
            if let Some(e) = elem.as_expression() {
                self.count_expression(e, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST index
                ctx.projected_pc += 1; // SET_ELEM
                ctx.restore_reg_checkpoint(elem_checkpoint);
            }
        }
    }

    pub(in crate::counter) fn count_object_domain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::ObjectExpression(_) => self.count_object_expression(expr, ctx),
            Expression::ArrayExpression(_) => self.count_array_expression(expr, ctx),
            _ => {}
        }
    }
}
