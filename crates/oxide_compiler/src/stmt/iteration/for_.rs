use crate::compiler::{CompileCtx, Compiler, Label};
use oxide_bytecode::{
    module::Constant,
    opcode::{self, OpCode},
};
use oxide_parser::{BindingPattern, ForStatementInit, Statement, VariableDeclarationKind};

impl Compiler {
    pub(crate) fn count_for_statement(&self, stmt: &oxide_parser::ForStatement<'_>, ctx: &mut CompileCtx) {
        let id = ctx.next_label_id();
        let start_label = Label::ForStart(id);
        let update_label = Label::ForUpdate(id);
        let end_label = Label::ForEnd(id);
        if let Some(init) = &stmt.init {
            if let Some(expr) = init.as_expression() {
                self.count_expression(expr, ctx);
            } else if let ForStatementInit::VariableDeclaration(decl) = init {
                for d in &decl.declarations {
                    if let Some(init_expr) = &d.init {
                        self.count_expression(init_expr, ctx);
                        self.count_binding_pattern(&d.id, ctx);
                    } else {
                        ctx.alloc_reg();
                        ctx.alloc_reg();
                        ctx.count_words(2);
                    }
                }
            }
        }
        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        if let Some(test) = &stmt.test {
            self.count_expression(test, ctx);
            ctx.projected_pc += 1;
        }
        self.count_statement(&stmt.body, ctx);
        ctx.labels.label_map.insert(update_label, ctx.projected_pc);
        if let Some(update) = &stmt.update {
            self.count_expression(update, ctx);
        }
        ctx.projected_pc += 1;
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }

    pub(crate) fn emit_for_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
        let Statement::ForStatement(fr) = stmt else {
            return Ok(None);
        };
        let id = ctx.next_label_id();
        let start_label = Label::ForStart(id);
        let update_label = Label::ForUpdate(id);
        let end_label = Label::ForEnd(id);
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
                        ctx.emit_load_const(tmp, idx);
                        let var_reg = ctx.alloc_reg();
                        ctx.declare(bi.name.as_str(), var_reg, decl.kind, is_const)?;
                        ctx.emit(opcode::encode(OpCode::STORE_VAR, var_reg, tmp, 0));
                        ctx.init_var(bi.name.as_str());
                    }
                }
            }
        }
        ctx.labels.label_map.insert(start_label, ctx.bytecode.len());
        if let Some(test) = &fr.test {
            let test_reg = self.emit_expression(test, ctx)?;
            ctx.emit_jmp_if_false_labeled(test_reg, end_label);
        }
        self.emit_statement(&fr.body, ctx)?;
        ctx.labels.label_map.insert(update_label, ctx.bytecode.len());
        if let Some(update) = &fr.update {
            self.emit_expression(update, ctx)?;
        }
        ctx.emit_jmp_labeled(start_label);
        ctx.labels.label_map.insert(end_label, ctx.bytecode.len());
        ctx.pop_label_scopes(n_labeled);
        ctx.pop_loop();
        Ok(None)
    }
}
