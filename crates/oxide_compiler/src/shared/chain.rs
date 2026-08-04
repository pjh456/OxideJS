use crate::compiler::{CompileCtx, Compiler, Label};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{ChainElement, Expression, LogicalOperator, PropertyKey};

impl Compiler {
    fn emit_optional_guard(&self, reg: u8, short_label: Label, ctx: &mut CompileCtx) -> Result<(), String> {
        let dup_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, reg, 0));
        ctx.emit_jmp_if_nullish_labeled(dup_reg, short_label);
        Ok(())
    }

    fn emit_static_member_get_preserve_base(
        &self, member: &oxide_parser::StaticMemberExpression, short_label: Option<Label>, ctx: &mut CompileCtx,
    ) -> Result<(u8, u8), String> {
        let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
        if member.optional {
            if let Some(label) = short_label {
                self.emit_optional_guard(obj_reg, label, ctx)?;
            }
        }
        let idx = ctx.add_constant(Constant::String(member.property.name.as_str().to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.emit_load_const(key_reg, idx);
        let value_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, value_reg, obj_reg, 0));
        ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, value_reg, key_reg));
        ctx.emit(0);
        ctx.emit(0);
        ctx.emit(0);
        Ok((value_reg, obj_reg))
    }

    fn emit_computed_member_get_preserve_base(
        &self, member: &oxide_parser::ComputedMemberExpression, short_label: Option<Label>, ctx: &mut CompileCtx,
    ) -> Result<(u8, u8), String> {
        let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
        if member.optional {
            if let Some(label) = short_label {
                self.emit_optional_guard(obj_reg, label, ctx)?;
            }
        }
        let key_reg = self.emit_expression(&member.expression, ctx)?;
        let value_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::GET_PROP_DYNAMIC, obj_reg, key_reg, value_reg));
        Ok((value_reg, obj_reg))
    }

    fn emit_chain_call(
        &self, call: &oxide_parser::CallExpression, short_label: Option<Label>, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let (callee_reg, this_reg) = match &call.callee {
            Expression::StaticMemberExpression(member) => {
                self.emit_static_member_get_preserve_base(member, short_label, ctx)?
            }
            Expression::ComputedMemberExpression(member) => {
                self.emit_computed_member_get_preserve_base(member, short_label, ctx)?
            }
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
                if member.optional {
                    if let Some(label) = short_label {
                        self.emit_optional_guard(obj_reg, label, ctx)?;
                    }
                }
                let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                let callee_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::GET_PRIVATE, callee_reg, obj_reg, key_reg));
                (callee_reg, obj_reg)
            }
            _ => {
                let callee_reg = self.emit_chainable_expression(&call.callee, short_label, ctx)?;
                if call.optional {
                    if let Some(label) = short_label {
                        self.emit_optional_guard(callee_reg, label, ctx)?;
                    }
                }
                let this_idx = ctx.add_constant(Constant::Undefined);
                let this_reg = ctx.alloc_reg();
                ctx.emit_load_const(this_reg, this_idx);
                (callee_reg, this_reg)
            }
        };
        if call.optional
            && matches!(
                &call.callee,
                Expression::StaticMemberExpression(_)
                    | Expression::ComputedMemberExpression(_)
                    | Expression::PrivateFieldExpression(_)
            )
        {
            if let Some(label) = short_label {
                self.emit_optional_guard(callee_reg, label, ctx)?;
            }
        }
        let mut arg_regs = Vec::new();
        for arg in &call.arguments {
            if let Some(expr) = arg.as_expression() {
                arg_regs.push(self.emit_expression(expr, ctx)?);
            }
        }
        let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
        let op = match &call.callee {
            Expression::Identifier(ident) if ctx.is_builtin(ident.name.as_str()) => OpCode::CALL_NATIVE,
            _ => OpCode::CALL,
        };
        ctx.emit(opcode::encode(op, callee_reg, this_reg, first_arg_reg));
        ctx.emit(arg_regs.len() as u32);
        let result_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, 0, 0));
        Ok(result_reg)
    }

    fn emit_chainable_expression(
        &self, expr: &Expression, short_label: Option<Label>, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        match expr {
            Expression::StaticMemberExpression(member) => {
                let (value_reg, _) = self.emit_static_member_get_preserve_base(member, short_label, ctx)?;
                Ok(value_reg)
            }
            Expression::ComputedMemberExpression(member) => {
                let (value_reg, _) = self.emit_computed_member_get_preserve_base(member, short_label, ctx)?;
                Ok(value_reg)
            }
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
                if member.optional {
                    if let Some(label) = short_label {
                        self.emit_optional_guard(obj_reg, label, ctx)?;
                    }
                }
                let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                let value_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::GET_PRIVATE, value_reg, obj_reg, key_reg));
                Ok(value_reg)
            }
            Expression::CallExpression(call) => self.emit_chain_call(call, short_label, ctx),
            Expression::ChainExpression(chain) => self.emit_chain_element(&chain.expression, short_label, ctx),
            _ => self.emit_expression(expr, ctx),
        }
    }

    pub(crate) fn emit_chain_element(
        &self, element: &ChainElement, short_label: Option<Label>, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        match element {
            ChainElement::StaticMemberExpression(member) => {
                let (value_reg, _) = self.emit_static_member_get_preserve_base(member, short_label, ctx)?;
                Ok(value_reg)
            }
            ChainElement::ComputedMemberExpression(member) => {
                let (value_reg, _) = self.emit_computed_member_get_preserve_base(member, short_label, ctx)?;
                Ok(value_reg)
            }
            ChainElement::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_chainable_expression(&member.object, short_label, ctx)?;
                if member.optional {
                    if let Some(label) = short_label {
                        self.emit_optional_guard(obj_reg, label, ctx)?;
                    }
                }
                let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                let value_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::GET_PRIVATE, value_reg, obj_reg, key_reg));
                Ok(value_reg)
            }
            ChainElement::CallExpression(call) => self.emit_chain_call(call, short_label, ctx),
            ChainElement::TSNonNullExpression(_) => Err("TS non-null expressions are not supported in JS mode".into()),
        }
    }

    pub(crate) fn emit_logical_assign_test(
        &self, op: LogicalOperator, test_reg: u8, store_label: Label, end_label: Label, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match op {
            LogicalOperator::And => ctx.emit_jmp_if_false_labeled(test_reg, end_label),
            LogicalOperator::Or => ctx.emit_jmp_if_true_labeled(test_reg, end_label),
            LogicalOperator::Coalesce => {
                ctx.emit_jmp_if_nullish_labeled(test_reg, store_label);
                ctx.emit_jmp_labeled(end_label);
            }
        }
        Ok(())
    }

    pub(crate) fn emit_object_property_read(&self, src_reg: u8, key: &str, ctx: &mut CompileCtx) -> u8 {
        let prop_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, prop_reg, src_reg, 0));
        let key_idx = ctx.add_constant(Constant::String(key.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.emit_load_const(key_reg, key_idx);
        ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, prop_reg, key_reg));
        ctx.emit(0);
        ctx.emit(0);
        ctx.emit(0);
        prop_reg
    }

    fn emit_property_key_expression(&self, key: &PropertyKey, ctx: &mut CompileCtx) -> Result<u8, String> {
        match key {
            PropertyKey::Identifier(ident) => {
                let name = ident.name.as_str();
                let var_reg = ctx.lookup_or_builtin(name)?;
                let key_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, key_reg, var_reg, 0));
                Ok(key_reg)
            }
            PropertyKey::StringLiteral(s) => {
                let key_idx = ctx.add_constant(Constant::String(s.value.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.emit_load_const(key_reg, key_idx);
                Ok(key_reg)
            }
            PropertyKey::NumericLiteral(n) => {
                let key_idx = ctx.add_constant(Constant::Number(n.value));
                let key_reg = ctx.alloc_reg();
                ctx.emit_load_const(key_reg, key_idx);
                Ok(key_reg)
            }
            PropertyKey::StaticIdentifier(ident) => {
                let key_idx = ctx.add_constant(Constant::String(ident.name.as_str().to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.emit_load_const(key_reg, key_idx);
                Ok(key_reg)
            }
            _ => Err("computed destructuring key expression not supported".into()),
        }
    }

    pub(crate) fn emit_object_property_read_key(
        &self, src_reg: u8, key: &PropertyKey, computed: bool, ctx: &mut CompileCtx,
    ) -> Result<(u8, Option<String>), String> {
        if !computed {
            let key_name = self.static_property_name(key)?;
            let prop_reg = self.emit_object_property_read(src_reg, &key_name, ctx);
            return Ok((prop_reg, Some(key_name)));
        }
        let prop_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, prop_reg, src_reg, 0));
        let key_reg = self.emit_property_key_expression(key, ctx)?;
        let val_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::GET_PROP_DYNAMIC, prop_reg, key_reg, val_reg));
        Ok((val_reg, None))
    }
}
