use crate::compiler::{is_side_effect_free, BinaryOperator, CompileCtx, Compiler, Label};
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_parser::{ChainElement, Expression, LogicalOperator, SimpleAssignmentTarget, UnaryOperator, UpdateOperator};

impl Compiler {
    fn count_binary_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::BinaryExpression(bin) = expr else {
            return;
        };
        let checkpoint = ctx.reg_checkpoint();
        self.count_expression(&bin.left, ctx);
        self.count_expression(&bin.right, ctx);
        ctx.projected_pc += 1; // ADD/SUB/MUL/DIV/etc.
        if is_side_effect_free(&bin.left) && is_side_effect_free(&bin.right) {
            ctx.restore_reg_checkpoint(checkpoint.saturating_add(1));
        }
    }

    fn count_private_in_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::PrivateInExpression(pin) = expr else {
            return;
        };
        self.count_expression(&pin.right, ctx);
        ctx.count_private_access();
    }

    fn count_unary_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::UnaryExpression(un) = expr else {
            return;
        };
        match un.operator {
            UnaryOperator::Delete => self.count_delete_expression(&un.argument, ctx),
            _ => {
                self.count_expression(&un.argument, ctx);
                ctx.projected_pc += 1; // NEG/TYPEOF/VOID/NOT
            }
        }
    }

    fn count_delete_expression(&self, argument: &Expression, ctx: &mut CompileCtx) {
        match argument {
            Expression::Identifier(_) => {
                // SyntaxError at compile time, no bytecode cost.
            }
            Expression::StaticMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.count_delete_static();
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                self.count_expression(&member.expression, ctx);
                ctx.projected_pc += 1; // DELETE_PROP_DYNAMIC
            }
            Expression::ChainExpression(chain) => {
                self.count_delete_chain_expression(&chain.expression, ctx);
            }
            _ => {}
        }
    }

    fn count_delete_chain_expression(&self, chain: &ChainElement, ctx: &mut CompileCtx) {
        match chain {
            ChainElement::StaticMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                if member.optional {
                    self.count_optional_guard(ctx);
                }
                ctx.count_delete_static();
            }
            ChainElement::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                if member.optional {
                    self.count_optional_guard(ctx);
                }
                self.count_expression(&member.expression, ctx);
                ctx.projected_pc += 1; // DELETE_PROP_DYNAMIC
            }
            _ => {}
        }
        ctx.projected_pc += 1; // JMP over nullish true writer
        ctx.projected_pc += 1; // LOAD_CONST true
    }

    fn count_update_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::UpdateExpression(update) = expr else {
            return;
        };
        match &update.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                let is_upvalue = ctx.current_upvalue_captures.iter().any(|u| u.name == name);
                let is_captured = ctx.scopes.symbols.lookup_is_captured(name);
                if is_upvalue || is_captured {
                    ctx.alloc_reg(); // val_reg
                    ctx.count_instr(); // LOAD_UPVALUE or CELL_GET
                    ctx.count_load_const(); // 1, alloc + instr
                    ctx.count_instr(); // ADD/SUB
                    ctx.count_instr(); // STORE_UPVALUE or CELL_SET
                } else {
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                }
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.count_load_const(); // key
                ctx.alloc_reg();
                ctx.count_ic_instr_with_ext(); // MEMBER_INC/MEMBER_DEC + 3 ext words
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                self.count_expression(&member.expression, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
            _ => {
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
        }
    }

    pub(crate) fn count_operator(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::BinaryExpression(_) => self.count_binary_expression(expr, ctx),
            Expression::PrivateInExpression(_) => self.count_private_in_expression(expr, ctx),
            Expression::UnaryExpression(_) => self.count_unary_expression(expr, ctx),
            Expression::UpdateExpression(_) => self.count_update_expression(expr, ctx),
            _ => {}
        }
    }

    fn count_conditional_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::ConditionalExpression(cond) = expr else {
            return;
        };
        let id = ctx.next_label_id();
        let else_label = Label::TernaryElse(id);
        let end_label = Label::TernaryEnd(id);

        self.count_expression(&cond.test, ctx);
        ctx.projected_pc += 1; // JMP_IF_FALSE
        self.count_expression(&cond.consequent, ctx);
        ctx.alloc_reg(); // result register
        ctx.projected_pc += 1; // LOAD_VAR result <- consequent
        ctx.projected_pc += 1; // JMP to end
        ctx.labels.label_map.insert(else_label, ctx.projected_pc);
        self.count_expression(&cond.alternate, ctx);
        ctx.projected_pc += 1; // LOAD_VAR result <- alternate
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }

    fn count_sequence_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::SequenceExpression(seq) = expr else {
            return;
        };
        for e in &seq.expressions {
            self.count_expression(e, ctx);
        }
    }

    fn count_logical_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::LogicalExpression(log) = expr else {
            return;
        };
        self.count_expression(&log.left, ctx);

        let is_simple = is_side_effect_free(&log.left) && is_side_effect_free(&log.right);

        if is_simple {
            self.count_expression(&log.right, ctx);
            ctx.alloc_reg();
            ctx.projected_pc += 1; // AND/OR
        } else if matches!(log.operator, LogicalOperator::Coalesce) {
            // Mirror emit_logical_expression's Coalesce path: it patches raw bytecode
            // positions and does NOT consume a label id. Consuming one here would shift
            // every later construct's label id and break resolve_label at emit time.
            ctx.alloc_reg(); // dup register
            ctx.projected_pc += 1; // LOAD_VAR (DUP)
            ctx.projected_pc += 1; // JMP_IF_NULLISH
            ctx.projected_pc += 1; // JMP over RHS on non-nullish
            self.count_expression(&log.right, ctx);
            ctx.projected_pc += 1; // LOAD_VAR (overwrite)
        } else {
            let id = ctx.next_label_id();
            ctx.alloc_reg(); // dup register
            ctx.projected_pc += 1; // LOAD_VAR (DUP)
            ctx.projected_pc += 1; // JMP_IF_FALSE or JMP_IF_TRUE
            self.count_expression(&log.right, ctx);
            ctx.projected_pc += 1; // LOAD_VAR (overwrite)
            let skip_label = match log.operator {
                LogicalOperator::And => Label::TernaryEnd(id),
                LogicalOperator::Or => Label::TernaryElse(id),
                LogicalOperator::Coalesce => unreachable!("coalesce handled above"),
            };
            ctx.labels.label_map.insert(skip_label, ctx.projected_pc);
        }
    }

    pub(crate) fn count_conditional_chain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::ConditionalExpression(_) => self.count_conditional_expression(expr, ctx),
            Expression::SequenceExpression(_) => self.count_sequence_expression(expr, ctx),
            Expression::LogicalExpression(_) => self.count_logical_expression(expr, ctx),
            _ => {}
        }
    }
}

impl Compiler {
    fn emit_private_in_expression(
        &self, pin: &oxide_parser::PrivateInExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let obj_reg = self.emit_expression(&pin.right, ctx)?;
        let key_reg = self.emit_private_id_reg(pin.left.name.as_str(), ctx)?;
        let result_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::PRIVATE_BRAND_IN, result_reg, obj_reg, key_reg));
        Ok(result_reg)
    }

    fn emit_binary_expression(&self, bin: &oxide_parser::BinaryExpression, ctx: &mut CompileCtx) -> Result<u8, String> {
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

    fn emit_unary_expression(&self, un: &oxide_parser::UnaryExpression, ctx: &mut CompileCtx) -> Result<u8, String> {
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
            UnaryOperator::Delete => Err("invalid delete target".into()),
        }
    }

    fn emit_conditional_expression(
        &self, cond: &oxide_parser::ConditionalExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
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

    fn emit_logical_expression(
        &self, log: &oxide_parser::LogicalExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
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
            return Ok(r);
        }

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

        let id = ctx.next_label_id();
        let skip_label = match log.operator {
            LogicalOperator::And => Label::TernaryEnd(id),
            LogicalOperator::Or => Label::TernaryElse(id),
            LogicalOperator::Coalesce => return Err("invalid logical operator dispatch".into()),
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
            LogicalOperator::Coalesce => return Err("invalid logical operator dispatch".into()),
        }

        let right_reg = self.emit_expression(&log.right, ctx)?;
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, right_reg, 0));

        Ok(dup_reg)
    }

    fn emit_update_expression(
        &self, update: &oxide_parser::UpdateExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        match &update.argument {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                let name = id.name.as_str();
                // Check if this is an upvalue or captured cell reference
                let uv_idx = ctx.current_upvalue_captures.iter().position(|u| u.name == name);
                let is_captured = ctx.scopes.symbols.lookup_is_captured(name);
                if let Some(uv) = uv_idx {
                    // Upvalue: LOAD_UPVALUE + CONST(1) + ADD/SUB + STORE_UPVALUE
                    let val_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::LOAD_UPVALUE, val_reg, uv as u8, 0));
                    let one_idx = ctx.add_constant(Constant::Int(1));
                    let one_reg = ctx.alloc_reg();
                    ctx.emit_load_const(one_reg, one_idx);
                    let op = if update.operator == UpdateOperator::Increment {
                        OpCode::ADD
                    } else {
                        OpCode::SUB
                    };
                    ctx.emit(opcode::encode(op, val_reg, val_reg, one_reg));
                    ctx.emit(opcode::encode(OpCode::STORE_UPVALUE, 0, val_reg, uv as u8));
                    Ok(val_reg)
                } else if is_captured {
                    // Captured cell: CELL_GET + CONST(1) + ADD/SUB + CELL_SET
                    let cell_idx = ctx
                        .scopes
                        .cell_registry
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, idx)| *idx)
                        .unwrap_or(0);
                    let val_reg = ctx.alloc_reg();
                    if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
                        ctx.emit(opcode::encode(OpCode::CELL_GET, val_reg, binding.reg, cell_idx));
                    } else {
                        ctx.emit(opcode::encode(OpCode::CELL_GET, val_reg, 0, cell_idx));
                    }
                    let one_idx = ctx.add_constant(Constant::Int(1));
                    let one_reg = ctx.alloc_reg();
                    ctx.emit_load_const(one_reg, one_idx);
                    let op = if update.operator == UpdateOperator::Increment {
                        OpCode::ADD
                    } else {
                        OpCode::SUB
                    };
                    ctx.emit(opcode::encode(op, val_reg, val_reg, one_reg));
                    ctx.emit(opcode::encode(OpCode::CELL_SET, 0, val_reg, cell_idx));
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
                    ctx.emit(opcode::encode(op, var_reg, result_reg, result_reg));
                    Ok(result_reg)
                }
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
        }
    }

    pub(crate) fn emit_operator(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
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
