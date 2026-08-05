use crate::compiler::{CompileCtx, Compiler};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::AssignmentOperator;

impl Compiler {
    pub(crate) fn emit_assignment_expression(
        &self, assign: &oxide_parser::AssignmentExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left {
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                let store_label = ctx.next_label_id();
                let end_label = ctx.next_label_id();
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg as u32), idx));
                let result_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::Reg(obj_reg as u32), Operand::None));
                ctx.inst(Inst::ic_get(Operand::Reg(result_reg as u32), Operand::Reg(key_reg as u32)));
                self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                ctx.labels.set_label_pos(store_label, ctx.insts.len());
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                ctx.inst(Inst::ic_set(Operand::Reg(obj_reg as u32), Operand::Reg(val_reg as u32), Operand::Reg(key_reg as u32)));
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::Reg(val_reg as u32), Operand::None));
                ctx.labels.set_label_pos(end_label, ctx.insts.len());
                return Ok(result_reg);
            }
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            let prop_name = member.property.name.as_str();
            let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg as u32), idx));
            if assign.operator != AssignmentOperator::Assign {
                let obj = Operand::Reg(obj_reg as u32);
                let val = Operand::Reg(val_reg as u32);
                let key = Operand::Reg(key_reg as u32);
                match assign.operator {
                    AssignmentOperator::Addition => ctx.inst(Inst::compound_member_add(obj, val, key)),
                    AssignmentOperator::Subtraction => ctx.inst(Inst::compound_member_sub(obj, val, key)),
                    AssignmentOperator::Multiplication => ctx.inst(Inst::compound_member_mul(obj, val, key)),
                    AssignmentOperator::Division => ctx.inst(Inst::compound_member_div(obj, val, key)),
                    AssignmentOperator::Remainder => ctx.inst(Inst::compound_member_mod(obj, val, key)),
                    AssignmentOperator::Exponential => ctx.inst(Inst::compound_member_exp(obj, val, key)),
                    _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
                }
                Ok(val_reg)
            } else {
                ctx.inst(Inst::ic_set(Operand::Reg(obj_reg as u32), Operand::Reg(val_reg as u32), Operand::Reg(key_reg as u32)));
                Ok(val_reg)
            }
        } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) = &assign.left {
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                let store_label = ctx.next_label_id();
                let end_label = ctx.next_label_id();
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_expression(&member.expression, ctx)?;
                let result_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(OpCode::GET_PROP_DYNAMIC, Operand::Reg(obj_reg as u32), Operand::Reg(key_reg as u32), Operand::Reg(result_reg as u32)));
                self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                ctx.labels.set_label_pos(store_label, ctx.insts.len());
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                ctx.inst(Inst::new(OpCode::SET_PROP_DYNAMIC, Operand::Reg(obj_reg as u32), Operand::Reg(key_reg as u32), Operand::Reg(val_reg as u32)));
                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::Reg(val_reg as u32), Operand::None));
                ctx.labels.set_label_pos(end_label, ctx.insts.len());
                return Ok(result_reg);
            }
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let key_reg = self.emit_expression(&member.expression, ctx)?;
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            ctx.inst(Inst::new(OpCode::SET_PROP_DYNAMIC, Operand::Reg(obj_reg as u32), Operand::Reg(key_reg as u32), Operand::Reg(val_reg as u32)));
            Ok(val_reg)
        } else if let oxide_parser::AssignmentTarget::PrivateFieldExpression(member) = &assign.left {
            if assign.operator != AssignmentOperator::Assign {
                return Err("compound assignment to private fields not supported".into());
            }
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
            ctx.inst(Inst::new(OpCode::SET_PRIVATE, Operand::Reg(obj_reg as u32), Operand::Reg(val_reg as u32), Operand::Reg(key_reg as u32)));
            Ok(val_reg)
        } else if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(id_ref) = &assign.left {
            if assign.operator != AssignmentOperator::Assign {
                if let Some(logical_op) = assign.operator.to_logical_operator() {
                    let store_label = ctx.next_label_id();
                    let end_label = ctx.next_label_id();
                    let name = id_ref.name.as_str();
                    let var_reg = ctx.lookup_or_global(name);
                    let result_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::Reg(var_reg as u32), Operand::None));
                    self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                    ctx.labels.set_label_pos(store_label, ctx.insts.len());
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    let is_const = ctx.lookup_const_flag(name);
                    let const_flag = if is_const { 1 } else { 0 };
                    ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg as u32), Operand::Reg(val_reg as u32), Operand::Imm(const_flag)));
                    ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg as u32), Operand::Reg(val_reg as u32), Operand::None));
                    ctx.labels.set_label_pos(end_label, ctx.insts.len());
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
                    ctx.inst(Inst::new(op, Operand::Reg(var_reg as u32), Operand::Reg(rhs as u32), Operand::None));
                    Ok(var_reg)
                } else {
                    Err(format!("compound assignment operator {:?} not supported", assign.operator))
                }
            } else {
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                let name = id_ref.name.as_str();
                // Check if target is an upvalue reference
                if let Some(uv_idx) = ctx.current_upvalue_captures.iter().position(|u| u.name == name) {
                    ctx.inst(Inst::new(OpCode::STORE_UPVALUE, Operand::None, Operand::Reg(val_reg as u32), Operand::Imm(uv_idx as u16)));
                    return Ok(val_reg);
                }
                // Check if target is a captured cell
                if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
                    ctx.inst(Inst::new(OpCode::CELL_SET, Operand::None, Operand::Reg(val_reg as u32), Operand::Imm(cell_idx as u16)));
                    return Ok(val_reg);
                }
                let var_reg = ctx.lookup_or_global(name);
                let is_const = ctx.lookup_const_flag(name);
                let const_flag = if is_const { 1 } else { 0 };
                ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(var_reg as u32), Operand::Reg(val_reg as u32), Operand::Imm(const_flag)));
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
