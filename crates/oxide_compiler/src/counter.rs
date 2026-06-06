use oxide_parser::{Expression, ForStatementInit, Statement};

use crate::compiler::{is_side_effect_free, CompileCtx, Compiler, Label};

impl Compiler {
    pub(crate) fn count_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => {
                self.count_expression(&es.expression, ctx);
            }
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    ctx.alloc_reg();
                    if let Some(init) = &d.init {
                        self.count_expression(init, ctx);
                        ctx.projected_pc += 1; // STORE_VAR
                    } else {
                        ctx.projected_pc += 2; // LOAD_CONST(undefined) + STORE_VAR
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
                            ctx.alloc_reg();
                            if let Some(init_expr) = &d.init {
                                self.count_expression(init_expr, ctx);
                                ctx.projected_pc += 1; // STORE_VAR
                            } else {
                                ctx.projected_pc += 2; // LOAD_CONST(undefined) + STORE_VAR
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
                ctx.projected_pc += 1; // FOR_IN_INIT

                ctx.label_map.insert(start_label, ctx.projected_pc);
                ctx.projected_pc += 3; // FOR_IN_DONE + JMP_IF_FALSE + JMP cleanup

                ctx.projected_pc += 1; // FOR_IN_NEXT
                match &fi.left {
                    oxide_parser::ForStatementLeft::VariableDeclaration(decl) => {
                        for _d in &decl.declarations {
                            ctx.alloc_reg();
                            ctx.projected_pc += 1; // STORE_VAR (value from FOR_IN_NEXT)
                        }
                    }
                    oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(_) => {
                        ctx.alloc_reg(); // key register
                        ctx.projected_pc += 1; // STORE_VAR
                    }
                    _ => {}
                }

                self.count_statement(&fi.body, ctx);
                ctx.projected_pc += 1; // JMP back to start

                ctx.label_map.insert(end_label, ctx.projected_pc);
                ctx.projected_pc += 1; // FOR_IN_CLEANUP
            }
            Statement::SwitchStatement(sw) => {
                let id = ctx.next_label_id();
                let end_label = Label::SwitchEnd(id);
                ctx.push_switch(end_label);

                self.count_expression(&sw.discriminant, ctx);

                let cases = &sw.cases;
                for case in cases.iter() {
                    if let Some(test) = &case.test {
                        self.count_expression(test, ctx);
                        ctx.projected_pc += 1; // EQ
                        ctx.alloc_reg(); // eq result
                        ctx.projected_pc += 1; // JMP_IF_TRUE
                    }
                }

                let has_default = cases.iter().any(|c| c.test.is_none());
                if !has_default {
                    ctx.projected_pc += 1; // JMP to SwitchEnd (no match)
                }

                for (case_idx, case) in cases.iter().enumerate() {
                    let case_label = Label::SwitchCase(id * 256 + case_idx as u32);
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
            _ => {}
        }
    }

    pub(crate) fn count_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::BinaryExpression(bin) => {
                self.count_expression(&bin.left, ctx);
                self.count_expression(&bin.right, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // ADD/SUB/MUL/DIV/etc.
            }
            Expression::UnaryExpression(un) => {
                self.count_expression(&un.argument, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // NEG/TYPEOF/VOID/NOT
            }
            Expression::CallExpression(call) => {
                self.count_expression(&call.callee, ctx);
                for arg in &call.arguments {
                    if let Some(expr) = arg.as_expression() {
                        self.count_expression(expr, ctx);
                    }
                }
                ctx.alloc_reg();
                ctx.projected_pc += 1; // CALL
            }
            Expression::AssignmentExpression(assign) => {
                if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left
                {
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // IC_SET_PROP
                } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) =
                    &assign.left
                {
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&member.expression, ctx);
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // SET_PROP_DYNAMIC
                } else {
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1;
                }
            }
            Expression::ConditionalExpression(cond) => {
                let id = ctx.next_label_id();
                let else_label = Label::TernaryElse(id);
                let end_label = Label::TernaryEnd(id);

                self.count_expression(&cond.test, ctx);
                ctx.projected_pc += 1; // JMP_IF_FALSE
                self.count_expression(&cond.consequent, ctx);
                ctx.alloc_reg(); // result register
                ctx.projected_pc += 1; // LOAD_VAR result <- consequent
                ctx.projected_pc += 1; // JMP to end
                ctx.label_map.insert(else_label, ctx.projected_pc);
                self.count_expression(&cond.alternate, ctx);
                ctx.projected_pc += 1; // LOAD_VAR result <- alternate
                ctx.label_map.insert(end_label, ctx.projected_pc);
            }
            Expression::SequenceExpression(seq) => {
                for e in &seq.expressions {
                    self.count_expression(e, ctx);
                }
            }
            Expression::LogicalExpression(log) => {
                self.count_expression(&log.left, ctx);

                let is_simple = is_side_effect_free(&log.left) && is_side_effect_free(&log.right);

                if is_simple {
                    self.count_expression(&log.right, ctx);
                    ctx.alloc_reg();
                    ctx.projected_pc += 1; // AND/OR
                } else {
                    use oxide_parser::LogicalOperator;
                    let id = ctx.next_label_id();
                    ctx.alloc_reg(); // dup register
                    ctx.projected_pc += 1; // LOAD_VAR (DUP)
                    ctx.projected_pc += 1; // JMP_IF_FALSE or JMP_IF_TRUE
                    self.count_expression(&log.right, ctx);
                    ctx.projected_pc += 1; // LOAD_VAR (overwrite)
                    let skip_label = match log.operator {
                        LogicalOperator::And => Label::TernaryEnd(id),
                        LogicalOperator::Or => Label::TernaryElse(id),
                        LogicalOperator::Coalesce => {
                            unreachable!()
                        }
                    };
                    ctx.label_map.insert(skip_label, ctx.projected_pc);
                }
            }
            Expression::ObjectExpression(obj) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // NEW_OBJECT
                for prop in &obj.properties {
                    if let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop {
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // LOAD_CONST key
                        self.count_expression(&p.value, ctx);
                        ctx.projected_pc += 1; // SET_PROP
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // NEW_ARRAY
                for elem in &arr.elements {
                    if let Some(e) = elem.as_expression() {
                        self.count_expression(e, ctx);
                        ctx.alloc_reg();
                        ctx.projected_pc += 1; // LOAD_CONST index
                        ctx.projected_pc += 1; // SET_ELEM
                    }
                }
            }
            Expression::StaticMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST key
                ctx.projected_pc += 1; // IC_GET_PROP
                ctx.projected_pc += 1; // IC ext word
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                self.count_expression(&member.expression, ctx);
                ctx.alloc_reg();
                ctx.projected_pc += 1; // GET_PROP_DYNAMIC
            }
            Expression::ParenthesizedExpression(p) => {
                self.count_expression(&p.expression, ctx);
            }
            _ => {
                ctx.alloc_reg();
                ctx.projected_pc += 1; // LOAD_CONST or LOAD_VAR
            }
        }
    }
}
