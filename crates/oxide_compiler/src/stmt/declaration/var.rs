use crate::compiler::{CompileCtx, Compiler};
use oxide_bytecode::{
    module::Constant,
    opcode::{self, OpCode},
};
use oxide_parser::{BindingPattern, Expression, Statement, VariableDeclarationKind};

impl Compiler {
    pub(crate) fn count_variable_declaration(
        &self, decl: &oxide_parser::VariableDeclaration<'_>, ctx: &mut CompileCtx,
    ) {
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
}
