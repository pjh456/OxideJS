use oxide_parser::{Expression, Statement};

use crate::compiler::{CompileCtx, Compiler};

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
                    }
                }
            }
            Statement::ReturnStatement(ret) => {
                if let Some(arg) = &ret.argument {
                    self.count_expression(arg, ctx);
                }
            }
            Statement::IfStatement(ifs) => {
                self.count_expression(&ifs.test, ctx);
                self.count_statement(&ifs.consequent, ctx);
                if let Some(alt) = &ifs.alternate {
                    self.count_statement(alt, ctx);
                }
            }
            Statement::WhileStatement(wh) => {
                self.count_expression(&wh.test, ctx);
                self.count_statement(&wh.body, ctx);
            }
            Statement::ForStatement(fr) => {
                if let Some(init) = &fr.init {
                    if let Some(expr) = init.as_expression() {
                        self.count_expression(expr, ctx);
                    }
                }
                if let Some(test) = &fr.test {
                    self.count_expression(test, ctx);
                }
                if let Some(update) = &fr.update {
                    self.count_expression(update, ctx);
                }
                self.count_statement(&fr.body, ctx);
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
            }
            Expression::UnaryExpression(un) => {
                self.count_expression(&un.argument, ctx);
                ctx.alloc_reg();
            }
            Expression::CallExpression(call) => {
                self.count_expression(&call.callee, ctx);
                ctx.alloc_reg();
            }
            Expression::AssignmentExpression(assign) => {
                if let oxide_parser::AssignmentTarget::StaticMemberExpression(member) = &assign.left
                {
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                } else if let oxide_parser::AssignmentTarget::ComputedMemberExpression(member) =
                    &assign.left
                {
                    self.count_expression(&member.object, ctx);
                    self.count_expression(&member.expression, ctx);
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                } else {
                    self.count_expression(&assign.right, ctx);
                    ctx.alloc_reg();
                }
            }
            Expression::ConditionalExpression(cond) => {
                self.count_expression(&cond.test, ctx);
                self.count_expression(&cond.consequent, ctx);
                self.count_expression(&cond.alternate, ctx);
                ctx.alloc_reg();
            }
            Expression::SequenceExpression(seq) => {
                for e in &seq.expressions {
                    self.count_expression(e, ctx);
                }
            }
            Expression::LogicalExpression(log) => {
                self.count_expression(&log.left, ctx);
                self.count_expression(&log.right, ctx);
                ctx.alloc_reg();
            }
            Expression::ObjectExpression(obj) => {
                ctx.alloc_reg();
                for prop in &obj.properties {
                    if let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop {
                        ctx.alloc_reg();
                        self.count_expression(&p.value, ctx);
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                ctx.alloc_reg();
                for elem in &arr.elements {
                    if let Some(e) = elem.as_expression() {
                        self.count_expression(e, ctx);
                        ctx.alloc_reg();
                    }
                }
            }
            Expression::StaticMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                ctx.alloc_reg();
                ctx.alloc_reg();
            }
            Expression::ComputedMemberExpression(member) => {
                self.count_expression(&member.object, ctx);
                self.count_expression(&member.expression, ctx);
                ctx.alloc_reg();
            }
            Expression::ParenthesizedExpression(p) => {
                self.count_expression(&p.expression, ctx);
            }
            _ => {
                ctx.alloc_reg();
            }
        }
    }
}
