use super::*;

impl Compiler {
    pub(in crate::counter) fn count_binary_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::BinaryExpression(bin) = expr else {
            return;
        };
        let checkpoint = ctx.reg_checkpoint();
        self.count_expression(&bin.left, ctx);
        self.count_expression(&bin.right, ctx);
        ctx.projected_pc += 1; // ADD/SUB/MUL/DIV/etc.
        if is_side_effect_free(&bin.left) && is_side_effect_free(&bin.right) {
            ctx.restore_reg_checkpoint(checkpoint.saturating_add(1));
        }
    }

    pub(in crate::counter) fn count_private_in_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::PrivateInExpression(pin) = expr else {
            return;
        };
        self.count_expression(&pin.right, ctx);
        ctx.count_private_access();
    }

    pub(in crate::counter) fn count_unary_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::UnaryExpression(un) = expr else {
            return;
        };
        match un.operator {
            UnaryOperator::Delete => self.count_delete_expression(&un.argument, ctx),
            _ => {
                self.count_expression(&un.argument, ctx);
                ctx.projected_pc += 1; // NEG/TYPEOF/VOID/NOT
            }
        }
    }

    fn count_delete_expression(&self, argument: &Expression, ctx: &mut CompileCtx) {
        match argument {
            Expression::Identifier(_) => {
                // SyntaxError at compile time, no bytecode cost.
            }
            Expression::StaticMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.count_delete_static();
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                self.count_expression(&member.expression, ctx);
                ctx.projected_pc += 1; // DELETE_PROP_DYNAMIC
            }
            Expression::ChainExpression(chain) => {
                self.count_delete_chain_expression(&chain.expression, ctx);
            }
            _ => {}
        }
    }

    fn count_delete_chain_expression(&self, chain: &ChainElement, ctx: &mut CompileCtx) {
        match chain {
            ChainElement::StaticMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                if member.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.count_delete_static();
            }
            ChainElement::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                if member.optional {
                    self.count_optional_guard(ctx);
                }
                self.count_expression(&member.expression, ctx);
                ctx.projected_pc += 1; // DELETE_PROP_DYNAMIC
            }
            _ => {}
        }
        ctx.projected_pc += 1; // JMP over nullish true writer
        ctx.projected_pc += 1; // LOAD_CONST true
    }

    pub(in crate::counter) fn count_update_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::UpdateExpression(update) = expr else {
            return;
        };
        match &update.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.count_load_const(); // key
                ctx.alloc_reg();
                ctx.count_ic_instr_with_ext(); // MEMBER_INC/MEMBER_DEC + 3 ext words
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                self.count_expression(&member.expression, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
            _ => {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
        }
    }
}
