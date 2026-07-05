use super::super::*;

impl Compiler {
    fn count_variable_declaration(&self, decl: &oxide_parser::VariableDeclaration<'_>, ctx: &mut CompileCtx) {
        let is_var = matches!(decl.kind, VariableDeclarationKind::Var);
        for d in &decl.declarations {
            if let Some(init) = &d.init {
                self.count_expression(init, ctx);
                if is_var {
                    if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                        let reg = ctx.alloc_reg();
                        let _ = ctx.declare_initialized(bi.name.as_str(), reg, VariableDeclarationKind::Var, false);
                        ctx.projected_pc += 1;
                        continue;
                    }
                }
                self.count_binding_pattern(&d.id, ctx);
            } else {
                ctx.alloc_reg();
                ctx.count_words(2);
            }
        }
    }

    fn count_function_declaration(&self, decl: &oxide_parser::Function<'_>, ctx: &mut CompileCtx) {
        let name = if let Some(id) = &decl.id {
            id.name.to_string()
        } else {
            return;
        };
        let func_reg = ctx.alloc_reg();
        let _ = ctx.declare_initialized(&name, func_reg, VariableDeclarationKind::Var, false);
        ctx.count_words(2);
    }

    fn count_class_declaration(&self, class: &oxide_parser::Class<'_>, ctx: &mut CompileCtx) {
        ctx.alloc_reg();
        self.count_class(class, ctx);
        ctx.projected_pc += 1;
    }

    pub(crate) fn count_declaration_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::VariableDeclaration(decl) => self.count_variable_declaration(decl, ctx),
            Statement::FunctionDeclaration(fd) => self.count_function_declaration(fd, ctx),
            Statement::ClassDeclaration(class) => self.count_class_declaration(class, ctx),
            _ => {}
        }
    }
}

impl Compiler {
    pub(crate) fn emit_variable_declaration_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::VariableDeclaration(decl) = stmt else {
            return Ok(None);
        };
        let mut r = None;
        for d in &decl.declarations {
            let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
            if is_const && d.init.is_none() {
                return Err("const declaration must have an initializer".into());
            }
            if let Some(init) = &d.init {
                let val_reg = self.emit_expression(init, ctx)?;
                self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, ctx)?;
                if let BindingPattern::BindingIdentifier(bi) = &d.id {
                    if matches!(*init, Expression::ArrowFunctionExpression(_)) {
                        if let Some(sub_mod) = ctx.sub_modules.last_mut() {
                            sub_mod.function_name = Some(bi.name.to_string());
                        }
                    }
                }
                r = Some(val_reg);
            } else {
                let BindingPattern::BindingIdentifier(bi) = &d.id else {
                    return Err("destructuring declaration requires an initializer".into());
                };
                let idx = ctx.add_constant(Constant::Undefined);
                let tmp = ctx.alloc_reg();
                ctx.emit_load_const(tmp, idx);
                let var_reg = ctx.alloc_reg();
                ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const)?;
                let is_captured = ctx.scopes.symbols.lookup_is_captured(bi.name.as_str());
                if is_captured {
                    let cell_idx = ctx.scopes.cell_registry.len() as u8;
                    ctx.scopes.cell_registry.push((bi.name.to_string(), cell_idx));
                    ctx.emit(opcode::encode(OpCode::MAKE_CELL, tmp, cell_idx, 0));
                } else {
                    ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, tmp, 0));
                }
                ctx.init_var(bi.name.as_str());
                r = Some(var_reg);
            }
        }
        Ok(r)
    }

    pub(crate) fn emit_function_declaration_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::FunctionDeclaration(fd) = stmt else {
            return Err("FunctionDeclaration without name".into());
        };
        let name = if let Some(id) = &fd.id {
            id.name.to_string()
        } else {
            return Err("FunctionDeclaration without name".into());
        };
        let mut param_names = Vec::new();
        for (idx, param) in fd.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                    param_names.push(ParamSpec::Identifier(bi.name.to_string()))
                }
                pattern => param_names.push(ParamSpec::Pattern {
                    synthetic_name: format!("@@param_{idx}"),
                    pattern,
                }),
            }
        }
        let body_stmts: &[Statement] = if let Some(body) = &fd.body { &body.statements } else { &[] };
        let mut sub_module = self.compile_function_body(&param_names, body_stmts, ctx, false, false)?;
        sub_module.function_name = Some(name.clone());
        ctx.sub_modules.push(sub_module);
        let sub_idx = ctx.sub_modules.len() as u32;
        let var_reg = ctx.lookup(&name)?;
        ctx.reserve_reg(var_reg);
        ctx.emit_create_closure(var_reg, sub_idx);
        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, var_reg, 0));
        Ok(None)
    }

    pub(crate) fn emit_class_declaration_statement(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u8>, String> {
        let Statement::ClassDeclaration(class) = stmt else {
            return Err("ClassDeclaration without name".into());
        };
        let name = class
            .id
            .as_ref()
            .map(|id| id.name.to_string())
            .ok_or_else(|| "ClassDeclaration without name".to_string())?;
        let var_reg = ctx.alloc_reg();
        ctx.declare(&name, var_reg, VariableDeclarationKind::Let, false)?;
        ctx.init_var(&name);
        let ctor_reg = self.emit_class(class, ctx)?;
        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, ctor_reg, 0));
        Ok(None)
    }

    pub(crate) fn emit_declaration_domain(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        match stmt {
            Statement::VariableDeclaration(_) => self.emit_variable_declaration_statement(stmt, ctx),
            Statement::FunctionDeclaration(_) => self.emit_function_declaration_statement(stmt, ctx),
            Statement::ClassDeclaration(_) => self.emit_class_declaration_statement(stmt, ctx),
            _ => Ok(None),
        }
    }
}
