use crate::opcode::{self, OpCode};
use oxide_parser::{AssignmentOperator, Expression, Statement, UnaryOperator};

use crate::compiler::{is_int_literal, BinaryOperator, CompileCtx, Compiler};
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
                    _ => Err(format!("unsupported unary operator: {:?}", un.operator)),
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
                let result_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::IC_GET_PROP,
                    result_reg,
                    obj_reg,
                    key_reg,
                ));
                ctx.emit(0);
                Ok(result_reg)
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
                    let idx = ctx.add_constant(Constant::Number(i as f64));
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
                    ctx.emit(opcode::encode(
                        OpCode::IC_SET_PROP,
                        obj_reg,
                        val_reg,
                        key_reg,
                    ));
                    ctx.emit(0);
                    Ok(val_reg)
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
                        return Err("compound assignment not yet supported".into());
                    }
                    let val_reg = self.emit_expression(&assign.right, ctx)?;
                    let name = id_ref.name.as_str();
                    let var_reg = ctx.lookup_or_global(name);
                    ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, val_reg, 0));
                    Ok(val_reg)
                } else {
                    Err("assignment target not supported".into())
                }
            }
            Expression::Identifier(ident) => {
                let var_reg = ctx.lookup(ident.name.as_str())?;
                let r = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::LOAD_VAR, r, var_reg, 0));
                Ok(r)
            }
            Expression::ParenthesizedExpression(p) => self.emit_expression(&p.expression, ctx),
            _ => Err(format!("unsupported expression type: {:?}", expr)),
        }
    }
}
