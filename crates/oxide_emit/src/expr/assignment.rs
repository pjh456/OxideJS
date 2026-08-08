//! 赋值表达式 emit：简单 `=`、复合赋值与解构赋值（数组/对象 pattern）。
//! 函数：`emit_assignment_expression`。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::AssignmentOperator;

impl Emitter {
    pub(crate) fn emit_assignment_expression(
        &self, assign: &oxide_parser::AssignmentExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left {
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                let store_label = ctx.next_label_id();
                let end_label = ctx.next_label_id();
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
                let result_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::LOAD_VAR,
                    Operand::Reg(result_reg),
                    Operand::Reg(obj_reg),
                    Operand::None,
                ));
                ctx.inst(Inst::ic_get(Operand::Reg(result_reg), Operand::Reg(key_reg)));
                self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                ctx.labels.set_label_pos(store_label, ctx.insts.len());
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                ctx.inst(Inst::ic_set(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                ctx.inst(Inst::new(
                    OpCode::LOAD_VAR,
                    Operand::Reg(result_reg),
                    Operand::Reg(val_reg),
                    Operand::None,
                ));
                ctx.labels.set_label_pos(end_label, ctx.insts.len());
                return Ok(result_reg);
            }
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            let prop_name = member.property.name.as_str();
            let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            if assign.operator != AssignmentOperator::Assign {
                let obj = Operand::Reg(obj_reg);
                let val = Operand::Reg(val_reg);
                let key = Operand::Reg(key_reg);
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
                ctx.inst(Inst::ic_set(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                Ok(val_reg)
            }
        } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) = &assign.left {
            if let Some(logical_op) = assign.operator.to_logical_operator() {
                let store_label = ctx.next_label_id();
                let end_label = ctx.next_label_id();
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_expression(&member.expression, ctx)?;
                let result_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::GET_PROP_DYNAMIC,
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    Operand::Reg(result_reg),
                ));
                self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                ctx.labels.set_label_pos(store_label, ctx.insts.len());
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                ctx.inst(Inst::new(
                    OpCode::SET_PROP_DYNAMIC,
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    Operand::Reg(val_reg),
                ));
                ctx.inst(Inst::new(
                    OpCode::LOAD_VAR,
                    Operand::Reg(result_reg),
                    Operand::Reg(val_reg),
                    Operand::None,
                ));
                ctx.labels.set_label_pos(end_label, ctx.insts.len());
                return Ok(result_reg);
            }
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let key_reg = self.emit_expression(&member.expression, ctx)?;
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            ctx.inst(Inst::new(
                OpCode::SET_PROP_DYNAMIC,
                Operand::Reg(obj_reg),
                Operand::Reg(key_reg),
                Operand::Reg(val_reg),
            ));
            Ok(val_reg)
        } else if let oxide_parser::AssignmentTarget::PrivateFieldExpression(member) = &assign.left {
            if assign.operator != AssignmentOperator::Assign {
                return Err("compound assignment to private fields not supported".into());
            }
            let obj_reg = self.emit_expression(&member.object, ctx)?;
            let val_reg = self.emit_expression(&assign.right, ctx)?;
            let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
            ctx.inst(Inst::new(
                OpCode::SET_PRIVATE,
                Operand::Reg(obj_reg),
                Operand::Reg(val_reg),
                Operand::Reg(key_reg),
            ));
            Ok(val_reg)
        } else if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(id_ref) = &assign.left {
            if assign.operator != AssignmentOperator::Assign {
                if let Some(logical_op) = assign.operator.to_logical_operator() {
                    let store_label = ctx.next_label_id();
                    let end_label = ctx.next_label_id();
                    let name = id_ref.name.as_str();
                    let var_reg = ctx.lookup_or_global(name);
                    let result_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(result_reg),
                        Operand::Reg(var_reg),
                        Operand::None,
                    ));
                    self.emit_logical_assign_test(logical_op, result_reg, store_label, end_label, ctx)?;
                    ctx.labels.set_label_pos(store_label, ctx.insts.len());
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    let is_const = ctx.lookup_const_flag(name);
                    let const_flag = if is_const { 1 } else { 0 };
                    ctx.inst(Inst::new(
                        OpCode::STORE_VAR,
                        Operand::Reg(var_reg),
                        Operand::Reg(val_reg),
                        Operand::Imm(const_flag),
                    ));
                    ctx.inst(Inst::new(
                        OpCode::LOAD_VAR,
                        Operand::Reg(result_reg),
                        Operand::Reg(val_reg),
                        Operand::None,
                    ));
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
                    // 目标为 upvalue / 被捕获 cell 时走显式读取-运算-写回，保证写穿透共享单元；
                    // 只有普通槽位才用 COMPOUND_* 的寄存器内 RMW（rd 兼作 lhs 源）。
                    let uv_idx = ctx.current_upvalue_captures.iter().position(|u| u.name == name);
                    let captured_cell = ctx.captured_bindings.get(name).copied();
                    if let Some(uv) = uv_idx {
                        let val_reg = ctx.alloc_reg();
                        ctx.inst(Inst::new(
                            OpCode::LOAD_UPVALUE,
                            Operand::Reg(val_reg),
                            Operand::Imm(uv as u16),
                            Operand::None,
                        ));
                        ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
                        ctx.inst(Inst::new(
                            OpCode::STORE_UPVALUE,
                            Operand::None,
                            Operand::Reg(val_reg),
                            Operand::Imm(uv as u16),
                        ));
                        Ok(val_reg)
                    } else if let Some(cell_idx) = captured_cell {
                        let val_reg = ctx.alloc_reg();
                        let a_operand = match ctx.scopes.symbols.lookup_any_binding(name) {
                            Some((binding, _)) => Operand::Reg(binding.reg),
                            None => Operand::None,
                        };
                        ctx.inst(Inst::new(
                            OpCode::CELL_GET,
                            Operand::Reg(val_reg),
                            a_operand,
                            Operand::Imm(cell_idx as u16),
                        ));
                        ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
                        ctx.inst(Inst::new(
                            OpCode::CELL_SET,
                            Operand::None,
                            Operand::Reg(val_reg),
                            Operand::Imm(cell_idx as u16),
                        ));
                        Ok(val_reg)
                    } else {
                        let var_reg = ctx.lookup_or_global(name);
                        ctx.inst(Inst::new(op, Operand::Reg(var_reg), Operand::Reg(rhs), Operand::None));
                        Ok(var_reg)
                    }
                } else {
                    Err(format!("compound assignment operator {:?} not supported", assign.operator))
                }
            } else {
                let val_reg = self.emit_expression(&assign.right, ctx)?;
                let name = id_ref.name.as_str();
                // 赋值函数/箭头/class 表达式 → 推断 name。
                if crate::is_anonymous_function_definition(&assign.right) {
                    if let Some(sub_mod) = ctx.nested.last_mut() {
                        if sub_mod.function_name.is_none() {
                            sub_mod.function_name = Some(name.to_string());
                        }
                    }
                }
                // 目标若是 upvalue 引用，走 STORE_UPVALUE
                if let Some(uv_idx) = ctx.current_upvalue_captures.iter().position(|u| u.name == name) {
                    ctx.inst(Inst::new(
                        OpCode::STORE_UPVALUE,
                        Operand::None,
                        Operand::Reg(val_reg),
                        Operand::Imm(uv_idx as u16),
                    ));
                    return Ok(val_reg);
                }
                // 目标若是被捕获 cell，走 CELL_SET
                if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
                    ctx.inst(Inst::new(
                        OpCode::CELL_SET,
                        Operand::None,
                        Operand::Reg(val_reg),
                        Operand::Imm(cell_idx as u16),
                    ));
                    return Ok(val_reg);
                }
                let var_reg = ctx.lookup_or_global(name);
                let is_const = ctx.lookup_const_flag(name);
                let const_flag = if is_const { 1 } else { 0 };
                ctx.inst(Inst::new(
                    OpCode::STORE_VAR,
                    Operand::Reg(var_reg),
                    Operand::Reg(val_reg),
                    Operand::Imm(const_flag),
                ));
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
