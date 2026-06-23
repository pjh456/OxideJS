use super::super::*;

impl Compiler {
    pub(in crate::counter) fn count_object_property_read_static(&self, ctx: &mut CompileCtx) {
        ctx.count_load_var(); // prop/object temp
        ctx.count_load_const(); // key reg
        ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext words
    }

    pub(in crate::counter) fn count_object_property_read_key(
        &self, key: &oxide_parser::PropertyKey, computed: bool, ctx: &mut CompileCtx,
    ) {
        if !computed {
            self.count_object_property_read_static(ctx);
            let _ = key;
            return;
        }

        ctx.count_load_var(); // prop/object temp
        match key {
            oxide_parser::PropertyKey::Identifier(ident) => {
                let name = ident.name.as_str();
                let _ = ctx.lookup_or_builtin(name);
                ctx.count_load_var(); // key
            }
            oxide_parser::PropertyKey::StringLiteral(_) => {
                ctx.count_load_const(); // key
            }
            oxide_parser::PropertyKey::NumericLiteral(_) => {
                ctx.count_load_const(); // key
            }
            oxide_parser::PropertyKey::StaticIdentifier(_) => {
                ctx.count_load_const(); // key
            }
            _ => {}
        }
        ctx.alloc_reg(); // result reg
        ctx.count_instr(); // GET_PROP_DYNAMIC
    }

    pub(in crate::counter) fn count_optional_guard(&self, ctx: &mut CompileCtx) {
        ctx.count_load_var(); // dup
        ctx.count_jump(); // JMP_IF_NULLISH
    }

    pub(in crate::counter) fn count_static_member_get_preserve_base(
        &self, member: &oxide_parser::StaticMemberExpression, short_chain: bool, ctx: &mut CompileCtx,
    ) {
        self.count_chainable_expression(&member.object, short_chain, ctx);
        if short_chain && member.optional {
            self.count_optional_guard(ctx);
        }
        ctx.count_load_const(); // key
        ctx.count_load_var(); // receiver copy
        ctx.count_ic_instr_with_ext(); // IC_GET_PROP + 3 ext words
    }

    pub(in crate::counter) fn count_computed_member_get_preserve_base(
        &self, member: &oxide_parser::ComputedMemberExpression, short_chain: bool, ctx: &mut CompileCtx,
    ) {
        self.count_chainable_expression(&member.object, short_chain, ctx);
        if short_chain && member.optional {
            self.count_optional_guard(ctx);
        }
        self.count_expression(&member.expression, ctx);
        ctx.alloc_reg();
        ctx.count_instr(); // GET_PROP_DYNAMIC
    }

    pub(in crate::counter) fn count_chain_call(
        &self, call: &oxide_parser::CallExpression, short_chain: bool, ctx: &mut CompileCtx,
    ) {
        match &call.callee {
            Expression::StaticMemberExpression(member) => {
                self.count_static_member_get_preserve_base(member, short_chain, ctx)
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_computed_member_get_preserve_base(member, short_chain, ctx);
            }
            Expression::PrivateFieldExpression(member) => {
                self.count_chainable_expression(&member.object, short_chain, ctx);
                if short_chain && member.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.count_private_access();
            }
            _ => {
                self.count_chainable_expression(&call.callee, short_chain, ctx);
                if short_chain && call.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST undefined this
            }
        }
        if short_chain
            && call.optional
            && matches!(
                &call.callee,
                Expression::StaticMemberExpression(_)
                    | Expression::ComputedMemberExpression(_)
                    | Expression::PrivateFieldExpression(_)
            )
        {
            self.count_optional_guard(ctx);
        }
        for arg in &call.arguments {
            if let Some(expr) = arg.as_expression() {
                self.count_expression(expr, ctx);
            }
        }
        ctx.count_call_instr_with_arg_ext(); // CALL + arg_count ext word
        ctx.count_load_var(); // result <- regs[0]
    }

    pub(in crate::counter) fn count_chainable_expression(
        &self, expr: &Expression, short_chain: bool, ctx: &mut CompileCtx,
    ) {
        match expr {
            Expression::StaticMemberExpression(member) => {
                self.count_static_member_get_preserve_base(member, short_chain, ctx)
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_computed_member_get_preserve_base(member, short_chain, ctx)
            }
            Expression::PrivateFieldExpression(member) => {
                self.count_chainable_expression(&member.object, short_chain, ctx);
                if short_chain && member.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.count_private_access();
            }
            Expression::CallExpression(call) => self.count_chain_call(call, short_chain, ctx),
            Expression::ChainExpression(chain) => self.count_chain_element(&chain.expression, short_chain, ctx),
            _ => self.count_expression(expr, ctx),
        }
    }

    pub(in crate::counter) fn count_chain_element(
        &self, element: &ChainElement, short_chain: bool, ctx: &mut CompileCtx,
    ) {
        match element {
            ChainElement::StaticMemberExpression(member) => {
                self.count_static_member_get_preserve_base(member, short_chain, ctx)
            }
            ChainElement::ComputedMemberExpression(member) => {
                self.count_computed_member_get_preserve_base(member, short_chain, ctx)
            }
            ChainElement::PrivateFieldExpression(member) => {
                self.count_chainable_expression(&member.object, short_chain, ctx);
                if short_chain && member.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.count_private_access();
            }
            ChainElement::CallExpression(call) => self.count_chain_call(call, short_chain, ctx),
            ChainElement::TSNonNullExpression(_) => {}
        }
    }

    pub(in crate::counter) fn count_logical_assign_test(&self, op: LogicalOperator, id: u32, ctx: &mut CompileCtx) {
        match op {
            LogicalOperator::And | LogicalOperator::Or => {
                ctx.projected_pc += 1;
            }
            LogicalOperator::Coalesce => {
                ctx.projected_pc += 1; // JMP_IF_NULLISH to store body
                ctx.projected_pc += 1; // JMP to end on non-nullish
                ctx.label_map.insert(Label::TernaryElse(id), ctx.projected_pc);
            }
        }
    }
}
