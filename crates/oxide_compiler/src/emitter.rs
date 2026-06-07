use crate::compiler::Label;
use crate::opcode::{self, OpCode};
use oxide_parser::{
    AssignmentOperator, Expression, ForStatementInit, ForStatementLeft, SimpleAssignmentTarget,
    Statement, UnaryOperator, UpdateOperator,
};

use crate::compiler::{is_int_literal, is_side_effect_free, BinaryOperator, CompileCtx, Compiler};
use crate::module::Constant;

impl Compiler {
    pub(crate) fn emit_statement(
        &self,
        stmt: &Statement,
        ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        match stmt {
            Statement::ExpressionStatement(es) => {
                Ok(Some(self.emit_expression(&es.expression, ctx)?))
            }
            Statement::VariableDeclaration(decl) => {
                let mut r = None;
                for d in &decl.declarations {
                    let name = match &d.id {
                        oxide_parser::BindingPattern::BindingIdentifier(bi) => bi.name.as_str(),
                        _ => return Err("destructuring not supported".into()),
                    };
                    let var_reg = ctx.alloc_reg();
                    ctx.declare(name, var_reg)?;
                    if let Some(init) = &d.init {
                        let val_reg = self.emit_expression(init, ctx)?;
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, 0));
                        ctx.init_var(name);
                        r = Some(val_reg);
                    } else {
                        let idx = ctx.add_constant(Constant::Undefined);
                        let tmp = ctx.alloc_reg();
                        ctx.emit(opcode::encode(
                            OpCode::LOAD_CONST,
                            tmp,
                            (idx & 0xFF) as u8,
                            ((idx >> 8) & 0xFF) as u8,
                        ));
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, tmp, 0));
                        ctx.init_var(name);
                        r = Some(var_reg);
                    }
                }
                Ok(r)
            }
            Statement::ReturnStatement(ret) => {
                match &ret.argument {
                    Some(expr) => {
                        let r = self.emit_expression(expr, ctx)?;
                        ctx.emit(opcode::encode(OpCode::RETURN, r, 0, 0));
                    }
                    None => {
                        ctx.emit(opcode::encode(OpCode::RETURN, 0, 0, 0));
                    }
                }
                Ok(None)
            }
            Statement::BlockStatement(block) => {
                ctx.push_scope();
                let mut r = None;
                for s in &block.body {
                    if let Some(rr) = self.emit_statement(s, ctx)? {
                        r = Some(rr);
                    }
                }
                ctx.pop_scope();
                Ok(r)
            }
            Statement::IfStatement(ifs) => {
                let id = ctx.next_label_id();
                let else_label = Label::IfElse(id);
                let end_label = Label::IfEnd(id);

                let test_reg = self.emit_expression(&ifs.test, ctx)?;

                let else_pos = ctx.resolve_label(else_label)?;
                let offset = (else_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset as i16));

                let cons_reg = self.emit_statement(&ifs.consequent, ctx)?;
                let result_reg = ctx.alloc_reg();
                if let Some(r) = cons_reg {
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
                } else {
                    let undef_idx = ctx.add_constant(Constant::Undefined);
                    ctx.emit(opcode::encode(
                        OpCode::LOAD_CONST,
                        result_reg,
                        (undef_idx & 0xFF) as u8,
                        ((undef_idx >> 8) & 0xFF) as u8,
                    ));
                }

                let has_alt = ifs.alternate.is_some();
                if has_alt {
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    ctx.emit(opcode::encode_jmp(offset as i16));
                }

                if let Some(alt) = &ifs.alternate {
                    let alt_reg = self.emit_statement(alt, ctx)?;
                    if let Some(r) = alt_reg {
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, r, 0));
                    } else {
                        let undef_idx = ctx.add_constant(Constant::Undefined);
                        ctx.emit(opcode::encode(
                            OpCode::LOAD_CONST,
                            result_reg,
                            (undef_idx & 0xFF) as u8,
                            ((undef_idx >> 8) & 0xFF) as u8,
                        ));
                    }
                }

                Ok(Some(result_reg))
            }
            Statement::WhileStatement(wh) => {
                let id = ctx.next_label_id();
                let start_label = Label::WhileStart(id);
                let end_label = Label::WhileEnd(id);

                ctx.push_loop(end_label, start_label);

                let test_reg = self.emit_expression(&wh.test, ctx)?;

                let end_pos = ctx.resolve_label(end_label)?;
                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset as i16));

                self.emit_statement(&wh.body, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::DoWhileStatement(dw) => {
                let id = ctx.next_label_id();
                let start_label = Label::DoWhileStart(id);
                let end_label = Label::DoWhileEnd(id);

                ctx.push_loop(end_label, start_label);

                self.emit_statement(&dw.body, ctx)?;

                let test_reg = self.emit_expression(&dw.test, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_true(test_reg, offset as i16));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::ForStatement(fr) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForStart(id);
                let update_label = Label::ForUpdate(id);
                let end_label = Label::ForEnd(id);

                ctx.push_loop(end_label, update_label);

                if let Some(init) = &fr.init {
                    if let Some(expr) = init.as_expression() {
                        self.emit_expression(expr, ctx)?;
                    } else if let ForStatementInit::VariableDeclaration(decl) = init {
                        for d in &decl.declarations {
                            let name = match &d.id {
                                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                                    bi.name.as_str()
                                }
                                _ => return Err("destructuring not supported".into()),
                            };
                            let var_reg = ctx.alloc_reg();
                            ctx.declare(name, var_reg)?;
                            if let Some(init_expr) = &d.init {
                                let val_reg = self.emit_expression(init_expr, ctx)?;
                                ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, 0));
                                ctx.init_var(name);
                            }
                        }
                    }
                }

                if let Some(test) = &fr.test {
                    let test_reg = self.emit_expression(test, ctx)?;
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    ctx.emit(opcode::encode_jmp_if_false(test_reg, offset as i16));
                }

                self.emit_statement(&fr.body, ctx)?;

                if let Some(update) = &fr.update {
                    self.emit_expression(update, ctx)?;
                }

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));

                ctx.pop_loop();

                Ok(None)
            }
            Statement::ForInStatement(fi) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForInStart(id);
                let end_label = Label::ForInEnd(id);

                let obj_reg = self.emit_expression(&fi.right, ctx)?;
                ctx.emit(opcode::encode(OpCode::FOR_IN_INIT, 0, obj_reg, 0));

                ctx.push_loop(end_label, start_label);

                let done_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::FOR_IN_DONE, done_reg, 0, 0));

                let end_pos = ctx.resolve_label(end_label)?;
                let cleanup_jmp_offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(done_reg, 2));
                ctx.emit(opcode::encode_jmp(cleanup_jmp_offset as i16));

                let key_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::FOR_IN_NEXT, key_reg, 0, 0));

                match &fi.left {
                    ForStatementLeft::VariableDeclaration(decl) => {
                        for d in &decl.declarations {
                            let name = match &d.id {
                                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                                    bi.name.as_str()
                                }
                                _ => return Err("destructuring not supported".into()),
                            };
                            let var_reg = ctx.alloc_reg();
                            ctx.declare(name, var_reg)?;
                            ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
                            ctx.init_var(name);
                        }
                    }
                    ForStatementLeft::AssignmentTargetIdentifier(id_ref) => {
                        let name = id_ref.name.as_str();
                        let var_reg = ctx.lookup_or_global(name);
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, key_reg, 0));
                    }
                    _ => return Err("unsupported for-in left-hand side".into()),
                }

                self.emit_statement(&fi.body, ctx)?;

                let start_pos = ctx.resolve_label(start_label)?;
                let offset = (start_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));

                ctx.emit(opcode::encode(OpCode::FOR_IN_CLEANUP, 0, 0, 0));

                ctx.pop_loop();
                Ok(None)
            }
            Statement::SwitchStatement(sw) => {
                let id = ctx.next_label_id();
                let end_label = Label::SwitchEnd(id);
                ctx.push_switch(end_label);

                let disc_reg = self.emit_expression(&sw.discriminant, ctx)?;
                let cases = &sw.cases;

                for (case_idx, case) in cases.iter().enumerate() {
                    if let Some(test) = &case.test {
                        let test_reg = self.emit_expression(test, ctx)?;
                        let eq_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::EQ, eq_reg, disc_reg, test_reg));

                        let case_label = Label::SwitchCase(id * 256 + case_idx as u32);
                        let body_pos = ctx.resolve_label(case_label)?;
                        let offset = (body_pos as isize) - (ctx.bytecode.len() as isize);
                        ctx.emit(opcode::encode_jmp_if_true(eq_reg, offset as i16));
                    }
                }

                let has_default = cases.iter().any(|c| c.test.is_none());
                if !has_default {
                    let end_pos = ctx.resolve_label(end_label)?;
                    let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                    ctx.emit(opcode::encode_jmp(offset as i16));
                }

                for case in cases.iter() {
                    for s in &case.consequent {
                        self.emit_statement(s, ctx)?;
                    }
                }

                ctx.pop_switch();
                Ok(None)
            }
            Statement::BreakStatement(_) => {
                let break_label = if let Some(sw_label) = ctx.current_switch() {
                    *sw_label
                } else {
                    let (bl, _) = ctx
                        .current_loop()
                        .ok_or("break outside switch or loop".to_string())?;
                    *bl
                };
                let break_pos = ctx.resolve_label(break_label)?;
                let offset = (break_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));
                Ok(None)
            }
            Statement::ContinueStatement(_) => {
                let (_, continue_label) = ctx
                    .current_loop()
                    .ok_or("continue outside loop".to_string())?;
                let continue_pos = ctx.resolve_label(*continue_label)?;
                let offset = (continue_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn emit_expression(
        &self,
        expr: &Expression,
        ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        match expr {
            Expression::NumericLiteral(n) => {
                let idx = if is_int_literal(n.value) {
                    ctx.add_constant(Constant::Int(n.value as i32))
                } else {
                    ctx.add_constant(Constant::Number(n.value))
                };
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    r,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                Ok(r)
            }
            Expression::StringLiteral(s) => {
                let idx = ctx.add_constant(Constant::String(s.value.to_string()));
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    r,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                Ok(r)
            }
            Expression::BooleanLiteral(b) => {
                let idx = ctx.add_constant(Constant::Boolean(b.value));
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    r,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                Ok(r)
            }
            Expression::NullLiteral(_) => {
                let idx = ctx.add_constant(Constant::Null);
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    r,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                Ok(r)
            }
            Expression::BinaryExpression(bin) => {
                let left = self.emit_expression(&bin.left, ctx)?;
                let right = self.emit_expression(&bin.right, ctx)?;
                let op = match bin.operator {
                    BinaryOperator::Addition => OpCode::ADD,
                    BinaryOperator::Subtraction => OpCode::SUB,
                    BinaryOperator::Multiplication => OpCode::MUL,
                    BinaryOperator::Division => OpCode::DIV,
                    BinaryOperator::Remainder => OpCode::MOD,
                    BinaryOperator::Equality => OpCode::EQ,
                    BinaryOperator::Inequality => OpCode::NEQ,
                    BinaryOperator::LessThan => OpCode::LT,
                    BinaryOperator::GreaterThan => OpCode::GT,
                    BinaryOperator::LessEqualThan => OpCode::LTE,
                    BinaryOperator::GreaterEqualThan => OpCode::GTE,
                    BinaryOperator::In => OpCode::IN,
                    BinaryOperator::StrictEquality => OpCode::STRICT_EQ,
                    BinaryOperator::StrictInequality => OpCode::STRICT_NEQ,
                    _ => return Err(format!("unsupported binary operator: {:?}", bin.operator)),
                };
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(op, r, left, right));
                Ok(r)
            }
            Expression::UnaryExpression(un) => {
                let arg = self.emit_expression(&un.argument, ctx)?;
                match un.operator {
                    UnaryOperator::UnaryNegation => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::NEG, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::Typeof => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::TYPEOF, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::Void => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::VOID, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::LogicalNot => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::NOT, r, arg, 0));
                        Ok(r)
                    }
                    UnaryOperator::UnaryPlus => {
                        let r = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::UNARY_PLUS, r, arg, 0));
                        Ok(r)
                    }
                    _ => Err(format!("unsupported unary operator: {:?}", un.operator)),
                }
            }
            Expression::ConditionalExpression(cond) => {
                let id = ctx.next_label_id();
                let else_label = Label::TernaryElse(id);
                let end_label = Label::TernaryEnd(id);

                let test_reg = self.emit_expression(&cond.test, ctx)?;
                let else_pos = ctx.resolve_label(else_label)?;
                let end_pos = ctx.resolve_label(end_label)?;

                let offset = (else_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp_if_false(test_reg, offset as i16));

                let cons_reg = self.emit_expression(&cond.consequent, ctx)?;
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, cons_reg, 0));

                let offset = (end_pos as isize) - (ctx.bytecode.len() as isize);
                ctx.emit(opcode::encode_jmp(offset as i16));

                let alt_reg = self.emit_expression(&cond.alternate, ctx)?;
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, alt_reg, 0));

                Ok(result_reg)
            }
            Expression::LogicalExpression(log) => {
                use oxide_parser::LogicalOperator;
                let left_reg = self.emit_expression(&log.left, ctx)?;

                if is_side_effect_free(&log.left) && is_side_effect_free(&log.right) {
                    let right_reg = self.emit_expression(&log.right, ctx)?;
                    let r = ctx.alloc_reg();
                    let op = match log.operator {
                        LogicalOperator::And => OpCode::AND,
                        LogicalOperator::Or => OpCode::OR,
                        LogicalOperator::Coalesce => {
                            return Err("nullish coalescing not supported".into());
                        }
                    };
                    ctx.emit(opcode::encode(op, r, left_reg, right_reg));
                    Ok(r)
                } else {
                    let id = ctx.next_label_id();
                    let skip_label = match log.operator {
                        LogicalOperator::And => Label::TernaryEnd(id),
                        LogicalOperator::Or => Label::TernaryElse(id),
                        LogicalOperator::Coalesce => {
                            return Err("nullish coalescing not supported".into());
                        }
                    };
                    let skip_pos = ctx.resolve_label(skip_label)?;

                    let dup_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, left_reg, 0));

                    let offset = (skip_pos as isize) - (ctx.bytecode.len() as isize);
                    match log.operator {
                        LogicalOperator::And => {
                            ctx.emit(opcode::encode_jmp_if_false(dup_reg, offset as i16));
                        }
                        LogicalOperator::Or => {
                            ctx.emit(opcode::encode_jmp_if_true(dup_reg, offset as i16));
                        }
                        LogicalOperator::Coalesce => {
                            return Err("nullish coalescing not supported".into());
                        }
                    }

                    let right_reg = self.emit_expression(&log.right, ctx)?;
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, dup_reg, right_reg, 0));

                    Ok(dup_reg)
                }
            }
            Expression::StaticMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    key_reg,
                    (idx & 0xFF) as u8,
                    ((idx >> 8) & 0xFF) as u8,
                ));
                ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, obj_reg, key_reg));
                ctx.emit(0);
                ctx.emit(0);
                ctx.emit(0);
                Ok(obj_reg)
            }
            Expression::ComputedMemberExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_expression(&member.expression, ctx)?;
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::GET_PROP_DYNAMIC,
                    obj_reg,
                    key_reg,
                    r,
                ));
                Ok(r)
            }
            Expression::ObjectExpression(obj) => {
                let obj_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::NEW_OBJECT, obj_reg, 0, 0));
                for prop in &obj.properties {
                    let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop else {
                        return Err("spread properties not yet supported".into());
                    };
                    let prop_name = match &p.key {
                        oxide_parser::PropertyKey::StaticIdentifier(ident) => {
                            ident.name.as_str().to_string()
                        }
                        oxide_parser::PropertyKey::StringLiteral(s) => s.value.to_string(),
                        _ => return Err("unsupported object property key type".into()),
                    };
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(
                        OpCode::LOAD_CONST,
                        key_reg,
                        (idx & 0xFF) as u8,
                        ((idx >> 8) & 0xFF) as u8,
                    ));
                    let val_reg = self.emit_expression(&p.value, ctx)?;
                    ctx.emit(opcode::encode(OpCode::SET_PROP, obj_reg, val_reg, key_reg));
                }
                Ok(obj_reg)
            }
            Expression::ArrayExpression(arr) => {
                let arr_reg = ctx.alloc_reg();
                let n = arr.elements.len() as u16;
                ctx.emit(opcode::encode(
                    OpCode::NEW_ARRAY,
                    arr_reg,
                    (n & 0xFF) as u8,
                    ((n >> 8) & 0xFF) as u8,
                ));
                for (i, elem) in arr.elements.iter().enumerate() {
                    let Some(e) = elem.as_expression() else {
                        return Err("spread not supported".into());
                    };
                    let val_reg = self.emit_expression(e, ctx)?;
                    let idx_reg = ctx.alloc_reg();
                    let idx = ctx.add_constant(Constant::Int(i as i32));
                    ctx.emit(opcode::encode(
                        OpCode::LOAD_CONST,
                        idx_reg,
                        (idx & 0xFF) as u8,
                        ((idx >> 8) & 0xFF) as u8,
                    ));
                    ctx.emit(opcode::encode(OpCode::SET_ELEM, arr_reg, idx_reg, val_reg));
                }
                Ok(arr_reg)
            }
            Expression::AssignmentExpression(assign) => {
                if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left
                {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    let prop_name = member.property.name.as_str();
                    let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                    let key_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(
                        OpCode::LOAD_CONST,
                        key_reg,
                        (idx & 0xFF) as u8,
                        ((idx >> 8) & 0xFF) as u8,
                    ));
                    if assign.operator != AssignmentOperator::Assign {
                        let op = match assign.operator {
                            AssignmentOperator::Addition => OpCode::COMPOUND_MEMBER_ADD,
                            AssignmentOperator::Subtraction => OpCode::COMPOUND_MEMBER_SUB,
                            AssignmentOperator::Multiplication => OpCode::COMPOUND_MEMBER_MUL,
                            AssignmentOperator::Division => OpCode::COMPOUND_MEMBER_DIV,
                            AssignmentOperator::Remainder => OpCode::COMPOUND_MEMBER_MOD,
                            AssignmentOperator::Exponential => OpCode::COMPOUND_MEMBER_EXP,
                            _ => unreachable!(),
                        };
                        ctx.emit(opcode::encode(op, obj_reg, val_reg, key_reg));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        Ok(val_reg)
                    } else {
                        ctx.emit(opcode::encode(
                            OpCode::IC_SET_PROP,
                            obj_reg,
                            val_reg,
                            key_reg,
                        ));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        Ok(val_reg)
                    }
                } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) =
                    &assign.left
                {
                    let obj_reg = self.emit_expression(&member.object, ctx)?;
                    let key_reg = self.emit_expression(&member.expression, ctx)?;
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    ctx.emit(opcode::encode(
                        OpCode::SET_PROP_DYNAMIC,
                        obj_reg,
                        key_reg,
                        val_reg,
                    ));
                    Ok(val_reg)
                } else if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(id_ref) =
                    &assign.left
                {
                    if assign.operator != AssignmentOperator::Assign {
                        if assign.operator == AssignmentOperator::Addition
                            || assign.operator == AssignmentOperator::Subtraction
                            || assign.operator == AssignmentOperator::Multiplication
                            || assign.operator == AssignmentOperator::Division
                            || assign.operator == AssignmentOperator::Remainder
                            || assign.operator == AssignmentOperator::Exponential
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
                                _ => unreachable!(),
                            };
                            ctx.emit(opcode::encode(op, var_reg, rhs, 0));
                            Ok(var_reg)
                        } else {
                            Err(format!(
                                "compound assignment operator {:?} not supported",
                                assign.operator
                            ))
                        }
                    } else {
                        let val_reg = self.emit_expression(&assign.right, ctx)?;
                        let name = id_ref.name.as_str();
                        let var_reg = ctx.lookup_or_global(name);
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, 0));
                        Ok(val_reg)
                    }
                } else {
                    Err("assignment target not supported".into())
                }
            }
            Expression::UpdateExpression(update) => match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                    let name = id.name.as_str();
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
            },
            Expression::Identifier(ident) => {
                let var_reg = ctx.lookup(ident.name.as_str())?;
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, var_reg, 0));
                Ok(r)
            }
            Expression::ParenthesizedExpression(p) => self.emit_expression(&p.expression, ctx),
            Expression::CallExpression(call) => {
                let (callee_reg, this_reg) = match &call.callee {
                    Expression::StaticMemberExpression(member) => {
                        let obj_reg = self.emit_expression(&member.object, ctx)?;
                        let prop_name = member.property.name.as_str();
                        let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                        let key_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(
                            OpCode::LOAD_CONST,
                            key_reg,
                            (idx & 0xFF) as u8,
                            ((idx >> 8) & 0xFF) as u8,
                        ));
                        let callee_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(OpCode::LOAD_VAR, callee_reg, obj_reg, 0));
                        ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, callee_reg, key_reg));
                        ctx.emit(0);
                        ctx.emit(0);
                        ctx.emit(0);
                        (callee_reg, obj_reg)
                    }
                    _ => {
                        let callee_reg = self.emit_expression(&call.callee, ctx)?;
                        let this_idx = ctx.add_constant(Constant::Undefined);
                        let this_reg = ctx.alloc_reg();
                        ctx.emit(opcode::encode(
                            OpCode::LOAD_CONST,
                            this_reg,
                            (this_idx & 0xFF) as u8,
                            ((this_idx >> 8) & 0xFF) as u8,
                        ));
                        (callee_reg, this_reg)
                    }
                };
                let mut arg_regs = Vec::new();
                for arg in &call.arguments {
                    if let Some(expr) = arg.as_expression() {
                        arg_regs.push(self.emit_expression(expr, ctx)?);
                    }
                }
                let first_arg_reg = if arg_regs.is_empty() {
                    0u8
                } else {
                    arg_regs[0]
                };
                ctx.emit(opcode::encode(
                    OpCode::CALL,
                    callee_reg,
                    this_reg,
                    first_arg_reg,
                ));
                ctx.emit(arg_regs.len() as u32);
                Ok(0u8)
            }
            _ => Err(format!("unsupported expression type: {:?}", expr)),
        }
    }
}
