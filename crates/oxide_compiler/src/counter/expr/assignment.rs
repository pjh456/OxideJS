use super::*;

impl Compiler {
    fn count_assignment_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::AssignmentExpression(assign) = expr else {
            return;
        };

        if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left {
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                self.count_static_member_logical_assignment(member, logical_op, &assign.right, ctx);
                return;
            }
            self.count_expression(&member.object, ctx);
            self.count_expression(&assign.right, ctx);
            if assign.operator != AssignmentOperator::Assign {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
            ctx.count_load_const(); // key
            ctx.count_ic_set_with_ext(); // IC_SET_PROP or COMPOUND_MEMBER_* + 3 ext words
        } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) = &assign.left {
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                self.count_computed_member_logical_assignment(member, logical_op, &assign.right, ctx);
                return;
            }
            self.count_expression(&member.object, ctx);
            self.count_expression(&member.expression, ctx);
            self.count_expression(&assign.right, ctx);
            ctx.alloc_reg();
            ctx.projected_pc += 1; // SET_PROP_DYNAMIC
        } else if let oxide_parser::AssignmentTarget::PrivateFieldExpression(member) = &assign.left {
            self.count_expression(&member.object, ctx);
            self.count_expression(&assign.right, ctx);
            ctx.count_load_const(); // private id
            ctx.count_instr(); // SET_PRIVATE
        } else if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(_) = &assign.left {
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                self.count_identifier_logical_assignment(logical_op, &assign.right, ctx);
            } else {
                self.count_expression(&assign.right, ctx);
                if assign.operator != AssignmentOperator::Assign {
                    ctx.projected_pc += 1; // COMPOUND_* on var
                } else {
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // STORE_VAR
                }
            }
        } else if let oxide_parser::AssignmentTarget::ArrayAssignmentTarget(ap) = &assign.left {
            self.count_expression(&assign.right, ctx);
            self.count_array_assignment(ap, ctx);
        } else if let oxide_parser::AssignmentTarget::ObjectAssignmentTarget(op) = &assign.left {
            self.count_expression(&assign.right, ctx);
            self.count_object_assignment(op, ctx);
        } else {
            self.count_expression(&assign.right, ctx);
            ctx.alloc_reg();
            ctx.projected_pc += 1;
        }
    }

    fn count_static_member_logical_assignment(
        &self, member: &oxide_parser::StaticMemberExpression<'_>, logical_op: LogicalOperator, right: &Expression,
        ctx: &mut CompileCtx,
    ) {
        let id = ctx.next_label_id();
        self.count_expression(&member.object, ctx);
        ctx.count_load_const(); // key
        ctx.count_load_var(); // current value copy
        ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext
        self.count_logical_assign_test(logical_op, id, ctx);
        self.count_expression(right, ctx);
        ctx.count_ic_set_with_ext(); // IC_SET_PROP + 3 ext
        ctx.projected_pc += 1; // LOAD_VAR result <- rhs
        ctx.labels.label_map.insert(Label::TernaryEnd(id), ctx.projected_pc);
    }

    fn count_computed_member_logical_assignment(
        &self, member: &oxide_parser::ComputedMemberExpression<'_>, logical_op: LogicalOperator, right: &Expression,
        ctx: &mut CompileCtx,
    ) {
        let id = ctx.next_label_id();
        self.count_expression(&member.object, ctx);
        self.count_expression(&member.expression, ctx);
        ctx.alloc_reg();
        ctx.projected_pc += 1; // GET_PROP_DYNAMIC
        self.count_logical_assign_test(logical_op, id, ctx);
        self.count_expression(right, ctx);
        ctx.projected_pc += 1; // SET_PROP_DYNAMIC
        ctx.projected_pc += 1; // LOAD_VAR result <- rhs
        ctx.labels.label_map.insert(Label::TernaryEnd(id), ctx.projected_pc);
    }

    fn count_identifier_logical_assignment(
        &self, logical_op: LogicalOperator, right: &Expression, ctx: &mut CompileCtx,
    ) {
        let id = ctx.next_label_id();
        ctx.alloc_reg();
        ctx.projected_pc += 1; // LOAD_VAR result <- current var
        self.count_logical_assign_test(logical_op, id, ctx);
        self.count_expression(right, ctx);
        ctx.projected_pc += 1; // STORE_VAR
        ctx.projected_pc += 1; // LOAD_VAR result <- rhs
        ctx.labels.label_map.insert(Label::TernaryEnd(id), ctx.projected_pc);
    }

    pub(in crate::counter) fn count_assignment(&self, expr: &Expression, ctx: &mut CompileCtx) {
        self.count_assignment_expression(expr, ctx);
    }
}
