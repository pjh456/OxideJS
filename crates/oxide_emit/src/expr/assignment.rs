//! 赋值表达式 emit：简单 `=`、复合赋值与解构赋值（数组/对象 pattern）。
//! 函数：`emit_assignment_expression`。

use crate::{CompileCtx, Emitter};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_parser::AssignmentOperator;

impl Emitter {
    /// 标识符复合赋值的静态路径：upvalue / 被捕获 cell 走显式读取-运算-写回
    /// （写穿透共享单元），普通槽用 COMPOUND_* 寄存器内 RMW。返回结果寄存器。
    ///
    /// # 边界与前提
    /// - `op` 必须是 COMPOUND_* 族（rd 兼作 lhs 源，a 为 rhs）。
    /// - upvalue / cell 场景忽略 const 语义（共享单元由闭包机制保证）。
    fn emit_compound_identifier_static(
        &self, name: &str, op: OpCode, rhs: u32, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
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
    }

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
                    AssignmentOperator::BitwiseAnd => ctx.inst(Inst::compound_member_bit_and(obj, val, key)),
                    AssignmentOperator::BitwiseOR => ctx.inst(Inst::compound_member_bit_or(obj, val, key)),
                    AssignmentOperator::BitwiseXOR => ctx.inst(Inst::compound_member_bit_xor(obj, val, key)),
                    AssignmentOperator::ShiftLeft => ctx.inst(Inst::compound_member_shl(obj, val, key)),
                    AssignmentOperator::ShiftRight => ctx.inst(Inst::compound_member_shr(obj, val, key)),
                    AssignmentOperator::ShiftRightZeroFill => ctx.inst(Inst::compound_member_ushr(obj, val, key)),
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
            if assign.operator != AssignmentOperator::Assign {
                // 复合赋值：读属性 → 运算 → 写回。val_reg 同时承载旧值与新值。
                let val_reg = ctx.alloc_reg();
                ctx.inst(Inst::new(
                    OpCode::GET_PROP_DYNAMIC,
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    Operand::Reg(val_reg),
                ));
                let rhs = self.emit_expression(&assign.right, ctx)?;
                let op = match assign.operator {
                    AssignmentOperator::Addition => OpCode::ADD,
                    AssignmentOperator::Subtraction => OpCode::SUB,
                    AssignmentOperator::Multiplication => OpCode::MUL,
                    AssignmentOperator::Division => OpCode::DIV,
                    AssignmentOperator::Remainder => OpCode::MOD,
                    AssignmentOperator::Exponential => OpCode::COMPOUND_EXP,
                    AssignmentOperator::BitwiseAnd => OpCode::BIT_AND,
                    AssignmentOperator::BitwiseOR => OpCode::BIT_OR,
                    AssignmentOperator::BitwiseXOR => OpCode::BIT_XOR,
                    AssignmentOperator::ShiftLeft => OpCode::SHL,
                    AssignmentOperator::ShiftRight => OpCode::SHR,
                    AssignmentOperator::ShiftRightZeroFill => OpCode::USHR,
                    _ => return Err(format!("compound assignment operator {:?} not supported", assign.operator)),
                };
                ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(val_reg), Operand::Reg(rhs)));
                ctx.inst(Inst::new(
                    OpCode::SET_PROP_DYNAMIC,
                    Operand::Reg(obj_reg),
                    Operand::Reg(key_reg),
                    Operand::Reg(val_reg),
                ));
                return Ok(val_reg);
            }
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
                    // with 体内自由标识符的复合赋值走动态路径：对象有属性则读对象、
                    // 运算后写回对象，否则回退外层静态复合。
                    if !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name) {
                        let obj_reg = ctx.innermost_with_obj().expect("with stack non-empty");
                        let key_idx = ctx.add_constant(Constant::String(name.to_string()));
                        let key_reg = ctx.alloc_reg();
                        ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));

                        let has_reg = ctx.alloc_reg();
                        ctx.inst(Inst::new(
                            OpCode::IN,
                            Operand::Reg(has_reg),
                            Operand::Reg(key_reg),
                            Operand::Reg(obj_reg),
                        ));
                        let fallback_label = ctx.next_label_id();
                        let end_label = ctx.next_label_id();
                        ctx.inst(Inst::jmp_if_false(has_reg, fallback_label));

                        // 对象有该属性：读对象属性 → 运算 → 写回对象。
                        let val_reg = ctx.alloc_reg();
                        ctx.inst(Inst::new(
                            OpCode::GET_PROP_DYNAMIC,
                            Operand::Reg(obj_reg),
                            Operand::Reg(key_reg),
                            Operand::Reg(val_reg),
                        ));
                        ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(rhs), Operand::None));
                        ctx.inst(Inst::new(
                            OpCode::SET_PROP_DYNAMIC,
                            Operand::Reg(obj_reg),
                            Operand::Reg(key_reg),
                            Operand::Reg(val_reg),
                        ));
                        ctx.inst(Inst::jmp(end_label));

                        // 对象无该属性：回退静态复合赋值，结果写入同一 val_reg。
                        ctx.labels.set_label_pos(fallback_label, ctx.insts.len());
                        let fallback_val = self.emit_compound_identifier_static(name, op, rhs, ctx)?;
                        ctx.inst(Inst::new(
                            OpCode::LOAD_VAR,
                            Operand::Reg(val_reg),
                            Operand::Reg(fallback_val),
                            Operand::None,
                        ));
                        ctx.labels.set_label_pos(end_label, ctx.insts.len());
                        return Ok(val_reg);
                    }
                    self.emit_compound_identifier_static(name, op, rhs, ctx)
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
                // with 体内的自由标识符走动态写：对象有属性则写对象，否则写外层。
                if !ctx.with_stack.is_empty() && !ctx.is_with_internal_binding(name) {
                    let is_const = ctx.lookup_const_flag(name);
                    let const_flag: u16 = if is_const { 1 } else { 0 };
                    self.emit_with_dynamic_write(name, val_reg, const_flag, ctx);
                    return Ok(val_reg);
                }
                let is_const = ctx.lookup_const_flag(name);
                let const_flag: u16 = if is_const { 1 } else { 0 };
                self.emit_identifier_store(name, val_reg, const_flag, ctx);
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
