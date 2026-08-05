use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{
    ArrayAssignmentTarget, ArrayPattern, AssignmentTarget, AssignmentTargetMaybeDefault, AssignmentTargetProperty,
    BindingPattern, Expression, ObjectAssignmentTarget, ObjectPattern, VariableDeclarationKind,
};

impl Compiler {
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
        let target_reg = if matches!(kind, VariableDeclarationKind::Var) {
            // `var` names are pre-declared (hoisting); reuse the pre-registered slot
            // instead of allocating a new one so n_registers matches let/const.
            if let Some(reg) = ctx.scopes.symbols.lookup_any(name) {
                reg
            } else {
                let var_reg = ctx.alloc_reg();
                ctx.declare(name, var_reg, kind, is_const)?;
                var_reg
            }
        } else {
            let var_reg = ctx.alloc_reg();
            ctx.declare(name, var_reg, kind, is_const)?;
            var_reg
        };
        if ctx.scopes.symbols.lookup_is_captured(name) {
            let cell_idx = ctx.scopes.cell_registry.len() as u8;
            ctx.scopes.cell_registry.push((name.to_string(), cell_idx));
            ctx.emit(opcode::encode(OpCode::MAKE_CELL, src_reg, cell_idx, 0));
        } else {
            ctx.emit(opcode::encode(OpCode::STORE_VAR, target_reg, src_reg, if is_const { 1 } else { 0 }));
        }
        ctx.init_var(name);
        Ok(())
    }

    pub(crate) fn emit_assign_target(
        &self, target: &AssignmentTarget, src_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
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
        &self, ap: &ArrayPattern, src_reg: u8, kind: VariableDeclarationKind, is_const: bool, ctx: &mut CompileCtx,
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

    fn emit_object_binding(
        &self, op: &ObjectPattern, src_reg: u8, kind: VariableDeclarationKind, is_const: bool, ctx: &mut CompileCtx,
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

    pub(crate) fn emit_array_assignment(
        &self, ap: &ArrayAssignmentTarget, src_reg: u8, ctx: &mut CompileCtx,
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

    pub(crate) fn emit_object_assignment(
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
}
