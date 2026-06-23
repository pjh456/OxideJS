use super::*;

impl Compiler {
    pub(in crate::emitter) fn emit_arrow_function_expression(
        &self, arrow: &oxide_parser::ArrowFunctionExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        // Rest params not yet supported (D-06 placeholder)
        if let Some(_rest) = &arrow.params.rest {
            return Err("rest params in arrow functions not yet supported".into());
        }

        // Extract param names (same pattern as FunctionExpression)
        let mut param_names = Vec::new();
        for (idx, param) in arrow.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                    param_names.push(ParamSpec::Identifier(bi.name.to_string()));
                }
                pattern => {
                    param_names.push(ParamSpec::Pattern {
                        synthetic_name: format!("@@param_{idx}"),
                        pattern,
                    });
                }
            }
        }

        // Expression body: pass body statements directly with is_expression_body=true.
        // Statement body: pass body statements with is_expression_body=false.
        let body_stmts = &arrow.body.statements;
        let is_expr_body = arrow.expression;

        let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, is_expr_body, true)?;
        sub_module.is_arrow = true;

        ctx.sub_modules.push(sub_module);
        // 1-indexed: 0 = no sub_module (sentinel)
        let sub_idx = ctx.sub_modules.len() as u32;

        let const_idx = ctx.add_constant(Constant::BytecodeFunc(sub_idx));
        let r = ctx.alloc_reg();
        ctx.emit_load_const(r, const_idx);
        Ok(r)
    }

    pub(in crate::emitter) fn emit_function_expression(
        &self, fe: &oxide_parser::Function, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        // FunctionExpression: compile body, emit LOAD_CONST(BytecodeFunc)
        let mut param_names = Vec::new();
        for (idx, param) in fe.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                    param_names.push(ParamSpec::Identifier(bi.name.to_string()));
                }
                pattern => {
                    param_names.push(ParamSpec::Pattern {
                        synthetic_name: format!("@@param_{idx}"),
                        pattern,
                    });
                }
            }
        }

        let body_stmts: &[Statement] = if let Some(body) = &fe.body { &body.statements } else { &[] };

        let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, false, false)?;
        if let Some(id) = &fe.id {
            sub_module.function_name = Some(id.name.to_string());
        }
        ctx.sub_modules.push(sub_module);
        // 1-indexed: 0 = no sub_module (sentinel)
        let sub_idx = ctx.sub_modules.len() as u32;

        let const_idx = ctx.add_constant(Constant::BytecodeFunc(sub_idx));
        let r = ctx.alloc_reg();
        ctx.emit_load_const(r, const_idx);
        Ok(r)
    }

    pub(in crate::emitter) fn emit_class_expression(&self, class: &Class, ctx: &mut CompileCtx) -> Result<u8, String> {
        self.emit_class(class, ctx)
    }

    pub(in crate::emitter) fn emit_new_expression(
        &self, ne: &oxide_parser::NewExpression, ctx: &mut CompileCtx,
    ) -> Result<u8, String> {
        let constructor_reg = self.emit_expression(&ne.callee, ctx)?;
        let mut arg_regs = Vec::new();
        for arg in &ne.arguments {
            if let Some(expr) = arg.as_expression() {
                arg_regs.push(self.emit_expression(expr, ctx)?);
            }
        }
        let first_arg_reg = if arg_regs.is_empty() { 0u8 } else { arg_regs[0] };
        let r = ctx.alloc_reg();
        ctx.emit(opcode::encode(OpCode::NEW_EXPRESSION, r, constructor_reg, first_arg_reg));
        ctx.emit(arg_regs.len() as u32);
        Ok(r)
    }
}
