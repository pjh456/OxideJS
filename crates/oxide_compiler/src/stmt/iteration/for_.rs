use crate::compiler::{CompileCtx, Compiler};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_parser::{BindingPattern, ForStatementInit, Statement, VariableDeclarationKind};

impl Compiler {
    pub(crate) fn emit_for_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ForStatement(fr) = stmt else {
            return Ok(None);
        };
        let start_label = ctx.next_label_id();
        let update_label = ctx.next_label_id();
        let end_label = ctx.next_label_id();
        ctx.push_loop(end_label, update_label);
        let n_labeled = ctx.take_pending_loop_labels(end_label, update_label);
        if let Some(init) = &fr.init {
            if let Some(expr) = init.as_expression() {
                self.emit_expression(expr, ctx)?;
            } else if let ForStatementInit::VariableDeclaration(decl) = init {
                for d in &decl.declarations {
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    if let Some(init_expr) = &d.init {
                        let val_reg = self.emit_expression(init_expr, ctx)?;
                        self.emit_binding_pattern(&d.id, val_reg, decl.kind, is_const, ctx)?;
                    } else if let BindingPattern::BindingIdentifier(bi) = &d.id {
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
                        ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(target_reg as u32), Operand::Reg(tmp as u32), Operand::None));
                        ctx.init_var(bi.name.as_str());
                    }
                }
            }
        }
        ctx.labels.set_label_pos(start_label, ctx.insts.len());
        if let Some(test) = &fr.test {
            let test_reg = self.emit_expression(test, ctx)?;
            ctx.inst(Inst::jmp_if_false(test_reg, end_label));
        }
        self.emit_statement(&fr.body, ctx)?;
        ctx.labels.set_label_pos(update_label, ctx.insts.len());
        if let Some(update) = &fr.update {
            self.emit_expression(update, ctx)?;
        }
        ctx.inst(Inst::jmp(start_label));
        ctx.labels.set_label_pos(end_label, ctx.insts.len());
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }
}
