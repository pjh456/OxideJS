//! 运算符域：二元/一元/条件/逻辑/更新/`in` 表达式 emit 与分发。
//! 逻辑与 `??` 用短路跳转，复合求值利用 `is_side_effect_free` 优化。
//! 函数：`emit_operator` 及各类 `emit_*_expression`。

use crate::{is_side_effect_free, BinaryOperator, CompileCtx, Emitter};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{ChainElement, Expression, LogicalOperator, SimpleAssignmentTarget, UnaryOperator, UpdateOperator};

impl Emitter {
    fn emit_private_in_expression(
        &self, pin: &oxide_parser::PrivateInExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let obj_reg = self.emit_expression(&pin.right, ctx)?;
        let key_reg = self.emit_private_id_reg(pin.left.name.as_str(), ctx)?;
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::PRIVATE_BRAND_IN, Operand::Reg(result_reg), Operand::Reg(obj_reg), Operand::Reg(key_reg)));
        Ok(result_reg)
    }

    fn emit_binary_expression(&self, bin: &oxide_parser::BinaryExpression, ctx: &mut CompileCtx) -> Result<u32, String> {
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
        ctx.inst(Inst::new(op, Operand::Reg(left), Operand::Reg(left), Operand::Reg(right)));
        Ok(left)
    }

    fn emit_unary_expression(&self, un: &oxide_parser::UnaryExpression, ctx: &mut CompileCtx) -> Result<u32, String> {
        if matches!(un.operator, UnaryOperator::Delete) {
            return match &un.argument {
                Expression::Identifier(_) => {
                    Err("SyntaxError: delete of an unqualified identifier in strict mode".into())
                }
                Expression::StaticMemberExpression(member) => {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let prop_name = member.property.name.as_str();
                    let const_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    ctx.inst(Inst::delete_prop_static(Operand::Reg(obj_reg), const_idx as u32));
                    Ok(obj_reg)
                }
                Expression::ComputedMemberExpression(member) => {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let key_reg = self.emit_expression(&member.expression, ctx)?;
                    ctx.inst(Inst::new(OpCode::DELETE_PROP_DYNAMIC, Operand::Reg(obj_reg), Operand::Reg(obj_reg), Operand::Reg(key_reg)));
                    Ok(obj_reg)
                }
                Expression::ChainExpression(chain) => {
                    let short_label = ctx.next_label_id();
                    let result_reg = match &chain.expression {
                        ChainElement::StaticMemberExpression(member) => {
                            let obj_reg = self.emit_expression(&member.object, ctx)?;
                            if member.optional {
                                let dup_reg = ctx.alloc_reg();
                                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(obj_reg), Operand::None));
                                ctx.inst(Inst::jmp_if_nullish(dup_reg, short_label));
                            }
                            let prop_name = member.property.name.as_str();
                            let const_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                            ctx.inst(Inst::delete_prop_static(Operand::Reg(obj_reg), const_idx as u32));
                            obj_reg
                        }
                        ChainElement::ComputedMemberExpression(member) => {
                            let obj_reg = self.emit_expression(&member.object, ctx)?;
                            if member.optional {
                                let dup_reg = ctx.alloc_reg();
                                ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(obj_reg), Operand::None));
                                ctx.inst(Inst::jmp_if_nullish(dup_reg, short_label));
                            }
                            let key_reg = self.emit_expression(&member.expression, ctx)?;
                            ctx.inst(Inst::new(OpCode::DELETE_PROP_DYNAMIC, Operand::Reg(obj_reg), Operand::Reg(obj_reg), Operand::Reg(key_reg)));
                            obj_reg
                        }
                        _ => return Err("invalid delete target".into()),
                    };
                    let end_label = ctx.next_label_id();
                    ctx.inst(Inst::jmp(end_label));
                    ctx.labels.set_label_pos(short_label, ctx.insts.len());
                    let true_idx = ctx.add_constant(Constant::Boolean(true));
                    ctx.inst(Inst::load_const(Operand::Reg(result_reg), true_idx));
                    ctx.labels.set_label_pos(end_label, ctx.insts.len());
                    Ok(result_reg)
                }
                _ => Err("invalid delete target".into()),
            };
        }
        let arg = self.emit_expression(&un.argument, ctx)?;
        match un.operator {
            UnaryOperator::UnaryNegation => {
                ctx.inst(Inst::new(OpCode::NEG, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::Typeof => {
                ctx.inst(Inst::new(OpCode::TYPEOF, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::Void => {
                ctx.inst(Inst::new(OpCode::VOID, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::LogicalNot => {
                ctx.inst(Inst::new(OpCode::NOT, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::BitwiseNot => {
                ctx.inst(Inst::new(OpCode::BIT_NOT, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::UnaryPlus => {
                ctx.inst(Inst::new(OpCode::UNARY_PLUS, Operand::Reg(arg), Operand::Reg(arg), Operand::None));
                Ok(arg)
            }
            UnaryOperator::Delete => Err("invalid delete target".into()),
        }
    }

    fn emit_conditional_expression(
        &self, cond: &oxide_parser::ConditionalExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let else_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();

        let test_reg = self.emit_expression(&cond.test, ctx)?;
        ctx.inst(Inst::jmp_if_false(test_reg, else_label));

        let cons_reg = self.emit_expression(&cond.consequent, ctx)?;
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::Reg(cons_reg), Operand::None));

        ctx.inst(Inst::jmp(end_label));

        ctx.labels.set_label_pos(else_label, ctx.insts.len());
        let alt_reg = self.emit_expression(&cond.alternate, ctx)?;
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::Reg(alt_reg), Operand::None));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());

        Ok(result_reg)
    }

    fn emit_logical_expression(
        &self, log: &oxide_parser::LogicalExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        let left_reg = self.emit_expression(&log.left, ctx)?;

        if is_side_effect_free(&log.left) && is_side_effect_free(&log.right) {
            let right_reg = self.emit_expression(&log.right, ctx)?;
            let r = ctx.alloc_reg();
            let op = match log.operator {
                LogicalOperator::And => OpCode::AND,
                LogicalOperator::Or => OpCode::OR,
                LogicalOperator::Coalesce => OpCode::NULLISH,
            };
            ctx.inst(Inst::new(op, Operand::Reg(r), Operand::Reg(left_reg), Operand::Reg(right_reg)));
            return Ok(r);
        }

        if matches!(log.operator, LogicalOperator::Coalesce) {
            let rhs_label = ctx.next_label_id();
            let end_label = ctx.next_label_id();
            let dup_reg = ctx.alloc_reg();
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(left_reg), Operand::None));
            ctx.inst(Inst::jmp_if_nullish(dup_reg, rhs_label));
            ctx.inst(Inst::jmp(end_label));
            ctx.labels.set_label_pos(rhs_label, ctx.insts.len());
            let right_reg = self.emit_expression(&log.right, ctx)?;
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(right_reg), Operand::None));
            ctx.labels.set_label_pos(end_label, ctx.insts.len());
            return Ok(dup_reg);
        }

        let skip_label = match log.operator {
            LogicalOperator::And => ctx.next_label_id(),
            LogicalOperator::Or => ctx.next_label_id(),
            LogicalOperator::Coalesce => return Err("invalid logical operator dispatch".into()),
        };
        let dup_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(left_reg), Operand::None));

        match log.operator {
            LogicalOperator::And => ctx.inst(Inst::jmp_if_false(dup_reg, skip_label)),
            LogicalOperator::Or => ctx.inst(Inst::jmp_if_true(dup_reg, skip_label)),
            LogicalOperator::Coalesce => return Err("invalid logical operator dispatch".into()),
        }

        let right_reg = self.emit_expression(&log.right, ctx)?;
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(dup_reg), Operand::Reg(right_reg), Operand::None));
        ctx.labels.set_label_pos(skip_label, ctx.insts.len());

        Ok(dup_reg)
    }

    fn emit_update_expression(
        &self, update: &oxide_parser::UpdateExpression, ctx: &mut CompileCtx,
    ) -> Result<u32, String> {
        match &update.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                // Check if this is an upvalue or captured cell reference
                let uv_idx = ctx.current_upvalue_captures.iter().position(|u| u.name == name);
                let captured_cell = ctx.captured_bindings.get(name).copied();
                if let Some(uv) = uv_idx {
                    // Upvalue: LOAD_UPVALUE + CONST(1) + ADD/SUB + STORE_UPVALUE
                    let val_reg = ctx.alloc_reg();
                    ctx.inst(Inst::new(OpCode::LOAD_UPVALUE, Operand::Reg(val_reg), Operand::Imm(uv as u16), Operand::None));
                    let one_idx = ctx.add_constant(Constant::Int(1));
                    let one_reg = ctx.alloc_reg();
                    ctx.inst(Inst::load_const(Operand::Reg(one_reg), one_idx));
                    let op = if update.operator == UpdateOperator::Increment {
                        OpCode::ADD
                    } else {
                        OpCode::SUB
                    };
                    ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(val_reg), Operand::Reg(one_reg)));
                    ctx.inst(Inst::new(OpCode::STORE_UPVALUE, Operand::None, Operand::Reg(val_reg), Operand::Imm(uv as u16)));
                    Ok(val_reg)
                } else if let Some(cell_idx) = captured_cell {
                    // Captured cell: CELL_GET + CONST(1) + ADD/SUB + CELL_SET
                    let val_reg = ctx.alloc_reg();
                    if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
                        ctx.inst(Inst::new(OpCode::CELL_GET, Operand::Reg(val_reg), Operand::Reg(binding.reg), Operand::Imm(cell_idx as u16)));
                    } else {
                        ctx.inst(Inst::new(OpCode::CELL_GET, Operand::Reg(val_reg), Operand::None, Operand::Imm(cell_idx as u16)));
                    }
                    let one_idx = ctx.add_constant(Constant::Int(1));
                    let one_reg = ctx.alloc_reg();
                    ctx.inst(Inst::load_const(Operand::Reg(one_reg), one_idx));
                    let op = if update.operator == UpdateOperator::Increment {
                        OpCode::ADD
                    } else {
                        OpCode::SUB
                    };
                    ctx.inst(Inst::new(op, Operand::Reg(val_reg), Operand::Reg(val_reg), Operand::Reg(one_reg)));
                    ctx.inst(Inst::new(OpCode::CELL_SET, Operand::None, Operand::Reg(val_reg), Operand::Imm(cell_idx as u16)));
                    Ok(val_reg)
                } else {
                    let var_reg = ctx.lookup_or_global(name);
                    let result_reg = ctx.alloc_reg();
                    let op = match (update.operator, update.prefix) {
                        (UpdateOperator::Increment, true) => OpCode::INC_PRE,
                        (UpdateOperator::Increment, false) => OpCode::INC_POST,
                        (UpdateOperator::Decrement, true) => OpCode::DEC_PRE,
                        (UpdateOperator::Decrement, false) => OpCode::DEC_POST,
                    };
                    ctx.inst(Inst::new(op, Operand::Reg(var_reg), Operand::Reg(result_reg), Operand::Reg(result_reg)));
                    Ok(result_reg)
                }
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let key_idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(key_reg), key_idx));
                let val_reg = ctx.alloc_reg();
                let op = match update.operator {
                    UpdateOperator::Increment => OpCode::MEMBER_INC,
                    UpdateOperator::Decrement => OpCode::MEMBER_DEC,
                };
                match op {
                    OpCode::MEMBER_INC => {
                        ctx.inst(Inst::member_inc(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                    }
                    OpCode::MEMBER_DEC => {
                        ctx.inst(Inst::member_dec(Operand::Reg(obj_reg), Operand::Reg(val_reg), Operand::Reg(key_reg)));
                    }
                    _ => unreachable!(),
                }
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
                ctx.inst(Inst::new(op, Operand::Reg(obj_reg), Operand::Reg(key_reg), Operand::Reg(val_reg)));
                Ok(val_reg)
            }
            _ => Err("member update not yet supported".into()),
        }
    }

    pub(crate) fn emit_operator(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::BinaryExpression(bin) => self.emit_binary_expression(bin, ctx),
            Expression::PrivateInExpression(pin) => self.emit_private_in_expression(pin, ctx),
            Expression::UnaryExpression(un) => self.emit_unary_expression(un, ctx),
            Expression::ConditionalExpression(cond) => self.emit_conditional_expression(cond, ctx),
            Expression::LogicalExpression(log) => self.emit_logical_expression(log, ctx),
            Expression::UpdateExpression(update) => self.emit_update_expression(update, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
