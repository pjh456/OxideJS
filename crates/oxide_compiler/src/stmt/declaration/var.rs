use crate::compiler::{CompileCtx, Compiler};
use crate::ir::inst::Inst;
use crate::ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{BindingPattern, Expression, Statement, VariableDeclarationKind};

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
                        if let Some(sub_mod) = ctx.nested.last_mut() {
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
                ctx.inst(Inst::load_const(Operand::Reg(tmp as u32), idx));
                let var_reg = ctx.alloc_reg();
                let target_reg = if matches!(decl.kind, VariableDeclarationKind::Var) {
                    match ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const) {
                        Ok(()) => var_reg,
                        Err(_) => ctx.lookup(bi.name.as_str()).unwrap_or(var_reg),
                    }
                } else {
                    ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const)?;
                    var_reg
                };
                let is_captured = ctx.scopes.symbols.lookup_is_captured(bi.name.as_str());
                if is_captured {
                    let cell_idx = ctx.scopes.cell_registry.len() as u8;
                    ctx.scopes.cell_registry.push((bi.name.to_string(), cell_idx));
                    ctx.inst(Inst::new(OpCode::MAKE_CELL, Operand::Reg(tmp as u32), Operand::Reg(cell_idx as u32), Operand::None));
                } else {
                    ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(target_reg as u32), Operand::Reg(tmp as u32), Operand::None));
                }
                ctx.init_var(bi.name.as_str());
                r = Some(var_reg);
            }
        }
        Ok(r)
    }
}
