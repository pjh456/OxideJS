use super::*;

impl Compiler {
    fn count_call_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::CallExpression(call) = expr else {
            return;
        };
        if matches!(&call.callee, Expression::Super(_)) {
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    self.count_expression(expr, ctx);
                }
            }
            ctx.alloc_reg();
            ctx.count_call_instr_with_arg_ext(); // SUPER_CALL + arg_count ext word
                                                 // Mirror emit: derived-ctor instance fields are injected right after super().
            if let Some(words) = ctx.after_super_count_words.take() {
                ctx.projected_pc += words;
            }
            return;
        }

        match &call.callee {
            Expression::PrivateFieldExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.count_private_access();
            }
            Expression::StaticMemberExpression(member) => {
                if matches!(&member.object, Expression::Super(_)) {
                    ctx.count_load_var(); // this register
                    ctx.count_load_const(); // key
                    ctx.alloc_reg(); // callee
                    ctx.count_instr(); // SUPER_GET_PROP
                } else {
                    self.count_expression(&member.object, ctx);
                    ctx.count_load_const(); // key
                    ctx.count_load_var(); // callee object copy
                    ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext words
                }
            }
            _ => {
                self.count_expression(&call.callee, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
        }

        for arg in &call.arguments {
            if let Some(expr) = arg.as_expression() {
                self.count_expression(expr, ctx);
            }
        }
        ctx.count_call_instr_with_arg_ext(); // CALL/CALL_NATIVE + arg_count ext word
        ctx.count_load_var(); // result <- regs[0]
    }

    fn count_new_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::NewExpression(ne) = expr else {
            return;
        };
        self.count_expression(&ne.callee, ctx);
        for arg in &ne.arguments {
            if let Some(expr) = arg.as_expression() {
                self.count_expression(expr, ctx);
            }
        }
        ctx.alloc_reg(); // result register
        ctx.count_instr_with_ext(1); // NEW_EXPRESSION + arg_count ext word
    }

    pub(in crate::counter) fn count_call_domain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::CallExpression(_) => self.count_call_expression(expr, ctx),
            Expression::NewExpression(_) => self.count_new_expression(expr, ctx),
            _ => {}
        }
    }
}
