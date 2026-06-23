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

mod expr;
mod stmt;

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
                            let (get_reg, set_reg) = if method.kind == MethodDefinitionKind::Get {
                                (accessor_reg, undef_reg)
                            } else {
                                (undef_reg, accessor_reg)
                            };
                            let key_name = self.class_property_name(&method.key)?;
                            let key_idx = ctx.add_constant(Constant::String(key_name));
                            ctx.emit(opcode::encode(OpCode::DEFINE_ACCESSOR, home_reg, get_reg, set_reg));
                            ctx.emit(key_idx as u32);
                        }
                        MethodDefinitionKind::Constructor => continue,
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
            Statement::ExpressionStatement(_) => self.emit_expression_statement(stmt, ctx),
            Statement::VariableDeclaration(_) => self.emit_variable_declaration_statement(stmt, ctx),
            Statement::ReturnStatement(_) => self.emit_return_statement(stmt, ctx),
            Statement::BlockStatement(_) => self.emit_block_statement(stmt, ctx),
            Statement::IfStatement(_) => self.emit_if_statement(stmt, ctx),
            Statement::WhileStatement(_) => self.emit_while_statement(stmt, ctx),
            Statement::DoWhileStatement(_) => self.emit_do_while_statement(stmt, ctx),
            Statement::ForStatement(_) => self.emit_for_statement(stmt, ctx),
            Statement::ForInStatement(_) => self.emit_for_in_statement(stmt, ctx),
            Statement::ForOfStatement(_) => self.emit_for_of_statement(stmt, ctx),
            Statement::SwitchStatement(_) => self.emit_switch_statement(stmt, ctx),
            Statement::BreakStatement(_) => self.emit_break_statement(ctx),
            Statement::ContinueStatement(_) => self.emit_continue_statement(ctx),
            Statement::FunctionDeclaration(_) => self.emit_function_declaration_statement(stmt, ctx),
            Statement::ClassDeclaration(_) => self.emit_class_declaration_statement(stmt, ctx),
            Statement::ThrowStatement(_) => self.emit_throw_statement(stmt, ctx),
            Statement::TryStatement(_) => self.emit_try_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
    pub(crate) fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::NumericLiteral(n) => self.emit_numeric_literal_expression(n, ctx),
            Expression::StringLiteral(s) => self.emit_string_literal_expression(s, ctx),
            Expression::BooleanLiteral(b) => self.emit_boolean_literal_expression(b, ctx),
            Expression::NullLiteral(_) => self.emit_null_literal_expression(ctx),
            Expression::PrivateInExpression(pin) => self.emit_private_in_expression(pin, ctx),
            Expression::BinaryExpression(bin) => self.emit_binary_expression(bin, ctx),
            Expression::UnaryExpression(un) => self.emit_unary_expression(un, ctx),
            Expression::ConditionalExpression(cond) => self.emit_conditional_expression(cond, ctx),
            Expression::LogicalExpression(log) => self.emit_logical_expression(log, ctx),
            Expression::StaticMemberExpression(member) => self.emit_static_member_expression(member, ctx),
            Expression::ComputedMemberExpression(member) => self.emit_computed_member_expression(member, ctx),
            Expression::PrivateFieldExpression(member) => self.emit_private_field_expression(member, ctx),
            Expression::ChainExpression(chain) => self.emit_chain_expression(chain, ctx),
            Expression::ObjectExpression(obj) => self.emit_object_expression(obj, ctx),
            Expression::ArrayExpression(arr) => self.emit_array_expression(arr, ctx),
            Expression::AssignmentExpression(assign) => self.emit_assignment_expression(assign, ctx),
            Expression::UpdateExpression(update) => self.emit_update_expression(update, ctx),
            Expression::Identifier(ident) => self.emit_identifier_expression(ident, ctx),
            Expression::TemplateLiteral(tl) => self.emit_template_literal_expression(tl, ctx),
            Expression::TaggedTemplateExpression(tt) => self.emit_tagged_template_expression(tt, ctx),
            Expression::ArrowFunctionExpression(arrow) => self.emit_arrow_function_expression(arrow, ctx),
            Expression::FunctionExpression(fe) => self.emit_function_expression(fe, ctx),
            Expression::ClassExpression(class) => self.emit_class_expression(class, ctx),
            Expression::NewExpression(ne) => self.emit_new_expression(ne, ctx),
            Expression::ParenthesizedExpression(p) => self.emit_parenthesized_expression(p, ctx),
            Expression::CallExpression(call) => self.emit_call_expression(call, ctx),
            Expression::ThisExpression(_) => self.emit_this_expression(ctx),
            Expression::RegExpLiteral(lit) => self.emit_reg_exp_literal_expression(lit, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
