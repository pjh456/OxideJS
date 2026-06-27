use super::super::*;

impl Compiler {
    pub(in crate::counter) fn count_binding_pattern(
        &self, pattern: &oxide_parser::BindingPattern, ctx: &mut CompileCtx,
    ) {
        match pattern {
            oxide_parser::BindingPattern::BindingIdentifier(_) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
            oxide_parser::BindingPattern::ArrayPattern(ap) => {
                ctx.count_instr(); // FOR_OF_INIT
                for elem in &ap.elements {
                    ctx.alloc_reg();
                    ctx.count_instr(); // FOR_OF_DONE
                    ctx.alloc_reg();
                    ctx.count_instr(); // FOR_OF_NEXT
                    if let Some(inner) = elem {
                        self.count_binding_pattern(inner, ctx);
                    }
                }
                if let Some(rest) = &ap.rest {
                    self.count_rest_array(ctx);
                    self.count_binding_pattern(&rest.argument, ctx);
                }
                ctx.count_instr(); // FOR_OF_CLOSE
            }
            oxide_parser::BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.count_object_property_read_key(&prop.key, prop.computed, ctx);
                    self.count_binding_pattern(&prop.value, ctx);
                }
                if let Some(rest) = &op.rest {
                    ctx.alloc_reg();
                    ctx.count_instr_with_ext(1); // REST_OBJECT + excluded-keys ext
                    self.count_binding_pattern(&rest.argument, ctx);
                }
            }
            oxide_parser::BindingPattern::AssignmentPattern(ap) => {
                ctx.count_load_const(); // undefined
                ctx.count_instr(); // STRICT_EQ
                ctx.count_jump(); // JMP_IF_FALSE
                self.count_expression(&ap.right, ctx);
                ctx.count_instr(); // LOAD_VAR default
                self.count_binding_pattern(&ap.left, ctx);
            }
        }
    }

    pub(in crate::counter) fn count_rest_array(&self, ctx: &mut CompileCtx) {
        ctx.alloc_reg(); // rest
        ctx.count_instr(); // NEW_ARRAY
        ctx.count_load_const(); // idx = 0
        ctx.alloc_reg(); // has
        ctx.count_instr(); // FOR_OF_DONE
        ctx.count_jump(); // JMP_IF_FALSE
        ctx.alloc_reg(); // val
        ctx.count_instr(); // FOR_OF_NEXT
        ctx.count_instr(); // SET_ELEM
        ctx.alloc_reg(); // inc tmp
        ctx.count_instr(); // INC_PRE
        ctx.count_jump(); // JMP
    }

    pub(in crate::counter) fn count_array_assignment(
        &self, ap: &oxide_parser::ArrayAssignmentTarget, ctx: &mut CompileCtx,
    ) {
        ctx.count_instr(); // FOR_OF_INIT
        for elem in &ap.elements {
            ctx.alloc_reg();
            ctx.count_instr(); // FOR_OF_DONE
            ctx.alloc_reg();
            ctx.count_instr(); // FOR_OF_NEXT
            if elem.is_some() {
                ctx.alloc_reg();
                ctx.count_instr(); // STORE_VAR
            }
        }
        if let Some(rest) = &ap.rest {
            self.count_rest_array(ctx);
            // Mirror the emitter (emit_array_assignment -> emit_assign_target): the rest
            // target may itself be a nested pattern, not just an identifier.
            self.count_assign_target(&rest.target, ctx);
        }
        ctx.count_instr(); // FOR_OF_CLOSE
    }

    pub(in crate::counter) fn count_object_assignment(
        &self, op: &oxide_parser::ObjectAssignmentTarget, ctx: &mut CompileCtx,
    ) {
        for prop in &op.properties {
            match prop {
                oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
                    self.count_object_property_read_static(ctx);
                    if let Some(default_expr) = &id.init {
                        ctx.count_load_const(); // undefined
                        ctx.count_instr(); // STRICT_EQ
                        ctx.count_jump(); // JMP_IF_FALSE
                        self.count_expression(default_expr, ctx);
                        ctx.count_instr(); // LOAD_VAR default
                    }
                    ctx.alloc_reg();
                    ctx.count_instr(); // STORE_VAR
                }
                oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(prop) => {
                    self.count_object_property_read_key(&prop.name, prop.computed, ctx);
                    self.count_assignment_maybe_default(&prop.binding, ctx);
                }
            }
        }
        if let Some(rest) = &op.rest {
            ctx.alloc_reg();
            ctx.count_instr_with_ext(1); // REST_OBJECT + excluded-keys ext
                                         // Mirror the emitter (emit_object_assignment -> emit_assign_target): the rest
                                         // target may itself be a nested pattern, not just an identifier.
            self.count_assign_target(&rest.target, ctx);
        }
    }

    pub(in crate::counter) fn count_assignment_maybe_default(
        &self, target: &oxide_parser::AssignmentTargetMaybeDefault, ctx: &mut CompileCtx,
    ) {
        match target {
            oxide_parser::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(default) => {
                ctx.count_load_const(); // undefined
                ctx.count_instr(); // STRICT_EQ
                ctx.count_jump(); // JMP_IF_FALSE
                self.count_expression(&default.init, ctx);
                ctx.count_instr(); // LOAD_VAR default
                self.count_assign_target(&default.binding, ctx);
            }
            oxide_parser::AssignmentTargetMaybeDefault::ArrayAssignmentTarget(ap) => {
                self.count_array_assignment(ap, ctx)
            }
            oxide_parser::AssignmentTargetMaybeDefault::ObjectAssignmentTarget(op) => {
                self.count_object_assignment(op, ctx)
            }
            oxide_parser::AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg();
                ctx.count_instr(); // STORE_VAR
            }
            _ => {}
        }
    }

    pub(in crate::counter) fn count_assign_target(
        &self, target: &oxide_parser::AssignmentTarget, ctx: &mut CompileCtx,
    ) {
        match target {
            oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg();
                ctx.count_instr(); // STORE_VAR
            }
            oxide_parser::AssignmentTarget::ArrayAssignmentTarget(ap) => self.count_array_assignment(ap, ctx),
            oxide_parser::AssignmentTarget::ObjectAssignmentTarget(op) => self.count_object_assignment(op, ctx),
            _ => {}
        }
    }
}
