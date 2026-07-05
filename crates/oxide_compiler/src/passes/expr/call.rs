use super::*;

impl Compiler {
    fn count_call_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::CallExpression(call) = expr else {
            return;
        };
        if matches!(&call.callee, Expression::Super(_)) {
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    self.count_expression(expr, ctx);
                }
            }
            ctx.alloc_reg();
            ctx.count_call_instr_with_arg_ext();
            if let Some(words) = ctx.after_super_count_words.take() {
                ctx.projected_pc += words;
            }
            return;
        }
        match &call.callee {
            Expression::PrivateFieldExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.count_private_access();
            }
            Expression::StaticMemberExpression(member) => {
                if matches!(&member.object, Expression::Super(_)) {
                    ctx.count_load_var();
                    ctx.count_load_const();
                    ctx.alloc_reg();
                    ctx.count_instr();
                } else {
                    self.count_expression(&member.object, ctx);
                    ctx.count_load_const();
                    ctx.count_load_var();
                    ctx.count_ic_instr_with_ext();
                }
            }
            _ => {
                self.count_expression(&call.callee, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1;
            }
        }
        for arg in &call.arguments {
            if let Some(expr) = arg.as_expression() {
                self.count_expression(expr, ctx);
            }
        }
        ctx.count_call_instr_with_arg_ext();
        ctx.count_load_var();
    }

    fn count_new_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        let Expression::NewExpression(ne) = expr else {
            return;
        };
        self.count_expression(&ne.callee, ctx);
        for arg in &ne.arguments {
            if let Some(expr) = arg.as_expression() {
                self.count_expression(expr, ctx);
            }
        }
        ctx.alloc_reg();
        ctx.count_instr_with_ext(1);
    }

    fn emit_call_expression(&self, call: &oxide_parser::CallExpression, ctx: &mut CompileCtx) -> Result<u8, String> {
        if matches!(&call.callee, Expression::Super(_)) {
            if !ctx.in_derived_constructor {
                return Err("super() only supported in derived constructors".into());
            }
            let mut arg_regs = Vec::new();
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    arg_regs.push(self.emit_expression(expr, ctx)?);
                }
            }
            let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
            let result_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(OpCode::SUPER_CALL, result_reg, first_arg_reg, 0));
            ctx.emit(arg_regs.len() as u32);
            if !ctx.after_super_inserted {
                if let Some(field_code) = ctx.after_super_insert.clone() {
                    ctx.bytecode.extend(field_code);
                    ctx.after_super_inserted = true;
                }
            }
            return Ok(result_reg);
        }
        let (callee_reg, this_reg) = match &call.callee {
            Expression::PrivateFieldExpression(member) => {
                let obj_reg = self.emit_expression(&member.object, ctx)?;
                let key_reg = self.emit_private_id_reg(member.field.name.as_str(), ctx)?;
                let callee_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(OpCode::GET_PRIVATE, callee_reg, obj_reg, key_reg));
                (callee_reg, obj_reg)
            }
            Expression::StaticMemberExpression(member) => {
                let is_super_member = matches!(&member.object, Expression::Super(_));
                let obj_reg = if is_super_member {
                    let this_reg = ctx.alloc_reg();
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, this_reg, 254, 0));
                    this_reg
                } else {
                    self.emit_expression(&member.object, ctx)?
                };
                let prop_name = member.property.name.as_str();
                let idx = ctx.add_constant(Constant::String(prop_name.to_string()));
                let key_reg = ctx.alloc_reg();
                ctx.emit_load_const(key_reg, idx);
                let callee_reg = ctx.alloc_reg();
                if is_super_member {
                    if !ctx.in_instance_method && !ctx.in_static_method && !ctx.in_derived_constructor {
                        return Err("super property only supported in class methods".into());
                    }
                    let op = if ctx.in_static_method {
                        OpCode::SUPER_STATIC_GET_PROP
                    } else {
                        OpCode::SUPER_GET_PROP
                    };
                    ctx.emit(opcode::encode(op, callee_reg, obj_reg, key_reg));
                } else {
                    ctx.emit(opcode::encode(OpCode::LOAD_VAR, callee_reg, obj_reg, 0));
                    ctx.emit(opcode::encode(OpCode::IC_GET_PROP, 0, callee_reg, key_reg));
                    ctx.emit(0);
                    ctx.emit(0);
                    ctx.emit(0);
                }
                (callee_reg, obj_reg)
            }
            _ => {
                let callee_reg = self.emit_expression(&call.callee, ctx)?;
                let this_idx = ctx.add_constant(Constant::Undefined);
                let this_reg = ctx.alloc_reg();
                ctx.emit_load_const(this_reg, this_idx);
                (callee_reg, this_reg)
            }
        };
        let mut arg_regs = Vec::new();
        for arg in &call.arguments {
            if let Some(expr) = arg.as_expression() {
                arg_regs.push(self.emit_expression(expr, ctx)?);
            }
        }
        let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
        let op = match &call.callee {
            Expression::Identifier(ident) if ctx.is_builtin(ident.name.as_str()) => OpCode::CALL_NATIVE,
            _ => OpCode::CALL,
        };
        ctx.emit(opcode::encode(op, callee_reg, this_reg, first_arg_reg));
        ctx.emit(arg_regs.len() as u32);
        let result_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::LOAD_VAR, result_reg, 0, 0));
        Ok(result_reg)
    }

    pub(crate) fn count_call_domain(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::CallExpression(_) => self.count_call_expression(expr, ctx),
            Expression::NewExpression(_) => self.count_new_expression(expr, ctx),
            _ => {}
        }
    }

    pub(crate) fn emit_call_domain(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
        match expr {
            Expression::CallExpression(call) => self.emit_call_expression(call, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }
}
