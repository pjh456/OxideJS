use super::*;

impl Compiler {
    pub(in crate::counter) fn count_while_statement(
        &self, stmt: &oxide_parser::WhileStatement<'_>, ctx: &mut CompileCtx,
    ) {
        let id = ctx.next_label_id();
        let start_label = Label::WhileStart(id);
        let end_label = Label::WhileEnd(id);

        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        self.count_expression(&stmt.test, ctx);
        ctx.projected_pc += 1; // JMP_IF_FALSE
        self.count_statement(&stmt.body, ctx);
        ctx.projected_pc += 1; // JMP (backward)
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }

    pub(in crate::counter) fn count_do_while_statement(
        &self, stmt: &oxide_parser::DoWhileStatement<'_>, ctx: &mut CompileCtx,
    ) {
        let id = ctx.next_label_id();
        let start_label = Label::DoWhileStart(id);
        let end_label = Label::DoWhileEnd(id);

        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        self.count_statement(&stmt.body, ctx);
        self.count_expression(&stmt.test, ctx);
        ctx.projected_pc += 1; // JMP_IF_TRUE (backward)
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }

    pub(in crate::counter) fn count_for_statement(&self, stmt: &oxide_parser::ForStatement<'_>, ctx: &mut CompileCtx) {
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
                        ctx.count_words(2); // LOAD_CONST(undefined) + STORE_VAR
                    }
                }
            }
        }
        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        if let Some(test) = &stmt.test {
            self.count_expression(test, ctx);
            ctx.projected_pc += 1; // JMP_IF_FALSE
        }
        self.count_statement(&stmt.body, ctx);
        ctx.labels.label_map.insert(update_label, ctx.projected_pc);
        if let Some(update) = &stmt.update {
            self.count_expression(update, ctx);
        }
        ctx.projected_pc += 1; // JMP (backward)
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
    }

    pub(in crate::counter) fn count_for_in_statement(
        &self, stmt: &oxide_parser::ForInStatement<'_>, ctx: &mut CompileCtx,
    ) {
        let id = ctx.next_label_id();
        let start_label = Label::ForInStart(id);
        let end_label = Label::ForInEnd(id);

        self.count_expression(&stmt.right, ctx);
        ctx.count_instr(); // FOR_IN_INIT

        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        ctx.count_instr(); // FOR_IN_DONE
        ctx.count_jump(); // JMP_IF_FALSE
        ctx.count_jump(); // JMP cleanup

        ctx.count_instr(); // FOR_IN_NEXT
        match &stmt.left {
            oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                for _d in &decl.declarations {
                    ctx.alloc_reg();
                    ctx.count_instr(); // STORE_VAR
                }
            }
            oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg(); // key register
                ctx.count_instr(); // STORE_VAR
            }
            _ => {}
        }

        self.count_statement(&stmt.body, ctx);
        ctx.count_jump(); // JMP back to start

        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
        ctx.count_instr(); // FOR_IN_CLEANUP
    }

    pub(in crate::counter) fn count_for_of_statement(
        &self, stmt: &oxide_parser::ForOfStatement<'_>, ctx: &mut CompileCtx,
    ) {
        let id = ctx.next_label_id();
        let start_label = Label::ForOfStart(id);
        let end_label = Label::ForOfEnd(id);

        self.count_expression(&stmt.right, ctx);
        ctx.count_instr(); // FOR_OF_INIT
        ctx.labels.label_map.insert(start_label, ctx.projected_pc);
        ctx.alloc_reg(); // has_reg
        ctx.count_instr(); // FOR_OF_DONE
        ctx.count_jump(); // JMP_IF_FALSE
        ctx.alloc_reg(); // val_reg
        ctx.count_instr(); // FOR_OF_NEXT
        match &stmt.left {
            oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    self.count_binding_pattern(&d.id, ctx);
                }
            }
            oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                ctx.alloc_reg();
                ctx.count_instr(); // STORE_VAR
            }
            oxide_parser::ForStatementLeft::ArrayAssignmentTarget(ap) => {
                self.count_array_assignment(ap, ctx);
            }
            oxide_parser::ForStatementLeft::ObjectAssignmentTarget(op) => {
                self.count_object_assignment(op, ctx);
            }
            _ => {}
        }
        self.count_statement(&stmt.body, ctx);
        ctx.count_jump(); // JMP back
        ctx.labels.label_map.insert(end_label, ctx.projected_pc);
        ctx.count_instr(); // FOR_OF_CLOSE
    }
}
