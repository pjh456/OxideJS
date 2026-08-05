use crate::compiler::{CompileCtx, Compiler};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
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
        ctx.inst(Inst::new(OpCode::STRICT_EQ, Operand::Reg(eq_reg as u32), Operand::Reg(val_reg as u32), Operand::Reg(undef_reg as u32)));
        // val != undefined（eq 为 false）→ 跳过默认值
        let end_label = ctx.next_label_id();
        ctx.inst(Inst::jmp_if_false(eq_reg, end_label));
        let default_reg = self.emit_expression(default_expr, ctx)?;
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(val_reg as u32), Operand::Reg(default_reg as u32), Operand::None));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
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
        if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
            ctx.inst(Inst::new(OpCode::MAKE_CELL, Operand::Reg(src_reg as u32), Operand::Imm(cell_idx as u16), Operand::None));
        } else {
            ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(target_reg as u32), Operand::Reg(src_reg as u32), Operand::Imm(if is_const { 1 } else { 0 })));
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
                ctx.inst(Inst::new(
                    OpCode::STORE_VAR,
                    Operand::Reg(var_reg as u32),
                    Operand::Reg(src_reg as u32),
                    Operand::Imm(if ctx.lookup_const_flag(name) { 1 } else { 0 }),
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
        ctx.inst(Inst::new(OpCode::FOR_OF_INIT, Operand::None, Operand::Reg(src_reg as u32), Operand::None));
        for elem in &ap.elements {
            let has_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg as u32), Operand::None, Operand::None));
            let val_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg as u32), Operand::None, Operand::None));
            if let Some(pattern) = elem {
                self.emit_binding_pattern(pattern, val_reg, kind, is_const, ctx)?;
            }
        }
        if let Some(rest) = &ap.rest {
            let rest_reg = self.emit_collect_rest_array(ctx)?;
            self.emit_binding_pattern(&rest.argument, rest_reg, kind, is_const, ctx)?;
        }
        ctx.inst(Inst::new(OpCode::FOR_OF_CLOSE, Operand::None, Operand::None, Operand::None));
        Ok(())
    }

    fn emit_collect_rest_array(&self, ctx: &mut CompileCtx) -> Result<u8, String> {
        let rest_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::NEW_ARRAY, Operand::Reg(rest_reg as u32), Operand::None, Operand::None));
        let idx_reg = ctx.alloc_reg();
        let zero_idx = ctx.add_constant(Constant::Int(0));
        ctx.inst(Inst::load_const(Operand::Reg(idx_reg as u32), zero_idx));
        let loop_start = ctx.next_label_id();
        let loop_end = ctx.next_label_id();
        ctx.labels.set_label_pos(loop_start, ctx.insts.len());
        let has_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg as u32), Operand::None, Operand::None));
        ctx.inst(Inst::jmp_if_false(has_reg, loop_end));
        let val_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg as u32), Operand::None, Operand::None));
        ctx.inst(Inst::new(OpCode::SET_ELEM, Operand::Reg(rest_reg as u32), Operand::Reg(idx_reg as u32), Operand::Reg(val_reg as u32)));
        let tmp_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::INC_PRE, Operand::Reg(idx_reg as u32), Operand::Reg(tmp_reg as u32), Operand::Reg(tmp_reg as u32)));
        ctx.inst(Inst::jmp(loop_start));
        ctx.labels.set_label_pos(loop_end, ctx.insts.len());
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
            ctx.inst(Inst::rest_object(Operand::Reg(rest_reg as u32), Operand::Reg(src_reg as u32), excluded_idx as u32));
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
                ctx.inst(Inst::new(
                    OpCode::STORE_VAR,
                    Operand::Reg(var_reg as u32),
                    Operand::Reg(src_reg as u32),
                    Operand::Imm(if ctx.lookup_const_flag(name) { 1 } else { 0 }),
                ));
                Ok(())
            }
            _ => Err("assignment target not supported".into()),
        }
    }

    pub(crate) fn emit_array_assignment(
        &self, ap: &ArrayAssignmentTarget, src_reg: u8, ctx: &mut CompileCtx,
    ) -> Result<(), String> {
        ctx.inst(Inst::new(OpCode::FOR_OF_INIT, Operand::None, Operand::Reg(src_reg as u32), Operand::None));
        for elem in &ap.elements {
            let has_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::FOR_OF_DONE, Operand::Reg(has_reg as u32), Operand::None, Operand::None));
            let val_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::FOR_OF_NEXT, Operand::Reg(val_reg as u32), Operand::None, Operand::None));
            if let Some(target) = elem {
                self.emit_assignment_maybe_default(target, val_reg, ctx)?;
            }
        }
        if let Some(rest) = &ap.rest {
            let rest_reg = self.emit_collect_rest_array(ctx)?;
            self.emit_assign_target(&rest.target, rest_reg, ctx)?;
        }
        ctx.inst(Inst::new(OpCode::FOR_OF_CLOSE, Operand::None, Operand::None, Operand::None));
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
                    ctx.inst(Inst::new(
                        OpCode::STORE_VAR,
                        Operand::Reg(var_reg as u32),
                        Operand::Reg(prop_reg as u32),
                        Operand::Imm(if ctx.lookup_const_flag(name) { 1 } else { 0 }),
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
            ctx.inst(Inst::rest_object(Operand::Reg(rest_reg as u32), Operand::Reg(src_reg as u32), excluded_idx as u32));
            self.emit_assign_target(&rest.target, rest_reg, ctx)?;
        }
        Ok(())
    }
}
