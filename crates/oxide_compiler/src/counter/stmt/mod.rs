use super::*;

impl Compiler {
    pub(crate) fn count_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => {
                self.count_expression(&es.expression, ctx);
            }
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    if let Some(init) = &d.init {
                        self.count_expression(init, ctx);
                        self.count_binding_pattern(&d.id, ctx);
                    } else {
                        ctx.alloc_reg();
                        ctx.count_words(2); // LOAD_CONST(undefined) + STORE_VAR
                    }
                }
            }
            Statement::ReturnStatement(ret) => {
                if let Some(arg) = &ret.argument {
                    self.count_expression(arg, ctx);
                }
                ctx.projected_pc += 1; // RETURN
            }
            Statement::IfStatement(ifs) => {
                let id = ctx.next_label_id();
                let else_label = Label::IfElse(id);
                let end_label = Label::IfEnd(id);

                self.count_expression(&ifs.test, ctx);
                ctx.projected_pc += 1; // JMP_IF_FALSE
                self.count_statement(&ifs.consequent, ctx);
                ctx.alloc_reg(); // result register
                ctx.projected_pc += 1; // LOAD_VAR result <- consequent
                if ifs.alternate.is_some() {
                    ctx.projected_pc += 1; // JMP (skip else)
                }
                ctx.label_map.insert(else_label, ctx.projected_pc);
                if let Some(alt_stmt) = &ifs.alternate {
                    self.count_statement(alt_stmt, ctx);
                    ctx.projected_pc += 1; // LOAD_VAR result <- alternate
                }
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Statement::WhileStatement(wh) => {
                let id = ctx.next_label_id();
                let start_label = Label::WhileStart(id);
                let end_label = Label::WhileEnd(id);

                ctx.label_map.insert(start_label, ctx.projected_pc);
                self.count_expression(&wh.test, ctx);
                ctx.projected_pc += 1; // JMP_IF_FALSE
                self.count_statement(&wh.body, ctx);
                ctx.projected_pc += 1; // JMP (backward)
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Statement::DoWhileStatement(dw) => {
                let id = ctx.next_label_id();
                let start_label = Label::DoWhileStart(id);
                let end_label = Label::DoWhileEnd(id);

                ctx.label_map.insert(start_label, ctx.projected_pc);
                self.count_statement(&dw.body, ctx);
                self.count_expression(&dw.test, ctx);
                ctx.projected_pc += 1; // JMP_IF_TRUE (backward)
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Statement::ForStatement(fr) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForStart(id);
                let update_label = Label::ForUpdate(id);
                let end_label = Label::ForEnd(id);

                if let Some(init) = &fr.init {
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
                ctx.label_map.insert(start_label, ctx.projected_pc);
                if let Some(test) = &fr.test {
                    self.count_expression(test, ctx);
                    ctx.projected_pc += 1; // JMP_IF_FALSE
                }
                self.count_statement(&fr.body, ctx);
                ctx.label_map.insert(update_label, ctx.projected_pc);
                if let Some(update) = &fr.update {
                    self.count_expression(update, ctx);
                }
                ctx.projected_pc += 1; // JMP (backward)
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Statement::ForInStatement(fi) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForInStart(id);
                let end_label = Label::ForInEnd(id);

                self.count_expression(&fi.right, ctx);
                ctx.count_instr(); // FOR_IN_INIT

                ctx.label_map.insert(start_label, ctx.projected_pc);
                ctx.count_instr(); // FOR_IN_DONE
                ctx.count_jump(); // JMP_IF_FALSE
                ctx.count_jump(); // JMP cleanup

                ctx.count_instr(); // FOR_IN_NEXT
                match &fi.left {
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

                self.count_statement(&fi.body, ctx);
                ctx.count_jump(); // JMP back to start

                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.count_instr(); // FOR_IN_CLEANUP
            }
            Statement::ForOfStatement(fo) => {
                let id = ctx.next_label_id();
                let start_label = Label::ForOfStart(id);
                let end_label = Label::ForOfEnd(id);

                self.count_expression(&fo.right, ctx);
                ctx.count_instr(); // FOR_OF_INIT
                ctx.label_map.insert(start_label, ctx.projected_pc);
                ctx.alloc_reg(); // has_reg
                ctx.count_instr(); // FOR_OF_DONE
                ctx.count_jump(); // JMP_IF_FALSE
                ctx.alloc_reg(); // val_reg
                ctx.count_instr(); // FOR_OF_NEXT
                match &fo.left {
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
                self.count_statement(&fo.body, ctx);
                ctx.count_jump(); // JMP back
                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.count_instr(); // FOR_OF_CLOSE
            }
            Statement::SwitchStatement(sw) => {
                let id = ctx.next_label_id();
                let end_label = Label::SwitchEnd(id);
                ctx.push_switch(end_label);

                self.count_expression(&sw.discriminant, ctx);
                let compare_reg_checkpoint = ctx.reg_checkpoint();

                let cases = &sw.cases;
                for case in cases.iter() {
                    if let Some(test) = &case.test {
                        self.count_expression(test, ctx);
                        ctx.projected_pc += 1; // EQ
                        ctx.alloc_reg(); // eq result
                        ctx.projected_pc += 1; // JMP_IF_TRUE
                        ctx.restore_reg_checkpoint(compare_reg_checkpoint);
                    }
                }

                let has_default = cases.iter().any(|c| c.test.is_none());
                if !has_default {
                    ctx.projected_pc += 1; // JMP to SwitchEnd (no match)
                }

                for (case_idx, case) in cases.iter().enumerate() {
                    let case_label = Label::SwitchCase(id, case_idx as u32);
                    ctx.label_map.insert(case_label, ctx.projected_pc);
                    for s in &case.consequent {
                        self.count_statement(s, ctx);
                    }
                }

                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.pop_switch();
            }
            Statement::BreakStatement(_) => {
                ctx.projected_pc += 1; // JMP
            }
            Statement::ContinueStatement(_) => {
                ctx.projected_pc += 1; // JMP
            }
            Statement::BlockStatement(block) => {
                for s in &block.body {
                    self.count_statement(s, ctx);
                }
            }
            Statement::FunctionDeclaration(fd) => {
                let name = if let Some(id) = &fd.id {
                    id.name.to_string()
                } else {
                    return;
                };

                // Hoisting: declare function name as initialized
                let func_reg = ctx.alloc_reg();
                let _ = ctx.declare_initialized(&name, func_reg, VariableDeclarationKind::Var, false);

                // Body is compiled in the emit pass only.
                // FD emits LOAD_CONST(BytecodeFunc) + STORE_VAR
                ctx.count_words(2);
            }
            Statement::ClassDeclaration(class) => {
                ctx.alloc_reg(); // class binding reg
                self.count_class(class, ctx);
                ctx.projected_pc += 1; // STORE_VAR binding <- ctor
            }
            Statement::ThrowStatement(ts) => {
                self.count_expression(&ts.argument, ctx);
                ctx.projected_pc += 1; // THROW
            }
            Statement::TryStatement(ts) => {
                let id = ctx.next_label_id();
                let catch_label = Label::CatchBody(id);
                let try_end_label = Label::TryEnd(id);
                let has_catch = ts.handler.is_some();
                let has_finally = ts.finalizer.is_some();

                ctx.alloc_reg(); // result_reg

                if has_finally {
                    ctx.projected_pc += 1; // TRY_FINALLY_BEGIN (before try body)
                }

                if has_catch {
                    ctx.projected_pc += 1; // TRY_BEGIN (before try body)
                }

                for s in &ts.block.body {
                    self.count_statement(s, ctx);
                }
                ctx.projected_pc += 1; // LOAD_VAR result_reg (if try body has result)

                if has_catch {
                    ctx.projected_pc += 1; // TRY_END
                }

                let jmp_needed = has_catch || has_finally;
                if jmp_needed {
                    ctx.projected_pc += 1; // JMP
                }

                ctx.label_map.insert(catch_label, ctx.projected_pc);
                if let Some(catch) = &ts.handler {
                    ctx.push_scope();
                    if let Some(_param) = &catch.param {
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // STORE_VAR
                    }
                    for cs in &catch.body.body {
                        self.count_statement(cs, ctx);
                    }
                    ctx.projected_pc += 1; // LOAD_VAR result_reg (if catch body has result)
                    ctx.pop_scope();
                }

                if let Some(finally) = &ts.finalizer {
                    let finally_label = Label::FinallyBody(id);
                    ctx.label_map.insert(finally_label, ctx.projected_pc);
                    for fs in &finally.body {
                        self.count_statement(fs, ctx);
                    }
                    ctx.projected_pc += 1; // LOAD_VAR result_reg (if finally has result)
                    ctx.projected_pc += 1; // TRY_FINALLY_END
                }

                ctx.label_map.insert(try_end_label, ctx.projected_pc);
            }
            _ => {}
        }
    }
}
