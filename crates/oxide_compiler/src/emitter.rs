use crate::compiler::Label;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{
    AssignmentOperator, AssignmentTarget, AssignmentTargetMaybeDefault, AssignmentTargetProperty, BindingPattern,
    ChainElement, Class, ClassElement, Expression, ForStatementInit, ForStatementLeft, LogicalOperator,
    MethodDefinitionKind, ObjectAssignmentTarget, PropertyKey, PropertyKind, SimpleAssignmentTarget, Statement,
    UnaryOperator, UpdateOperator, VariableDeclarationKind,
};

use crate::compiler::{
    is_int_literal, is_side_effect_free, BinaryOperator, CompileCtx, Compiler, FunctionBodyContext, ParamSpec,
};
impl Compiler {
    fn emit_optional_guard(&self, reg: u8, short_label: Label, ctx: &mut CompileCtx) -> Result<(), String> {
        let short_pos = ctx.resolve_label(short_label)?;
        let dup_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, reg, 0));
        let offset = (short_pos as isize) - (ctx.bytecode.len() as isize);
        let offset = ctx.checked_jump_offset(offset);
        ctx.emit(opcode::encode_jmp_if_nullish(dup_reg, offset));
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

    fn emit_chain_element(
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

    fn emit_logical_assign_test(
        &self, op: LogicalOperator, test_reg: u8, store_label: Label, end_label: Label, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match op {
            LogicalOperator::And => {
                let end_pos = ctx.resolve_label(end_label)?;
                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset));
            }
            LogicalOperator::Or => {
                let end_pos = ctx.resolve_label(end_label)?;
                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_true(test_reg, offset));
            }
            LogicalOperator::Coalesce => {
                let store_pos = ctx.resolve_label(store_label)?;
                let offset = (store_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_nullish(test_reg, offset));
                let end_pos = ctx.resolve_label(end_label)?;
                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));
            }
        }
        Ok(())
    }

    fn private_name_id(&self, name: &str, ctx: &CompileCtx) -> Result<u32, String> {
        ctx.private_name_map
            .iter()
            .find_map(|(n, id)| (n == name).then_some(*id))
            .ok_or_else(|| format!("private name #{name} is not defined"))
    }

    fn emit_private_id_reg(&self, name: &str, ctx: &mut CompileCtx) -> Result<u8, String> {
        let id = self.private_name_id(name, ctx)?;
        let idx = ctx.add_constant(Constant::Int(id as i32));
        let reg = ctx.alloc_reg();
        ctx.emit_load_const(reg, idx);
        Ok(reg)
    }

    fn emit_class_key_reg(&self, key: &PropertyKey, computed: bool, ctx: &mut CompileCtx) -> Result<u8, String> {
        if !computed {
            let name = self.class_property_name(key)?;
            let idx = ctx.add_constant(Constant::String(name));
            let reg = ctx.alloc_reg();
            ctx.emit_load_const(reg, idx);
            return Ok(reg);
        }

        if matches!(key, PropertyKey::PrivateIdentifier(_)) {
            return Err("private class elements not yet supported".into());
        }
        self.emit_expression(key.to_expression(), ctx)
    }

    fn count_class_key(&self, key: &PropertyKey, computed: bool, ctx: &mut CompileCtx) {
        if computed {
            self.count_expression(key.to_expression(), ctx);
        } else {
            ctx.alloc_reg();
            ctx.projected_pc += 1;
        }
    }

    fn emit_public_field_init(
        &self, target_reg: u8, key: &PropertyKey, computed: bool, value: Option<&Expression>, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let key_reg = self.emit_class_key_reg(key, computed, ctx)?;
        let value_reg = if let Some(expr) = value {
            self.emit_expression(expr, ctx)?
        } else {
            self.emit_undefined(ctx)
        };
        if computed {
            ctx.emit(opcode::encode(OpCode::SET_PROP_DYNAMIC, target_reg, key_reg, value_reg));
        } else {
            ctx.emit(opcode::encode(OpCode::SET_PROP, target_reg, value_reg, key_reg));
        }
        Ok(())
    }

    fn emit_private_field_init(
        &self, target_reg: u8, name: &str, value: Option<&Expression>, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let key_reg = self.emit_private_id_reg(name, ctx)?;
        let value_reg = if let Some(expr) = value {
            self.emit_expression(expr, ctx)?
        } else {
            self.emit_undefined(ctx)
        };
        ctx.emit(opcode::encode(OpCode::INIT_PRIVATE, target_reg, value_reg, key_reg));
        Ok(())
    }

    fn emit_private_method_init(
        &self, target_reg: u8, method: &oxide_parser::MethodDefinition, home_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let PropertyKey::PrivateIdentifier(private) = &method.key else {
            return Err("expected private method key".into());
        };
        let name = private.name.as_str();
        let key_reg = self.emit_private_id_reg(name, ctx)?;
        let method_reg = self.emit_class_method_function(method, name, home_reg, ctx, &[])?;
        ctx.emit(opcode::encode(OpCode::INIT_PRIVATE, target_reg, method_reg, key_reg));
        Ok(())
    }

    fn count_public_field_init(
        &self, key: &PropertyKey, computed: bool, value: Option<&Expression>, ctx: &mut CompileCtx,
    ) {
        self.count_class_key(key, computed, ctx);
        if let Some(expr) = value {
            self.count_expression(expr, ctx);
        } else {
            ctx.alloc_reg();
            ctx.projected_pc += 1;
        }
        ctx.projected_pc += 1;
    }

    fn count_private_field_init(&self, value: Option<&Expression>, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        ctx.projected_pc += 1;
        if let Some(expr) = value {
            self.count_expression(expr, ctx);
        } else {
            ctx.alloc_reg();
            ctx.projected_pc += 1;
        }
        ctx.projected_pc += 1;
    }

    fn static_property_name(&self, key: &PropertyKey) -> Result<String, String> {
        match key {
            PropertyKey::StaticIdentifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::Identifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::StringLiteral(s) => Ok(s.value.to_string()),
            PropertyKey::NumericLiteral(n) => Ok(n.value.to_string()),
            _ => Err("computed destructuring keys not yet supported".into()),
        }
    }

    fn emit_default_if_undefined(
        &self, val_reg: u8, default_expr: &Expression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let undef_reg = self.emit_undefined(ctx);
        let eq_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::STRICT_EQ, eq_reg, val_reg, undef_reg));
        let jump_pos = ctx.bytecode.len();
        ctx.emit(opcode::encode_jmp_if_false(eq_reg, 0));
        let default_reg = self.emit_expression(default_expr, ctx)?;
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, val_reg, default_reg, 0));
        let after = ctx.bytecode.len();
        let offset = after as isize - jump_pos as isize;
        let offset = ctx.checked_jump_offset(offset);
        ctx.bytecode[jump_pos] = opcode::encode_jmp_if_false(eq_reg, offset);
        Ok(val_reg)
    }

    fn emit_bind_target(
        &self, name: &str, src_reg: u8, kind: VariableDeclarationKind, is_const: bool, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let var_reg = ctx.alloc_reg();
        ctx.declare(name, var_reg, kind, is_const)?;
        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, src_reg, if is_const { 1 } else { 0 }));
        ctx.init_var(name);
        Ok(())
    }

    fn emit_assign_target(&self, target: &AssignmentTarget, src_reg: u8, ctx: &mut CompileCtx) -> Result<(), String> {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.emit(opcode::encode(
                    OpCode::STORE_VAR,
                    var_reg,
                    src_reg,
                    if ctx.lookup_const_flag(name) { 1 } else { 0 },
                ));
                Ok(())
            }
            AssignmentTarget::ArrayAssignmentTarget(ap) => self.emit_array_assignment(ap, src_reg, ctx),
            AssignmentTarget::ObjectAssignmentTarget(op) => self.emit_object_assignment(op, src_reg, ctx),
            _ => Err("assignment target not supported".into()),
        }
    }

    pub(crate) fn emit_binding_pattern(
        &self, pattern: &BindingPattern, src_reg: u8, kind: VariableDeclarationKind, is_const: bool,
        ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match pattern {
            BindingPattern::BindingIdentifier(bi) => {
                self.emit_bind_target(bi.name.as_str(), src_reg, kind, is_const, ctx)
            }
            BindingPattern::ArrayPattern(ap) => self.emit_array_binding(ap, src_reg, kind, is_const, ctx),
            BindingPattern::ObjectPattern(op) => self.emit_object_binding(op, src_reg, kind, is_const, ctx),
            BindingPattern::AssignmentPattern(ap) => {
                let val_reg = self.emit_default_if_undefined(src_reg, &ap.right, ctx)?;
                self.emit_binding_pattern(&ap.left, val_reg, kind, is_const, ctx)
            }
        }
    }

    fn emit_array_binding(
        &self, ap: &oxide_parser::ArrayPattern, src_reg: u8, kind: VariableDeclarationKind, is_const: bool,
        ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        ctx.emit(opcode::encode(OpCode::FOR_OF_INIT, 0, src_reg, 0));
        for elem in &ap.elements {
            let has_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::FOR_OF_DONE, has_reg, 0, 0));
            let val_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::FOR_OF_NEXT, val_reg, 0, 0));
            if let Some(pattern) = elem {
                self.emit_binding_pattern(pattern, val_reg, kind, is_const, ctx)?;
            }
        }
        if let Some(rest) = &ap.rest {
            let rest_reg = self.emit_collect_rest_array(ctx)?;
            self.emit_binding_pattern(&rest.argument, rest_reg, kind, is_const, ctx)?;
        }
        ctx.emit(opcode::encode(OpCode::FOR_OF_CLOSE, 0, 0, 0));
        Ok(())
    }

    fn emit_collect_rest_array(&self, ctx: &mut CompileCtx) -> Result<u8, String> {
        let rest_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::NEW_ARRAY, rest_reg, 0, 0));
        let idx_reg = ctx.alloc_reg();
        let zero_idx = ctx.add_constant(Constant::Int(0));
        ctx.emit_load_const(idx_reg, zero_idx);
        let loop_start = ctx.bytecode.len();
        let has_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_OF_DONE, has_reg, 0, 0));
        let end_jmp = ctx.bytecode.len();
        ctx.emit(opcode::encode_jmp_if_false(has_reg, 0));
        let val_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::FOR_OF_NEXT, val_reg, 0, 0));
        ctx.emit(opcode::encode(OpCode::SET_ELEM, rest_reg, idx_reg, val_reg));
        let tmp_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::INC_PRE, idx_reg, tmp_reg, tmp_reg));
        let back = loop_start as isize - ctx.bytecode.len() as isize;
        let back = ctx.checked_jump_offset(back);
        ctx.emit(opcode::encode_jmp(back));
        let after = ctx.bytecode.len();
        let end_offset = after as isize - end_jmp as isize;
        let end_offset = ctx.checked_jump_offset(end_offset);
        ctx.bytecode[end_jmp] = opcode::encode_jmp_if_false(has_reg, end_offset);
        Ok(rest_reg)
    }

    fn emit_object_property_read(&self, src_reg: u8, key: &str, ctx: &mut CompileCtx) -> u8 {
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

    fn emit_object_property_read_key(
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

    fn emit_object_binding(
        &self, op: &oxide_parser::ObjectPattern, src_reg: u8, kind: VariableDeclarationKind, is_const: bool,
        ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let mut excluded = Vec::new();
        for prop in &op.properties {
            let (prop_reg, static_key) = self.emit_object_property_read_key(src_reg, &prop.key, prop.computed, ctx)?;
            if let Some(key) = static_key {
                excluded.push(key);
            }
            self.emit_binding_pattern(&prop.value, prop_reg, kind, is_const, ctx)?;
        }
        if let Some(rest) = &op.rest {
            let rest_reg = ctx.alloc_reg();
            let excluded_idx = ctx.add_constant(Constant::String(excluded.join("\0")));
            ctx.emit(opcode::encode(OpCode::REST_OBJECT, rest_reg, src_reg, 0));
            ctx.emit(excluded_idx as u32);
            self.emit_binding_pattern(&rest.argument, rest_reg, kind, is_const, ctx)?;
        }
        Ok(())
    }

    fn emit_assignment_maybe_default(
        &self, target: &AssignmentTargetMaybeDefault, src_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(default) => {
                let val_reg = self.emit_default_if_undefined(src_reg, &default.init, ctx)?;
                self.emit_assign_target(&default.binding, val_reg, ctx)
            }
            AssignmentTargetMaybeDefault::ArrayAssignmentTarget(ap) => self.emit_array_assignment(ap, src_reg, ctx),
            AssignmentTargetMaybeDefault::ObjectAssignmentTarget(op) => self.emit_object_assignment(op, src_reg, ctx),
            AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                let var_reg = ctx.lookup_or_global(name);
                ctx.emit(opcode::encode(
                    OpCode::STORE_VAR,
                    var_reg,
                    src_reg,
                    if ctx.lookup_const_flag(name) { 1 } else { 0 },
                ));
                Ok(())
            }
            _ => Err("assignment target not supported".into()),
        }
    }

    fn emit_array_assignment(
        &self, ap: &oxide_parser::ArrayAssignmentTarget, src_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        ctx.emit(opcode::encode(OpCode::FOR_OF_INIT, 0, src_reg, 0));
        for elem in &ap.elements {
            let has_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::FOR_OF_DONE, has_reg, 0, 0));
            let val_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::FOR_OF_NEXT, val_reg, 0, 0));
            if let Some(target) = elem {
                self.emit_assignment_maybe_default(target, val_reg, ctx)?;
            }
        }
        if let Some(rest) = &ap.rest {
            let rest_reg = self.emit_collect_rest_array(ctx)?;
            self.emit_assign_target(&rest.target, rest_reg, ctx)?;
        }
        ctx.emit(opcode::encode(OpCode::FOR_OF_CLOSE, 0, 0, 0));
        Ok(())
    }

    fn emit_object_assignment(
        &self, op: &ObjectAssignmentTarget, src_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        let mut excluded = Vec::new();
        for prop in &op.properties {
            match prop {
                AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
                    let key = id.binding.name.as_str().to_string();
                    excluded.push(key.clone());
                    let mut prop_reg = self.emit_object_property_read(src_reg, &key, ctx);
                    if let Some(default_expr) = &id.init {
                        prop_reg = self.emit_default_if_undefined(prop_reg, default_expr, ctx)?;
                    }
                    let name = id.binding.name.as_str();
                    let var_reg = ctx.lookup_or_global(name);
                    ctx.emit(opcode::encode(
                        OpCode::STORE_VAR,
                        var_reg,
                        prop_reg,
                        if ctx.lookup_const_flag(name) { 1 } else { 0 },
                    ));
                }
                AssignmentTargetProperty::AssignmentTargetPropertyProperty(prop) => {
                    let (prop_reg, static_key) =
                        self.emit_object_property_read_key(src_reg, &prop.name, prop.computed, ctx)?;
                    if let Some(key) = static_key {
                        excluded.push(key);
                    }
                    self.emit_assignment_maybe_default(&prop.binding, prop_reg, ctx)?;
                }
            }
        }
        if let Some(rest) = &op.rest {
            let rest_reg = ctx.alloc_reg();
            let excluded_idx = ctx.add_constant(Constant::String(excluded.join("\0")));
            ctx.emit(opcode::encode(OpCode::REST_OBJECT, rest_reg, src_reg, 0));
            ctx.emit(excluded_idx as u32);
            self.emit_assign_target(&rest.target, rest_reg, ctx)?;
        }
        Ok(())
    }

    fn class_property_name(&self, key: &PropertyKey) -> Result<String, String> {
        match key {
            PropertyKey::StaticIdentifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::Identifier(ident) => Ok(ident.name.as_str().to_string()),
            PropertyKey::StringLiteral(s) => Ok(s.value.to_string()),
            PropertyKey::NumericLiteral(n) => Ok(n.value.to_string()),
            PropertyKey::PrivateIdentifier(_) => Err("private class elements not yet supported".into()),
            _ => Err("unsupported class property key type".into()),
        }
    }

    fn emit_class(&self, class: &Class, ctx: &mut CompileCtx) -> Result<u8, String> {
        let mut constructor_method = None;
        let mut instance_fields = Vec::new();
        let mut private_names = Vec::<(String, u32)>::new();
        let is_derived = class.super_class.is_some();

        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let method = method.as_ref();
                    if let PropertyKey::PrivateIdentifier(private) = &method.key {
                        let name = private.name.as_str().to_string();
                        if private_names.iter().any(|(existing, _)| existing == &name) {
                            return Err(format!("duplicate private name #{name}"));
                        }
                        let id = ctx.next_private_name_id;
                        ctx.next_private_name_id = ctx.next_private_name_id.saturating_add(1);
                        private_names.push((name, id));
                    }
                    if method.kind == MethodDefinitionKind::Constructor {
                        if constructor_method.is_some() {
                            return Err("duplicate class constructor".into());
                        }
                        constructor_method = Some(method);
                    }
                }
                ClassElement::PropertyDefinition(prop) => {
                    let prop = prop.as_ref();
                    if let PropertyKey::PrivateIdentifier(private) = &prop.key {
                        let name = private.name.as_str().to_string();
                        if private_names.iter().any(|(existing, _)| existing == &name) {
                            return Err(format!("duplicate private name #{name}"));
                        }
                        let id = ctx.next_private_name_id;
                        ctx.next_private_name_id = ctx.next_private_name_id.saturating_add(1);
                        private_names.push((name, id));
                    }
                    if !prop.r#static {
                        instance_fields.push(prop);
                    }
                }
                ClassElement::AccessorProperty(_) => return Err("class accessor properties not yet supported".into()),
                ClassElement::StaticBlock(_) | ClassElement::TSIndexSignature(_) => {}
            }
        }

        let ctor_name = class.id.as_ref().map(|id| id.name.to_string());
        let ctor_reg = ctx.alloc_reg();
        let proto_reg = ctx.alloc_reg();
        let self_binding = ctor_name.as_deref().map(|name| vec![(name, ctor_reg)]).unwrap_or_default();
        let super_reg = if let Some(super_expr) = &class.super_class {
            Some(self.emit_expression(super_expr, ctx)?)
        } else {
            None
        };

        let saved_derived = ctx.in_derived_constructor;
        let saved_private_names = ctx.private_name_map.clone();
        ctx.in_derived_constructor = is_derived;
        ctx.private_name_map = private_names.clone();

        let mut ctor_module = if let Some(method) = constructor_method {
            let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
            self.compile_function_body_with_field_hooks(
                &param_names,
                body_stmts,
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
                Some(|compiler: &Compiler, field_ctx: &mut CompileCtx| {
                    for field in &instance_fields {
                        if matches!(field.key, PropertyKey::PrivateIdentifier(_)) {
                            compiler.count_private_field_init(field.value.as_ref(), field_ctx);
                        } else {
                            compiler.count_public_field_init(
                                &field.key,
                                field.computed,
                                field.value.as_ref(),
                                field_ctx,
                            );
                        }
                    }
                }),
                Some(|compiler: &Compiler, field_ctx: &mut CompileCtx| -> Result<(), String> {
                    for field in &instance_fields {
                        if let PropertyKey::PrivateIdentifier(private) = &field.key {
                            compiler.emit_private_field_init(
                                254,
                                private.name.as_str(),
                                field.value.as_ref(),
                                field_ctx,
                            )?;
                        } else {
                            compiler.emit_public_field_init(
                                254,
                                &field.key,
                                field.computed,
                                field.value.as_ref(),
                                field_ctx,
                            )?;
                        }
                    }
                    Ok(())
                }),
                is_derived,
            )?
        } else {
            let mut module = self.compile_function_body_with_field_hooks(
                &[],
                &[],
                ctx,
                false,
                &self_binding,
                FunctionBodyContext::ClassElement,
                Some(|compiler: &Compiler, field_ctx: &mut CompileCtx| {
                    for field in &instance_fields {
                        if matches!(field.key, PropertyKey::PrivateIdentifier(_)) {
                            compiler.count_private_field_init(field.value.as_ref(), field_ctx);
                        } else {
                            compiler.count_public_field_init(
                                &field.key,
                                field.computed,
                                field.value.as_ref(),
                                field_ctx,
                            );
                        }
                    }
                }),
                Some(|compiler: &Compiler, field_ctx: &mut CompileCtx| -> Result<(), String> {
                    for field in &instance_fields {
                        if let PropertyKey::PrivateIdentifier(private) = &field.key {
                            compiler.emit_private_field_init(
                                254,
                                private.name.as_str(),
                                field.value.as_ref(),
                                field_ctx,
                            )?;
                        } else {
                            compiler.emit_public_field_init(
                                254,
                                &field.key,
                                field.computed,
                                field.value.as_ref(),
                                field_ctx,
                            )?;
                        }
                    }
                    Ok(())
                }),
                is_derived,
            )?;
            if is_derived {
                module.bytecode.clear();
                module.constants.clear();
                module.n_registers = 1;
                module.bytecode.push(opcode::encode(OpCode::SUPER_CALL, 0, 0, 0));
                module.bytecode.push(0);
                let mut field_ctx = CompileCtx::new();
                field_ctx.private_name_map = private_names.clone();
                for field in &instance_fields {
                    if let PropertyKey::PrivateIdentifier(private) = &field.key {
                        self.emit_private_field_init(254, private.name.as_str(), field.value.as_ref(), &mut field_ctx)?;
                    } else {
                        self.emit_public_field_init(
                            254,
                            &field.key,
                            field.computed,
                            field.value.as_ref(),
                            &mut field_ctx,
                        )?;
                    }
                }
                module.bytecode.extend(field_ctx.bytecode);
                module.constants = field_ctx.constants;
                module.n_registers = field_ctx.max_regs.max(1);
                module.bytecode.push(opcode::encode(OpCode::RETURN, 0, 0, 0));
            }
            module
        };
        ctx.in_derived_constructor = saved_derived;
        ctor_module.is_class_constructor = true;
        ctor_module.is_derived_constructor = is_derived;
        ctor_module.function_name = ctor_name.clone();
        ctx.sub_modules.push(ctor_module);

        let ctor_idx = ctx.add_constant(Constant::BytecodeFunc(ctx.sub_modules.len() as u32));
        ctx.emit_load_const(ctor_reg, ctor_idx);
        ctx.emit(opcode::encode(OpCode::NEW_OBJECT, proto_reg, 0, 0));

        if let Some(super_reg) = super_reg {
            let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
            let parent_proto_key_reg = ctx.alloc_reg();
            ctx.emit_load_const(parent_proto_key_reg, proto_key_idx);
            let parent_proto_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::GET_PROP, super_reg, parent_proto_reg, parent_proto_key_reg));

            let proto_link_idx = ctx.add_constant(Constant::String("__proto__".to_string()));
            let proto_link_key_reg = ctx.alloc_reg();
            ctx.emit_load_const(proto_link_key_reg, proto_link_idx);
            ctx.emit(opcode::encode(OpCode::SET_PROP, proto_reg, parent_proto_reg, proto_link_key_reg));
            ctx.emit(opcode::encode(OpCode::SET_PROP, ctor_reg, super_reg, proto_link_key_reg));
        }

        let ctor_key_idx = ctx.add_constant(Constant::String("constructor".to_string()));
        let ctor_key_reg = ctx.alloc_reg();
        ctx.emit_load_const(ctor_key_reg, ctor_key_idx);
        ctx.emit(opcode::encode(OpCode::SET_PROP, proto_reg, ctor_reg, ctor_key_reg));

        let proto_key_idx = ctx.add_constant(Constant::String("prototype".to_string()));
        let proto_key_reg = ctx.alloc_reg();
        ctx.emit_load_const(proto_key_reg, proto_key_idx);
        ctx.emit(opcode::encode(OpCode::SET_PROP, ctor_reg, proto_reg, proto_key_reg));

        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let method = method.as_ref();
                    if method.kind == MethodDefinitionKind::Constructor {
                        continue;
                    }
                    if matches!(method.key, PropertyKey::PrivateIdentifier(_)) {
                        let home_reg = if method.r#static { ctor_reg } else { proto_reg };
                        self.emit_private_method_init(home_reg, method, home_reg, ctx)?;
                        continue;
                    }
                    let home_reg = if method.r#static { ctor_reg } else { proto_reg };
                    let key_reg = self.emit_class_key_reg(&method.key, method.computed, ctx)?;
                    let method_name = if method.computed {
                        "<computed>".to_string()
                    } else {
                        self.class_property_name(&method.key)?
                    };
                    let accessor_reg =
                        self.emit_class_method_function(method, &method_name, home_reg, ctx, &self_binding)?;
                    match method.kind {
                        MethodDefinitionKind::Method => {
                            if method.computed {
                                ctx.emit(opcode::encode(OpCode::SET_PROP_DYNAMIC, home_reg, key_reg, accessor_reg));
                            } else {
                                ctx.emit(opcode::encode(OpCode::SET_PROP, home_reg, accessor_reg, key_reg));
                            }
                        }
                        MethodDefinitionKind::Get | MethodDefinitionKind::Set => {
                            if method.computed {
                                return Err("computed class accessors not yet supported".into());
                            }
                            let undef_reg = self.emit_undefined(ctx);
                            let (get_reg, set_reg) = match method.kind {
                                MethodDefinitionKind::Get => (accessor_reg, undef_reg),
                                MethodDefinitionKind::Set => (undef_reg, accessor_reg),
                                _ => unreachable!(),
                            };
                            let key_name = self.class_property_name(&method.key)?;
                            let key_idx = ctx.add_constant(Constant::String(key_name));
                            ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, home_reg, get_reg, set_reg));
                            ctx.emit(key_idx as u32);
                        }
                        MethodDefinitionKind::Constructor => unreachable!(),
                    }
                }
                ClassElement::PropertyDefinition(prop) => {
                    let prop = prop.as_ref();
                    if prop.r#static {
                        let saved_static_this = ctx.static_block_this_reg;
                        ctx.static_block_this_reg = Some(ctor_reg);
                        if let PropertyKey::PrivateIdentifier(private) = &prop.key {
                            self.emit_private_field_init(ctor_reg, private.name.as_str(), prop.value.as_ref(), ctx)?;
                        } else {
                            self.emit_public_field_init(ctor_reg, &prop.key, prop.computed, prop.value.as_ref(), ctx)?;
                        }
                        ctx.static_block_this_reg = saved_static_this;
                    }
                }
                ClassElement::StaticBlock(block) => {
                    let saved_static_this = ctx.static_block_this_reg;
                    ctx.static_block_this_reg = Some(ctor_reg);
                    ctx.push_scope();
                    for stmt in &block.body {
                        self.emit_statement(stmt, ctx)?;
                    }
                    ctx.pop_scope();
                    ctx.static_block_this_reg = saved_static_this;
                }
                ClassElement::AccessorProperty(_) | ClassElement::TSIndexSignature(_) => {}
            }
        }

        ctx.private_name_map = saved_private_names;
        Ok(ctor_reg)
    }

    fn emit_undefined(&self, ctx: &mut CompileCtx) -> u8 {
        let idx = ctx.add_constant(Constant::Undefined);
        let reg = ctx.alloc_reg();
        ctx.emit_load_const(reg, idx);
        reg
    }

    fn emit_class_method_function(
        &self, method: &oxide_parser::MethodDefinition, method_name: &str, home_reg: u8, ctx: &mut CompileCtx,
        self_binding: &[(&str, u8)],
    ) -> Result<u8, String> {
        let (param_names, body_stmts) = self.extract_function_parts(method.value.as_ref())?;
        let saved_instance = ctx.in_instance_method;
        let saved_static = ctx.in_static_method;
        ctx.in_instance_method = !method.r#static;
        ctx.in_static_method = method.r#static;
        let mut method_module = self.compile_function_body_with_bindings(
            &param_names,
            body_stmts,
            ctx,
            false,
            self_binding,
            FunctionBodyContext::ClassElement,
        )?;
        ctx.in_instance_method = saved_instance;
        ctx.in_static_method = saved_static;
        method_module.function_name = Some(method_name.to_string());
        method_module.needs_home_object = true;
        ctx.sub_modules.push(method_module);

        let method_idx = ctx.add_constant(Constant::BytecodeFunc(ctx.sub_modules.len() as u32));
        let method_reg = ctx.alloc_reg();
        ctx.emit_load_const(method_reg, method_idx);
        ctx.emit(opcode::encode(OpCode::SET_HOME_OBJECT, method_reg, home_reg, 0));
        Ok(method_reg)
    }

    pub(crate) fn emit_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::ExpressionStatement(es) => Ok(Some(self.emit_expression(&es.expression, ctx)?)),
            Statement::VariableDeclaration(decl) => {
                let mut r = None;
                for d in &decl.declarations {
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    if is_const && d.init.is_none() {
                        return Err("const declaration must have an initializer".into());
                    }
                    if let Some(init) = &d.init {
                        let val_reg = self.emit_expression(init, ctx)?;
                        self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, ctx)?;
                        // Name inference (D-04): if the initializer is an arrow function,
                        // set the compiled sub_module's function_name.
                        if let BindingPattern::BindingIdentifier(bi) = &d.id {
                            if matches!(*init, Expression::ArrowFunctionExpression(_)) {
                                if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                                    sub_mod.function_name = Some(bi.name.to_string());
                                }
                            }
                        }
                        r = Some(val_reg);
                    } else {
                        let BindingPattern::BindingIdentifier(bi) = &d.id else {
                            return Err("destructuring declaration requires an initializer".into());
                        };
                        let idx = ctx.add_constant(Constant::Undefined);
                        let tmp = ctx.alloc_reg();
                        ctx.emit_load_const(tmp, idx);
                        let var_reg = ctx.alloc_reg();
                        ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const)?;
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, tmp, 0));
                        ctx.init_var(bi.name.as_str());
                        r = Some(var_reg);
                    }
                }
                Ok(r)
            }
            Statement::ReturnStatement(ret) => {
                match &ret.argument {
                    Some(expr) => {
                        let r = self.emit_expression(expr, ctx)?;
                        ctx.emit(opcode::encode(OpCode::RETURN, r, 0, 0));
                    }
                    None => {
                        ctx.emit(opcode::encode(OpCode::RETURN, 0, 0, 0));
                    }
                }
                Ok(None)
            }
            Statement::BlockStatement(block) => {
                ctx.push_scope();
                let mut r = None;
                for s in &block.body {
                    if let Some(rr) = self.emit_statement(s, ctx)? {
                        r = Some(rr);
                    }
                }
                ctx.pop_scope();
                Ok(r)
            }
            Statement::IfStatement(ifs) => {
                let id = ctx.next_label_id();
                let else_label = Label::IfElse(id);
                let end_label = Label::IfEnd(id);

                let test_reg = self.emit_expression(&ifs.test, ctx)?;

                let else_pos = ctx.resolve_label(else_label)?;
                let offset = (else_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset));

                let cons_reg = self.emit_statement(&ifs.consequent, ctx)?;
                let result_reg = ctx.alloc_reg();
                if let Some(r) = cons_reg {
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
                } else {
                    let undef_idx = ctx.add_constant(Constant::Undefined);
                    ctx.emit_load_const(result_reg, undef_idx);
                }

                let has_alt = ifs.alternate.is_some();
                if has_alt {
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    let offset = ctx.checked_jump_offset(offset);
                    ctx.emit(opcode::encode_jmp(offset));
                }

                if let Some(alt) = &ifs.alternate {
                    let alt_reg = self.emit_statement(alt, ctx)?;
                    if let Some(r) = alt_reg {
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
                    } else {
                        let undef_idx = ctx.add_constant(Constant::Undefined);
                        ctx.emit_load_const(result_reg, undef_idx);
                    }
                }

                Ok(Some(result_reg))
            }
            Statement::WhileStatement(wh) => {
                let id = ctx.next_label_id();
                let start_label = Label::WhileStart(id);
                let end_label = Label::WhileEnd(id);

                ctx.push_loop(end_label, start_label);

                let test_reg = self.emit_expression(&wh.test, ctx)?;

                let end_pos = ctx.resolve_label(end_label)?;
                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset));

                self.emit_statement(&wh.body, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::DoWhileStatement(dw) => {
                let id = ctx.next_label_id();
                let start_label = Label::DoWhileStart(id);
                let end_label = Label::DoWhileEnd(id);

                ctx.push_loop(end_label, start_label);

                self.emit_statement(&dw.body, ctx)?;

                let test_reg = self.emit_expression(&dw.test, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_true(test_reg, offset));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::ForStatement(fr) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForStart(id);
                let update_label = Label::ForUpdate(id);
                let end_label = Label::ForEnd(id);

                ctx.push_loop(end_label, update_label);

                if let Some(init) = &fr.init {
                    if let Some(expr) = init.as_expression() {
                        self.emit_expression(expr, ctx)?;
                    } else if let ForStatementInit::VariableDeclaration(decl) = init {
                        for d in &decl.declarations {
                            let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                            if let Some(init_expr) = &d.init {
                                let val_reg = self.emit_expression(init_expr, ctx)?;
                                self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, ctx)?;
                            } else if let BindingPattern::BindingIdentifier(bi) = &d.id {
                                let var_reg = ctx.alloc_reg();
                                ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const)?;
                            }
                        }
                    }
                }

                if let Some(test) = &fr.test {
                    let test_reg = self.emit_expression(test, ctx)?;
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    let offset = ctx.checked_jump_offset(offset);
                    ctx.emit(opcode::encode_jmp_if_false(test_reg, offset));
                }

                self.emit_statement(&fr.body, ctx)?;

                if let Some(update) = &fr.update {
                    self.emit_expression(update, ctx)?;
                }

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));

                ctx.pop_loop();

                Ok(None)
            }
            Statement::ForInStatement(fi) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForInStart(id);
                let end_label = Label::ForInEnd(id);

                let obj_reg = self.emit_expression(&fi.right, ctx)?;
                ctx.emit(opcode::encode(OpCode::FOR_IN_INIT, 0, obj_reg, 0));

                ctx.push_loop(end_label, start_label);

                let done_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::FOR_IN_DONE, done_reg, 0, 0));

                let end_pos = ctx.resolve_label(end_label)?;
                let cleanup_jmp_offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(done_reg, 2));
                let cleanup_jmp_offset = ctx.checked_jump_offset(cleanup_jmp_offset);
                ctx.emit(opcode::encode_jmp(cleanup_jmp_offset));

                let key_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::FOR_IN_NEXT, key_reg, 0, 0));

                match &fi.left {
                    ForStatementLeft::VariableDeclaration(decl) => {
                        for d in &decl.declarations {
                            let name = match &d.id {
                                oxide_parser::BindingPattern::BindingIdentifier(bi) => bi.name.as_str(),
                                _ => return Err("destructuring not supported".into()),
                            };
                            let var_reg = ctx.alloc_reg();
                            ctx.declare(name, var_reg, decl.kind, matches!(decl.kind, VariableDeclarationKind::Const))?;
                            ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
                            ctx.init_var(name);
                        }
                    }
                    ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                        let name = id_ref.name.as_str();
                        let var_reg = ctx.lookup_or_global(name);
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
                    }
                    _ => return Err("unsupported for-in left-hand side".into()),
                }

                self.emit_statement(&fi.body, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));

                ctx.emit(opcode::encode(OpCode::FOR_IN_CLEANUP, 0, 0, 0));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::ForOfStatement(fo) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForOfStart(id);
                let end_label = Label::ForOfEnd(id);

                let iter_src_reg = self.emit_expression(&fo.right, ctx)?;
                ctx.emit(opcode::encode(OpCode::FOR_OF_INIT, 0, iter_src_reg, 0));
                ctx.push_loop(end_label, start_label);

                let has_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::FOR_OF_DONE, has_reg, 0, 0));
                let end_pos = ctx.resolve_label(end_label)?;
                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_false(has_reg, offset));

                let val_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::FOR_OF_NEXT, val_reg, 0, 0));
                match &fo.left {
                    ForStatementLeft::VariableDeclaration(decl) => {
                        for d in &decl.declarations {
                            self.emit_binding_pattern(&d.id, val_reg, decl.kind, false, ctx)?;
                        }
                    }
                    ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                        let name = id_ref.name.as_str();
                        let var_reg = ctx.lookup_or_global(name);
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, 0));
                    }
                    ForStatementLeft::ArrayAssignmentTarget(ap) => {
                        self.emit_array_assignment(ap, val_reg, ctx)?;
                    }
                    ForStatementLeft::ObjectAssignmentTarget(op) => {
                        self.emit_object_assignment(op, val_reg, ctx)?;
                    }
                    _ => return Err("unsupported for-of left-hand side".into()),
                }

                self.emit_statement(&fo.body, ctx)?;
                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));
                ctx.emit(opcode::encode(OpCode::FOR_OF_CLOSE, 0, 0, 0));
                ctx.pop_loop();
                Ok(None)
            }
            Statement::SwitchStatement(sw) => {
                let id = ctx.next_label_id();
                let end_label = Label::SwitchEnd(id);
                ctx.push_switch(end_label);

                let disc_reg = self.emit_expression(&sw.discriminant, ctx)?;
                let compare_reg_checkpoint = ctx.reg_checkpoint();
                let cases = &sw.cases;

                for (case_idx, case) in cases.iter().enumerate() {
                    if let Some(test) = &case.test {
                        let test_reg = self.emit_expression(test, ctx)?;
                        let eq_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::EQ, eq_reg, disc_reg, test_reg));

                        let case_label = Label::SwitchCase(id, case_idx as u32);
                        let body_pos = ctx.resolve_label(case_label)?;
                        let offset = (body_pos as isize) - (ctx.bytecode.len() as isize);
                        let offset = ctx.checked_jump_offset(offset);
                        ctx.emit(opcode::encode_jmp_if_true(eq_reg, offset));
                        ctx.restore_reg_checkpoint(compare_reg_checkpoint);
                    }
                }

                let has_default = cases.iter().any(|c| c.test.is_none());
                if !has_default {
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    let offset = ctx.checked_jump_offset(offset);
                    ctx.emit(opcode::encode_jmp(offset));
                }

                for case in cases.iter() {
                    for s in &case.consequent {
                        self.emit_statement(s, ctx)?;
                    }
                }

                ctx.pop_switch();
                Ok(None)
            }
            Statement::BreakStatement(_) => {
                let break_label = if let Some(sw_label) = ctx.current_switch() {
                    *sw_label
                } else {
                    let (bl, _) = ctx.current_loop().ok_or("break outside switch or loop".to_string())?;
                    *bl
                };
                let break_pos = ctx.resolve_label(break_label)?;
                let offset = (break_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));
                Ok(None)
            }
            Statement::ContinueStatement(_) => {
                let (_, continue_label) = ctx.current_loop().ok_or("continue outside loop".to_string())?;
                let continue_pos = ctx.resolve_label(*continue_label)?;
                let offset = (continue_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));
                Ok(None)
            }
            Statement::FunctionDeclaration(fd) => {
                // FunctionDeclaration: emit LOAD_CONST(BytecodeFunc) + STORE_VAR
                let name = if let Some(id) = &fd.id {
                    id.name.to_string()
                } else {
                    return Err("FunctionDeclaration without name".into());
                };

                // Extract params
                let mut param_names = Vec::new();
                for (idx, param) in fd.params.items.iter().enumerate() {
                    match &param.pattern {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                            param_names.push(ParamSpec::Identifier(bi.name.to_string()));
                        }
                        pattern => {
                            param_names.push(ParamSpec::Pattern {
                                synthetic_name: format!("@@param_{idx}"),
                                pattern,
                            });
                        }
                    }
                }

                // Extract body statements (pass by reference)
                let body_stmts: &[Statement] = if let Some(body) = &fd.body { &body.statements } else { &[] };

                // Compile body into sub-module
                let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, false, false)?;
                sub_module.function_name = Some(name.clone());
                ctx.sub_modules.push(sub_module);
                // 1-indexed: 0 = no sub_module (sentinel)
                let sub_idx = ctx.sub_modules.len() as u32;

                // Use the register allocated during the count/hoist pass. Calling
                // lookup_or_global here would allocate an extra register during
                // emission and desynchronize later function declaration bindings.
                let var_reg = ctx.lookup(&name)?;
                ctx.reserve_reg(var_reg);
                let const_idx = ctx.add_constant(Constant::BytecodeFunc(sub_idx));
                ctx.emit_load_const(var_reg, const_idx);
                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, var_reg, 0));

                Ok(None)
            }
            Statement::ClassDeclaration(class) => {
                let name = class
                    .id
                    .as_ref()
                    .map(|id| id.name.to_string())
                    .ok_or_else(|| "ClassDeclaration without name".to_string())?;
                let var_reg = ctx.alloc_reg();
                ctx.declare(&name, var_reg, VariableDeclarationKind::Let, false)?;
                ctx.init_var(&name);
                let ctor_reg = self.emit_class(class, ctx)?;
                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, ctor_reg, 0));
                Ok(None)
            }
            Statement::ThrowStatement(ts) => {
                let exc_reg = self.emit_expression(&ts.argument, ctx)?;
                ctx.emit(opcode::encode(OpCode::THROW, exc_reg, 0, 0));
                Ok(None)
            }
            Statement::TryStatement(ts) => {
                let id = ctx.next_label_id();
                let catch_label = Label::CatchBody(id);
                let try_end_label = Label::TryEnd(id);
                let has_catch = ts.handler.is_some();
                let has_finally = ts.finalizer.is_some();

                let result_reg = ctx.alloc_reg();
                let mut try_finally_begin_pos: Option<usize> = None;
                let mut try_begin_pos: Option<usize> = None;

                if has_finally {
                    try_finally_begin_pos = Some(ctx.bytecode.len());
                    ctx.emit(opcode::encode_try_finally_begin(0)); // placeholder
                }

                if has_catch {
                    try_begin_pos = Some(ctx.bytecode.len());
                    ctx.emit(opcode::encode_try_begin(0)); // placeholder
                }

                let mut last_try_result: Option<u8> = None;
                for s in &ts.block.body {
                    if let Some(r) = self.emit_statement(s, ctx)? {
                        last_try_result = Some(r);
                    }
                }
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, last_try_result.unwrap_or(result_reg), 0));

                if has_catch {
                    ctx.emit(opcode::encode(OpCode::TRY_END, 0, 0, 0));
                }

                let jmp_needed = has_catch || has_finally;
                let jmp_skip_pos = if jmp_needed {
                    let pos = ctx.bytecode.len();
                    ctx.emit(opcode::encode_jmp(0)); // placeholder
                    Some(pos)
                } else {
                    None
                };

                let catch_label_pc = ctx.bytecode.len();
                ctx.label_map.insert(catch_label, catch_label_pc);

                if let Some(try_begin_pc) = try_begin_pos {
                    let offset = catch_label_pc as isize - (try_begin_pc as isize);
                    let offset = ctx.checked_jump_offset(offset);
                    ctx.bytecode[try_begin_pc] = opcode::encode_try_begin(offset);
                }

                if let Some(catch) = &ts.handler {
                    ctx.push_scope();
                    if let Some(param) = &catch.param {
                        let catch_reg = ctx.alloc_reg();
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                            ctx.declare_initialized(bi.name.as_str(), catch_reg, VariableDeclarationKind::Let, false)?;
                            ctx.emit(opcode::encode(OpCode::STORE_VAR, catch_reg, 0, 0));
                        }
                    }
                    let mut last_catch_result: Option<u8> = None;
                    for s in &catch.body.body {
                        if let Some(r) = self.emit_statement(s, ctx)? {
                            last_catch_result = Some(r);
                        }
                    }
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, last_catch_result.unwrap_or(result_reg), 0));
                    ctx.pop_scope();
                }

                if has_finally {
                    let finally_label = Label::FinallyBody(id);
                    let finally_label_pc = ctx.bytecode.len();
                    ctx.label_map.insert(finally_label, finally_label_pc);
                    if let Some(fb_pos) = try_finally_begin_pos {
                        let offset = finally_label_pc as isize - (fb_pos as isize);
                        let offset = ctx.checked_jump_offset(offset);
                        ctx.bytecode[fb_pos] = opcode::encode_try_finally_begin(offset);
                    }

                    if let Some(jmp_pos) = jmp_skip_pos {
                        let offset = finally_label_pc as isize - (jmp_pos as isize);
                        let offset = ctx.checked_jump_offset(offset);
                        ctx.bytecode[jmp_pos] = opcode::encode_jmp(offset);
                    }

                    let mut last_finally_result: Option<u8> = None;
                    for s in &ts.finalizer.as_ref().unwrap().body {
                        if let Some(r) = self.emit_statement(s, ctx)? {
                            last_finally_result = Some(r);
                        }
                    }
                    ctx.emit(opcode::encode(
                        OpCode::LOAD_VAR,
                        result_reg,
                        last_finally_result.unwrap_or(result_reg),
                        0,
                    ));
                    ctx.emit(opcode::encode(OpCode::TRY_FINALLY_END, 0, 0, 0));
                } else if let Some(jmp_pos) = jmp_skip_pos {
                    let offset = ctx.bytecode.len() as isize - (jmp_pos as isize);
                    let offset = ctx.checked_jump_offset(offset);
                    ctx.bytecode[jmp_pos] = opcode::encode_jmp(offset);
                }

                let try_end_pc = ctx.bytecode.len();
                ctx.label_map.insert(try_end_label, try_end_pc);

                Ok(Some(result_reg))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::NumericLiteral(n) => {
                let idx = if is_int_literal(n.value) {
                    ctx.add_constant(Constant::Int(n.value as i32))
                } else {
                    ctx.add_constant(Constant::Number(n.value))
                };
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, idx);
                Ok(r)
            }
            Expression::StringLiteral(s) => {
                let idx = ctx.add_constant(Constant::String(s.value.to_string()));
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, idx);
                Ok(r)
            }
            Expression::BooleanLiteral(b) => {
                let idx = ctx.add_constant(Constant::Boolean(b.value));
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, idx);
                Ok(r)
            }
            Expression::NullLiteral(_) => {
                let idx = ctx.add_constant(Constant::Null);
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, idx);
                Ok(r)
            }
            Expression::PrivateInExpression(pin) => {
                let obj_reg = self.emit_expression(&pin.right, ctx)?;
                let key_reg = self.emit_private_id_reg(pin.left.name.as_str(), ctx)?;
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::PRIVATE_BRAND_IN, result_reg, obj_reg, key_reg));
                Ok(result_reg)
            }
            Expression::BinaryExpression(bin) => {
                let checkpoint = ctx.reg_checkpoint();
                let left = self.emit_expression(&bin.left, ctx)?;
                let right = self.emit_expression(&bin.right, ctx)?;
                let op = match bin.operator {
                    BinaryOperator::Addition => OpCode::ADD,
                    BinaryOperator::Subtraction => OpCode::SUB,
                    BinaryOperator::Multiplication => OpCode::MUL,
                    BinaryOperator::Division => OpCode::DIV,
                    BinaryOperator::Remainder => OpCode::MOD,
                    BinaryOperator::BitwiseAnd => OpCode::BIT_AND,
                    BinaryOperator::BitwiseOR => OpCode::BIT_OR,
                    BinaryOperator::BitwiseXOR => OpCode::BIT_XOR,
                    BinaryOperator::ShiftLeft => OpCode::SHL,
                    BinaryOperator::ShiftRight => OpCode::SHR,
                    BinaryOperator::ShiftRightZeroFill => OpCode::USHR,
                    BinaryOperator::Equality => OpCode::EQ,
                    BinaryOperator::Inequality => OpCode::NEQ,
                    BinaryOperator::LessThan => OpCode::LT,
                    BinaryOperator::GreaterThan => OpCode::GT,
                    BinaryOperator::LessEqualThan => OpCode::LTE,
                    BinaryOperator::GreaterEqualThan => OpCode::GTE,
                    BinaryOperator::In => OpCode::IN,
                    BinaryOperator::Instanceof => OpCode::INSTANCEOF,
                    BinaryOperator::StrictEquality => OpCode::STRICT_EQ,
                    BinaryOperator::StrictInequality => OpCode::STRICT_NEQ,
                    _ => return Err(format!("unsupported binary operator: {:?}", bin.operator)),
                };
                ctx.emit(opcode::encode(op, left, left, right));
                if is_side_effect_free(&bin.left) && is_side_effect_free(&bin.right) {
                    ctx.restore_reg_checkpoint(checkpoint.saturating_add(1).max(left.saturating_add(1)));
                }
                Ok(left)
            }
            Expression::UnaryExpression(un) => {
                if matches!(un.operator, UnaryOperator::Delete) {
                    return match &un.argument {
                        Expression::Identifier(_) => {
                            Err("SyntaxError: delete of an unqualified identifier in strict mode".into())
                        }
                        Expression::StaticMemberExpression(member) => {
                            let obj_reg = self.emit_expression(&member.object, ctx)?;
                            let prop_name = member.property.name.as_str();
                            let const_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                            ctx.emit(opcode::encode(OpCode::DELETE_PROP_STATIC, obj_reg, obj_reg, 0));
                            ctx.emit(const_idx as u32);
                            Ok(obj_reg)
                        }
                        Expression::ComputedMemberExpression(member) => {
                            let obj_reg = self.emit_expression(&member.object, ctx)?;
                            let key_reg = self.emit_expression(&member.expression, ctx)?;
                            ctx.emit(opcode::encode(OpCode::DELETE_PROP_DYNAMIC, obj_reg, obj_reg, key_reg));
                            Ok(obj_reg)
                        }
                        Expression::ChainExpression(chain) => {
                            let mut nullish_jumps = Vec::new();
                            let result_reg = match &chain.expression {
                                ChainElement::StaticMemberExpression(member) => {
                                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                                    if member.optional {
                                        let dup_reg = ctx.alloc_reg();
                                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, obj_reg, 0));
                                        let jump_pos = ctx.bytecode.len();
                                        ctx.emit(opcode::encode_jmp_if_nullish(dup_reg, 0));
                                        nullish_jumps.push(jump_pos);
                                    }
                                    let prop_name = member.property.name.as_str();
                                    let const_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                                    ctx.emit(opcode::encode(OpCode::DELETE_PROP_STATIC, obj_reg, obj_reg, 0));
                                    ctx.emit(const_idx as u32);
                                    obj_reg
                                }
                                ChainElement::ComputedMemberExpression(member) => {
                                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                                    if member.optional {
                                        let dup_reg = ctx.alloc_reg();
                                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, obj_reg, 0));
                                        let jump_pos = ctx.bytecode.len();
                                        ctx.emit(opcode::encode_jmp_if_nullish(dup_reg, 0));
                                        nullish_jumps.push(jump_pos);
                                    }
                                    let key_reg = self.emit_expression(&member.expression, ctx)?;
                                    ctx.emit(opcode::encode(OpCode::DELETE_PROP_DYNAMIC, obj_reg, obj_reg, key_reg));
                                    obj_reg
                                }
                                _ => return Err("invalid delete target".into()),
                            };
                            let end_jump_pos = ctx.bytecode.len();
                            ctx.emit(opcode::encode_jmp(0));
                            let short_pos = ctx.bytecode.len();
                            for jump_pos in nullish_jumps {
                                let offset = (short_pos as isize) - (jump_pos as isize);
                                let instr = ctx.bytecode[jump_pos];
                                let rd = opcode::rd(instr);
                                let offset = ctx.checked_jump_offset(offset);
                                ctx.bytecode[jump_pos] = opcode::encode_jmp_if_nullish(rd, offset);
                            }
                            let true_idx = ctx.add_constant(Constant::Boolean(true));
                            ctx.emit_load_const(result_reg, true_idx);
                            let end_pos = ctx.bytecode.len();
                            let offset = (end_pos as isize) - (end_jump_pos as isize);
                            let offset = ctx.checked_jump_offset(offset);
                            ctx.bytecode[end_jump_pos] = opcode::encode_jmp(offset);
                            Ok(result_reg)
                        }
                        _ => Err("invalid delete target".into()),
                    };
                }
                let arg = self.emit_expression(&un.argument, ctx)?;
                match un.operator {
                    UnaryOperator::UnaryNegation => {
                        ctx.emit(opcode::encode(OpCode::NEG, arg, arg, 0));
                        Ok(arg)
                    }
                    UnaryOperator::Typeof => {
                        ctx.emit(opcode::encode(OpCode::TYPEOF, arg, arg, 0));
                        Ok(arg)
                    }
                    UnaryOperator::Void => {
                        ctx.emit(opcode::encode(OpCode::VOID, arg, arg, 0));
                        Ok(arg)
                    }
                    UnaryOperator::LogicalNot => {
                        ctx.emit(opcode::encode(OpCode::NOT, arg, arg, 0));
                        Ok(arg)
                    }
                    UnaryOperator::BitwiseNot => {
                        ctx.emit(opcode::encode(OpCode::BIT_NOT, arg, arg, 0));
                        Ok(arg)
                    }
                    UnaryOperator::UnaryPlus => {
                        ctx.emit(opcode::encode(OpCode::UNARY_PLUS, arg, arg, 0));
                        Ok(arg)
                    }
                    UnaryOperator::Delete => unreachable!("delete handled before argument emit"),
                }
            }
            Expression::ConditionalExpression(cond) => {
                let id = ctx.next_label_id();
                let else_label = Label::TernaryElse(id);
                let end_label = Label::TernaryEnd(id);

                let test_reg = self.emit_expression(&cond.test, ctx)?;
                let else_pos = ctx.resolve_label(else_label)?;
                let end_pos = ctx.resolve_label(end_label)?;

                let offset = (else_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset));

                let cons_reg = self.emit_expression(&cond.consequent, ctx)?;
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, cons_reg, 0));

                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));

                let alt_reg = self.emit_expression(&cond.alternate, ctx)?;
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, alt_reg, 0));

                Ok(result_reg)
            }
            Expression::LogicalExpression(log) => {
                use oxide_parser::LogicalOperator;
                let left_reg = self.emit_expression(&log.left, ctx)?;

                if is_side_effect_free(&log.left) && is_side_effect_free(&log.right) {
                    let right_reg = self.emit_expression(&log.right, ctx)?;
                    let r = ctx.alloc_reg();
                    let op = match log.operator {
                        LogicalOperator::And => OpCode::AND,
                        LogicalOperator::Or => OpCode::OR,
                        LogicalOperator::Coalesce => OpCode::NULLISH,
                    };
                    ctx.emit(opcode::encode(op, r, left_reg, right_reg));
                    Ok(r)
                } else {
                    let id = ctx.next_label_id();
                    if matches!(log.operator, LogicalOperator::Coalesce) {
                        let dup_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, left_reg, 0));
                        let nullish_jump_pos = ctx.bytecode.len();
                        ctx.emit(opcode::encode_jmp_if_nullish(dup_reg, 0));
                        let end_jump_pos = ctx.bytecode.len();
                        ctx.emit(opcode::encode_jmp(0));
                        let rhs_pos = ctx.bytecode.len();
                        let right_reg = self.emit_expression(&log.right, ctx)?;
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, right_reg, 0));
                        let end_pos = ctx.bytecode.len();
                        let offset = (rhs_pos as isize) - (nullish_jump_pos as isize);
                        let offset = ctx.checked_jump_offset(offset);
                        ctx.bytecode[nullish_jump_pos] = opcode::encode_jmp_if_nullish(dup_reg, offset);
                        let offset = (end_pos as isize) - (end_jump_pos as isize);
                        let offset = ctx.checked_jump_offset(offset);
                        ctx.bytecode[end_jump_pos] = opcode::encode_jmp(offset);
                        return Ok(dup_reg);
                    }
                    let skip_label = match log.operator {
                        LogicalOperator::And => Label::TernaryEnd(id),
                        LogicalOperator::Or => Label::TernaryElse(id),
                        LogicalOperator::Coalesce => unreachable!(),
                    };
                    let skip_pos = ctx.resolve_label(skip_label)?;

                    let dup_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, left_reg, 0));

                    let offset = (skip_pos as isize) - (ctx.bytecode.len() as isize);
                    match log.operator {
                        LogicalOperator::And => {
                            let offset = ctx.checked_jump_offset(offset);
                            ctx.emit(opcode::encode_jmp_if_false(dup_reg, offset));
                        }
                        LogicalOperator::Or => {
                            let offset = ctx.checked_jump_offset(offset);
                            ctx.emit(opcode::encode_jmp_if_true(dup_reg, offset));
                        }
                        LogicalOperator::Coalesce => unreachable!(),
                    }

                    let right_reg = self.emit_expression(&log.right, ctx)?;
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, right_reg, 0));

                    Ok(dup_reg)
                }
            }
            Expression::StaticMemberExpression(member) => {
                if matches!(&member.object, Expression::Super(_)) {
                    if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                        return Err("super property only supported in class methods".into());
                    }
                    let prop_name = member.property.name.as_str();
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit_load_const(key_reg, idx);
                    let this_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, this_reg, 254, 0));
                    let result_reg = ctx.alloc_reg();
                    let op = if ctx.in_static_method {
                        OpCode::SUPER_STATIC_GET_PROP
                    } else {
                        OpCode::SUPER_GET_PROP
                    };
                    ctx.emit(opcode::encode(op, result_reg, this_reg, key_reg));
                    return Ok(result_reg);
                }
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_CONST, key_reg, (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8));
                ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, obj_reg, key_reg));
                ctx.emit(0);
                ctx.emit(0);
                ctx.emit(0);
                Ok(obj_reg)
            }
            Expression::ComputedMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_expression(&member.expression, ctx)?;
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::GET_PROP_DYNAMIC, obj_reg, key_reg, r));
                Ok(r)
            }
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::GET_PRIVATE, r, obj_reg, key_reg));
                Ok(r)
            }
            Expression::ChainExpression(chain) => {
                let id = ctx.next_label_id();
                let short_label = Label::TernaryElse(id);
                let end_label = Label::TernaryEnd(id);
                let value_reg = self.emit_chain_element(&chain.expression, Some(short_label), ctx)?;
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, value_reg, 0));
                let end_pos = ctx.resolve_label(end_label)?;
                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                let offset = ctx.checked_jump_offset(offset);
                ctx.emit(opcode::encode_jmp(offset));
                let undefined_idx = ctx.add_constant(Constant::Undefined);
                ctx.emit_load_const(result_reg, undefined_idx);
                Ok(result_reg)
            }
            Expression::ObjectExpression(obj) => {
                let obj_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_OBJECT, obj_reg, 0, 0));
                let prop_checkpoint = ctx.reg_checkpoint();
                for prop in &obj.properties {
                    let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop else {
                        return Err("spread properties not yet supported".into());
                    };
                    let prop_name = match &p.key {
                        oxide_parser::PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                        oxide_parser::PropertyKey::StringLiteral(s) => s.value.to_string(),
                        _ => return Err("unsupported object property key type".into()),
                    };
                    if matches!(p.kind, PropertyKind::Get | PropertyKind::Set) {
                        let accessor_reg = self.emit_expression(&p.value, ctx)?;
                        if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                            sub_mod.function_name = Some(prop_name.to_string());
                        }
                        let undef_reg = self.emit_undefined(ctx);
                        let (get_reg, set_reg) = match p.kind {
                            PropertyKind::Get => (accessor_reg, undef_reg),
                            PropertyKind::Set => (undef_reg, accessor_reg),
                            _ => unreachable!(),
                        };
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, obj_reg, get_reg, set_reg));
                        ctx.emit(idx as u32);
                        ctx.restore_reg_checkpoint(prop_checkpoint);
                    } else {
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        let key_reg = ctx.alloc_reg();
                        ctx.emit_load_const(key_reg, idx);
                        let val_reg = self.emit_expression(&p.value, ctx)?;
                        // Name inference (D-04): if property value is an arrow function,
                        // set the compiled sub_module's function_name to the property key.
                        if matches!(&p.value, Expression::ArrowFunctionExpression(_)) {
                            if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                                sub_mod.function_name = Some(prop_name.to_string());
                            }
                        }
                        ctx.emit(opcode::encode(OpCode::SET_PROP, obj_reg, val_reg, key_reg));
                        ctx.restore_reg_checkpoint(prop_checkpoint);
                    }
                }
                Ok(obj_reg)
            }
            Expression::ArrayExpression(arr) => {
                let arr_reg = ctx.alloc_reg();
                let n = arr.elements.len() as u16;
                ctx.emit(opcode::encode(OpCode::NEW_ARRAY, arr_reg, (n & 0xFF) as u8, ((n >> 8) & 0xFF) as u8));
                let elem_checkpoint = ctx.reg_checkpoint();
                for (i, elem) in arr.elements.iter().enumerate() {
                    let Some(e) = elem.as_expression() else {
                        return Err("spread not supported".into());
                    };
                    let val_reg = self.emit_expression(e, ctx)?;
                    let idx_reg = ctx.alloc_reg();
                    let idx = ctx.add_constant(Constant::Int(i as i32));
                    ctx.emit_load_const(idx_reg, idx);
                    ctx.emit(opcode::encode(OpCode::SET_ELEM, arr_reg, idx_reg, val_reg));
                    ctx.restore_reg_checkpoint(elem_checkpoint);
                }
                Ok(arr_reg)
            }
            Expression::AssignmentExpression(assign) => {
                if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left {
                    if let Some(logical_op) = assign.operator.to_logical_operator() {
                        let id = ctx.next_label_id();
                        let store_label = Label::TernaryElse(id);
                        let end_label = Label::TernaryEnd(id);
                        let obj_reg = self.emit_expression(&member.object, ctx)?;
                        let prop_name = member.property.name.as_str();
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        let key_reg = ctx.alloc_reg();
                        ctx.emit_load_const(key_reg, idx);
                        let result_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, obj_reg, 0));
                        ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, result_reg, key_reg));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                        let val_reg = self.emit_expression(&assign.right, ctx)?;
                        ctx.emit(opcode::encode(OpCode::IC_SET_PROP, obj_reg, val_reg, key_reg));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, val_reg, 0));
                        return Ok(result_reg);
                    }
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    let prop_name = member.property.name.as_str();
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit_load_const(key_reg, idx);
                    if assign.operator != AssignmentOperator::Assign {
                        let op = match assign.operator {
                            AssignmentOperator::Addition => OpCode::COMPOUND_MEMBER_ADD,
                            AssignmentOperator::Subtraction => OpCode::COMPOUND_MEMBER_SUB,
                            AssignmentOperator::Multiplication => OpCode::COMPOUND_MEMBER_MUL,
                            AssignmentOperator::Division => OpCode::COMPOUND_MEMBER_DIV,
                            AssignmentOperator::Remainder => OpCode::COMPOUND_MEMBER_MOD,
                            AssignmentOperator::Exponential => OpCode::COMPOUND_MEMBER_EXP,
                            _ => {
                                return Err(format!("compound assignment operator {:?} not supported", assign.operator))
                            }
                        };
                        ctx.emit(opcode::encode(op, obj_reg, val_reg, key_reg));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        Ok(val_reg)
                    } else {
                        ctx.emit(opcode::encode(OpCode::IC_SET_PROP, obj_reg, val_reg, key_reg));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        Ok(val_reg)
                    }
                } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) = &assign.left {
                    if let Some(logical_op) = assign.operator.to_logical_operator() {
                        let id = ctx.next_label_id();
                        let store_label = Label::TernaryElse(id);
                        let end_label = Label::TernaryEnd(id);
                        let obj_reg = self.emit_expression(&member.object, ctx)?;
                        let key_reg = self.emit_expression(&member.expression, ctx)?;
                        let result_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::GET_PROP_DYNAMIC, obj_reg, key_reg, result_reg));
                        self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                        let val_reg = self.emit_expression(&assign.right, ctx)?;
                        ctx.emit(opcode::encode(OpCode::SET_PROP_DYNAMIC, obj_reg, key_reg, val_reg));
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, val_reg, 0));
                        return Ok(result_reg);
                    }
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let key_reg = self.emit_expression(&member.expression, ctx)?;
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    ctx.emit(opcode::encode(OpCode::SET_PROP_DYNAMIC, obj_reg, key_reg, val_reg));
                    Ok(val_reg)
                } else if let oxide_parser::AssignmentTarget::PrivateFieldExpression(member) = &assign.left {
                    if assign.operator != AssignmentOperator::Assign {
                        return Err("compound assignment to private fields not supported".into());
                    }
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                    ctx.emit(opcode::encode(OpCode::SET_PRIVATE, obj_reg, val_reg, key_reg));
                    Ok(val_reg)
                } else if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(id_ref) = &assign.left {
                    if assign.operator != AssignmentOperator::Assign {
                        if let Some(logical_op) = assign.operator.to_logical_operator() {
                            let id = ctx.next_label_id();
                            let store_label = Label::TernaryElse(id);
                            let end_label = Label::TernaryEnd(id);
                            let name = id_ref.name.as_str();
                            let var_reg = ctx.lookup_or_global(name);
                            let result_reg = ctx.alloc_reg();
                            ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, var_reg, 0));
                            self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                            let val_reg = self.emit_expression(&assign.right, ctx)?;
                            let is_const = ctx.lookup_const_flag(name);
                            let const_flag = if is_const { 1 } else { 0 };
                            ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, const_flag));
                            ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, val_reg, 0));
                            Ok(result_reg)
                        } else if assign.operator == AssignmentOperator::Addition
                            || assign.operator == AssignmentOperator::Subtraction
                            || assign.operator == AssignmentOperator::Multiplication
                            || assign.operator == AssignmentOperator::Division
                            || assign.operator == AssignmentOperator::Remainder
                            || assign.operator == AssignmentOperator::Exponential
                            || assign.operator == AssignmentOperator::BitwiseAnd
                            || assign.operator == AssignmentOperator::BitwiseOR
                            || assign.operator == AssignmentOperator::BitwiseXOR
                            || assign.operator == AssignmentOperator::ShiftLeft
                            || assign.operator == AssignmentOperator::ShiftRight
                            || assign.operator == AssignmentOperator::ShiftRightZeroFill
                        {
                            let rhs = self.emit_expression(&assign.right, ctx)?;
                            let name = id_ref.name.as_str();
                            let var_reg = ctx.lookup_or_global(name);
                            let op = match assign.operator {
                                AssignmentOperator::Addition => OpCode::COMPOUND_ADD,
                                AssignmentOperator::Subtraction => OpCode::COMPOUND_SUB,
                                AssignmentOperator::Multiplication => OpCode::COMPOUND_MUL,
                                AssignmentOperator::Division => OpCode::COMPOUND_DIV,
                                AssignmentOperator::Remainder => OpCode::COMPOUND_MOD,
                                AssignmentOperator::Exponential => OpCode::COMPOUND_EXP,
                                AssignmentOperator::BitwiseAnd => OpCode::COMPOUND_AND,
                                AssignmentOperator::BitwiseOR => OpCode::COMPOUND_OR,
                                AssignmentOperator::BitwiseXOR => OpCode::COMPOUND_XOR,
                                AssignmentOperator::ShiftLeft => OpCode::COMPOUND_SHL,
                                AssignmentOperator::ShiftRight => OpCode::COMPOUND_SHR,
                                AssignmentOperator::ShiftRightZeroFill => OpCode::COMPOUND_USHR,
                                _ => {
                                    return Err(format!(
                                        "compound assignment operator {:?} not supported",
                                        assign.operator
                                    ))
                                }
                            };
                            ctx.emit(opcode::encode(op, var_reg, rhs, 0));
                            Ok(var_reg)
                        } else {
                            Err(format!("compound assignment operator {:?} not supported", assign.operator))
                        }
                    } else {
                        let val_reg = self.emit_expression(&assign.right, ctx)?;
                        let name = id_ref.name.as_str();
                        let var_reg = ctx.lookup_or_global(name);
                        let is_const = ctx.lookup_const_flag(name);
                        let const_flag = if is_const { 1 } else { 0 };
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, const_flag));
                        Ok(val_reg)
                    }
                } else if matches!(
                    &assign.left,
                    oxide_parser::AssignmentTarget::ArrayAssignmentTarget(_)
                        | oxide_parser::AssignmentTarget::ObjectAssignmentTarget(_)
                ) {
                    if assign.operator != AssignmentOperator::Assign {
                        return Err("compound destructuring assignment not supported".into());
                    }
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    self.emit_assign_target(&assign.left, val_reg, ctx)?;
                    Ok(val_reg)
                } else {
                    Err("assignment target not supported".into())
                }
            }
            Expression::UpdateExpression(update) => match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                    let name = id.name.as_str();
                    let var_reg = ctx.lookup_or_global(name);
                    let result_reg = ctx.alloc_reg();
                    let op = match (update.operator, update.prefix) {
                        (UpdateOperator::Increment, true) => OpCode::INC_PRE,
                        (UpdateOperator::Increment, false) => OpCode::INC_POST,
                        (UpdateOperator::Decrement, true) => OpCode::DEC_PRE,
                        (UpdateOperator::Decrement, false) => OpCode::DEC_POST,
                    };
                    ctx.emit(opcode::encode(op, var_reg, result_reg, result_reg));
                    Ok(result_reg)
                }
                SimpleAssignmentTarget::StaticMemberExpression(member) => {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let prop_name = member.property.name.as_str();
                    let key_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(
                        OpCode::LOAD_CONST,
                        key_reg,
                        (key_idx & 0xFF) as u8,
                        ((key_idx >> 8) & 0xFF) as u8,
                    ));
                    let val_reg = ctx.alloc_reg();
                    let op = match update.operator {
                        UpdateOperator::Increment => OpCode::MEMBER_INC,
                        UpdateOperator::Decrement => OpCode::MEMBER_DEC,
                    };
                    ctx.emit(opcode::encode(op, obj_reg, val_reg, key_reg));
                    ctx.emit(0);
                    ctx.emit(0);
                    ctx.emit(0);
                    Ok(val_reg)
                }
                SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let key_reg = self.emit_expression(&member.expression, ctx)?;
                    let val_reg = ctx.alloc_reg();
                    let op = match update.operator {
                        UpdateOperator::Increment => OpCode::DYN_MEMBER_INC,
                        UpdateOperator::Decrement => OpCode::DYN_MEMBER_DEC,
                    };
                    ctx.emit(opcode::encode(op, obj_reg, key_reg, val_reg));
                    Ok(val_reg)
                }
                _ => Err("member update not yet supported".into()),
            },
            Expression::Identifier(ident) => {
                let var_reg = ctx.lookup_or_builtin(ident.name.as_str())?;
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, var_reg, 0));
                Ok(r)
            }
            // TemplateLiteral: interleaved quasis + expressions via TEMPLATE_STR opcode (D-07)
            Expression::TemplateLiteral(tl) => {
                let r = ctx.alloc_reg();
                let quasis = &tl.quasis;
                let expressions = &tl.expressions;
                let segment_count = quasis.len() + expressions.len();

                // Evaluate each expression, collecting registers
                let expr_regs: Vec<u8> = expressions
                    .iter()
                    .map(|e| self.emit_expression(e, ctx))
                    .collect::<Result<Vec<_>, _>>()?;

                // Add each quasi string to constant pool
                let quasi_const_idxs: Vec<u16> = quasis
                    .iter()
                    .map(|q| {
                        let s = q.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
                        ctx.add_constant(Constant::String(s))
                    })
                    .collect();

                // Compute total length hint
                let total_len_hint: usize = quasis
                    .iter()
                    .map(|q| q.value.cooked.as_ref().map(|c| c.len()).unwrap_or(0))
                    .sum();

                // Emit TEMPLATE_STR rd, 0, 0
                ctx.emit(opcode::encode(OpCode::TEMPLATE_STR, r, 0, 0));

                // Ext word 0: (segment_count << 16) | (total_len_hint & 0xFFFF)
                ctx.emit(((segment_count as u32) << 16) | (total_len_hint as u32 & 0xFFFF));

                // Interleave segments: quasi[0], expr[0], quasi[1], expr[1], ...
                let mut expr_iter = expr_regs.iter();
                for const_idx in quasi_const_idxs.iter() {
                    // Quasi: is_expression=0, bits 0-15 = const_idx
                    ctx.emit(*const_idx as u32 & 0x7FFF_FFFF);

                    // Expression (if any remaining)
                    if let Some(expr_reg) = expr_iter.next() {
                        // Expression: is_expression=1, bits 0-15 = reg
                        ctx.emit(0x8000_0000u32 | (*expr_reg as u32));
                    }
                }

                Ok(r)
            }
            // TaggedTemplateExpression: tag`str ${expr}` => CALL(tag, undefined, cooked_array, raw_array, ...exprs) (D-08)
            Expression::TaggedTemplateExpression(tt) => {
                let quasis = &tt.quasi.quasis;
                let expressions = &tt.quasi.expressions;

                // 1. Evaluate tag expression
                let tag_reg = self.emit_expression(&tt.tag, ctx)?;

                // 2. Build cooked strings array (into a temp register)
                let cooked_temp = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_ARRAY, cooked_temp, quasis.len() as u8, 0));
                for (i, quasi) in quasis.iter().enumerate() {
                    let s = quasi.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_default();
                    let const_idx = ctx.add_constant(Constant::String(s));
                    let str_reg = ctx.alloc_reg();
                    ctx.emit_load_const(str_reg, const_idx);
                    let idx_const = ctx.add_constant(Constant::Int(i as i32));
                    let idx_reg = ctx.alloc_reg();
                    ctx.emit_load_const(idx_reg, idx_const);
                    ctx.emit(opcode::encode(OpCode::SET_ELEM, cooked_temp, idx_reg, str_reg));
                }

                // 3. Build raw strings array (into a temp register)
                let raw_temp = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_ARRAY, raw_temp, quasis.len() as u8, 0));
                for (i, quasi) in quasis.iter().enumerate() {
                    let raw = quasi.value.raw.to_string();
                    let const_idx = ctx.add_constant(Constant::String(raw));
                    let str_reg = ctx.alloc_reg();
                    ctx.emit_load_const(str_reg, const_idx);
                    let idx_const = ctx.add_constant(Constant::Int(i as i32));
                    let idx_reg = ctx.alloc_reg();
                    ctx.emit_load_const(idx_reg, idx_const);
                    ctx.emit(opcode::encode(OpCode::SET_ELEM, raw_temp, idx_reg, str_reg));
                }

                // 4. Evaluate expression arguments (into temp registers)
                let mut expr_temps = Vec::new();
                for expr in expressions {
                    expr_temps.push(self.emit_expression(expr, ctx)?);
                }

                // 5. Allocate consecutive argument slots: cooked, raw, expr[0], expr[1], ...
                let cooked_slot = ctx.alloc_reg();
                let raw_slot = ctx.alloc_reg();
                let mut expr_slots = Vec::new();
                for _ in expressions {
                    expr_slots.push(ctx.alloc_reg());
                }

                // Copy temps to consecutive slots using LOAD_VAR (register-to-register move)
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, cooked_slot, cooked_temp, 0));
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, raw_slot, raw_temp, 0));
                for (slot, temp) in expr_slots.iter().zip(expr_temps.iter()) {
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, *slot, *temp, 0));
                }

                // 6. Emit undefined as this_arg
                let undef_idx = ctx.add_constant(Constant::Undefined);
                let undef_reg = ctx.alloc_reg();
                ctx.emit_load_const(undef_reg, undef_idx);

                // 7. Emit CALL(tag, undefined, cooked_slot)
                let arg_count = 2 + expressions.len();
                ctx.emit(opcode::encode(OpCode::CALL, tag_reg, undef_reg, cooked_slot));
                ctx.emit(arg_count as u32);

                // 8. Result from regs[0]
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, 0, 0));
                Ok(result_reg)
            }
            // ArrowFunctionExpression: compile body, capture lexical this (D-01)
            // Name inference (D-04) happens at assignment site - see VariableDeclaration/ObjectProperty.
            Expression::ArrowFunctionExpression(arrow) => {
                // Rest params not yet supported (D-06 placeholder)
                if let Some(_rest) = &arrow.params.rest {
                    return Err("rest params in arrow functions not yet supported".into());
                }

                // Extract param names (same pattern as FunctionExpression)
                let mut param_names = Vec::new();
                for (idx, param) in arrow.params.items.iter().enumerate() {
                    match &param.pattern {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                            param_names.push(ParamSpec::Identifier(bi.name.to_string()));
                        }
                        pattern => {
                            param_names.push(ParamSpec::Pattern {
                                synthetic_name: format!("@@param_{idx}"),
                                pattern,
                            });
                        }
                    }
                }

                // Expression body: pass body statements directly with is_expression_body=true.
                // Statement body: pass body statements with is_expression_body=false.
                let body_stmts = &arrow.body.statements;
                let is_expr_body = arrow.expression;

                let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, is_expr_body, true)?;
                sub_module.is_arrow = true;

                ctx.sub_modules.push(sub_module);
                // 1-indexed: 0 = no sub_module (sentinel)
                let sub_idx = ctx.sub_modules.len() as u32;

                let const_idx = ctx.add_constant(Constant::BytecodeFunc(sub_idx));
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, const_idx);
                Ok(r)
            }
            Expression::FunctionExpression(fe) => {
                // FunctionExpression: compile body, emit LOAD_CONST(BytecodeFunc)
                let mut param_names = Vec::new();
                for (idx, param) in fe.params.items.iter().enumerate() {
                    match &param.pattern {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                            param_names.push(ParamSpec::Identifier(bi.name.to_string()));
                        }
                        pattern => {
                            param_names.push(ParamSpec::Pattern {
                                synthetic_name: format!("@@param_{idx}"),
                                pattern,
                            });
                        }
                    }
                }

                let body_stmts: &[Statement] = if let Some(body) = &fe.body { &body.statements } else { &[] };

                let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, false, false)?;
                if let Some(id) = &fe.id {
                    sub_module.function_name = Some(id.name.to_string());
                }
                ctx.sub_modules.push(sub_module);
                // 1-indexed: 0 = no sub_module (sentinel)
                let sub_idx = ctx.sub_modules.len() as u32;

                let const_idx = ctx.add_constant(Constant::BytecodeFunc(sub_idx));
                let r = ctx.alloc_reg();
                ctx.emit_load_const(r, const_idx);
                Ok(r)
            }
            Expression::ClassExpression(class) => self.emit_class(class, ctx),
            Expression::NewExpression(ne) => {
                let constructor_reg = self.emit_expression(&ne.callee, ctx)?;
                let mut arg_regs = Vec::new();
                for arg in &ne.arguments {
                    if let Some(expr) = arg.as_expression() {
                        arg_regs.push(self.emit_expression(expr, ctx)?);
                    }
                }
                let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_EXPRESSION, r, constructor_reg, first_arg_reg));
                ctx.emit(arg_regs.len() as u32);
                Ok(r)
            }
            Expression::ParenthesizedExpression(p) => self.emit_expression(&p.expression, ctx),
            Expression::CallExpression(call) => {
                if matches!(&call.callee, Expression::Super(_)) {
                    if !ctx.in_derived_constructor {
                        return Err("super() only supported in derived constructors".into());
                    }
                    let mut arg_regs = Vec::new();
                    for arg in &call.arguments {
                        if let Some(expr) = arg.as_expression() {
                            arg_regs.push(self.emit_expression(expr, ctx)?);
                        }
                    }
                    let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
                    let result_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::SUPER_CALL, result_reg, first_arg_reg, 0));
                    ctx.emit(arg_regs.len() as u32);
                    if !ctx.after_super_inserted {
                        if let Some(field_code) = ctx.after_super_insert.clone() {
                            ctx.bytecode.extend(field_code);
                            ctx.after_super_inserted = true;
                        }
                    }
                    return Ok(result_reg);
                }
                let (callee_reg, this_reg) = match &call.callee {
                    Expression::PrivateFieldExpression(member) => {
                        let obj_reg = self.emit_expression(&member.object, ctx)?;
                        let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                        let callee_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::GET_PRIVATE, callee_reg, obj_reg, key_reg));
                        (callee_reg, obj_reg)
                    }
                    Expression::StaticMemberExpression(member) => {
                        let is_super_member = matches!(&member.object, Expression::Super(_));
                        let obj_reg = if is_super_member {
                            let this_reg = ctx.alloc_reg();
                            ctx.emit(opcode::encode(OpCode::LOAD_VAR, this_reg, 254, 0));
                            this_reg
                        } else {
                            self.emit_expression(&member.object, ctx)?
                        };
                        let prop_name = member.property.name.as_str();
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        let key_reg = ctx.alloc_reg();
                        ctx.emit_load_const(key_reg, idx);
                        let callee_reg = ctx.alloc_reg();
                        if is_super_member {
                            if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                                return Err("super property only supported in class methods".into());
                            }
                            let op = if ctx.in_static_method {
                                OpCode::SUPER_STATIC_GET_PROP
                            } else {
                                OpCode::SUPER_GET_PROP
                            };
                            ctx.emit(opcode::encode(op, callee_reg, obj_reg, key_reg));
                        } else {
                            ctx.emit(opcode::encode(OpCode::LOAD_VAR, callee_reg, obj_reg, 0));
                            ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, callee_reg, key_reg));
                            ctx.emit(0);
                            ctx.emit(0);
                            ctx.emit(0);
                        }
                        (callee_reg, obj_reg)
                    }
                    _ => {
                        let callee_reg = self.emit_expression(&call.callee, ctx)?;
                        let this_idx = ctx.add_constant(Constant::Undefined);
                        let this_reg = ctx.alloc_reg();
                        ctx.emit_load_const(this_reg, this_idx);
                        (callee_reg, this_reg)
                    }
                };
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
                // Copy result from regs[0] into a dedicated register so multiple
                // call expressions don't overwrite each other.
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, 0, 0));
                Ok(result_reg)
            }
            Expression::ThisExpression(_) => {
                let r = ctx.alloc_reg();
                let src = ctx.static_block_this_reg.unwrap_or(254);
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, src, 0));
                Ok(r)
            }
            Expression::RegExpLiteral(lit) => {
                if let Some(raw) = &lit.raw {
                    let raw_str = raw.to_string();
                    if raw_str.len() >= 2 && raw_str.starts_with('/') {
                        let last_slash = raw_str.rfind('/').unwrap_or(raw_str.len() - 1);
                        let pattern = &raw_str[1..last_slash];
                        let flags = &raw_str[last_slash + 1..];
                        let ci = ctx.add_constant(Constant::RegExp(pattern.to_string(), flags.to_string()));
                        let r = ctx.alloc_reg();
                        ctx.emit_load_const(r, ci);
                        Ok(r)
                    } else {
                        Err(format!("unsupported expression type: {:?}", expr))
                    }
                } else {
                    Err(format!("unsupported expression type: {:?}", expr))
                }
            }
            _ => Err(format!("unsupported expression type: {:?}", expr)),
        }
    }
}
