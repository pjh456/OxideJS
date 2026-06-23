use super::*;

impl Compiler {
    pub(in crate::emitter) fn emit_assignment_expression(
        &self, assign: &oxide_parser::AssignmentExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
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
                    _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
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
                        _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
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
}
