//! emit 前置 pass，builtin 引用预扫描：临时寄存器池分配前遍历 AST，把内置
//! 全局标识符出现处（语句/表达式/赋值目标/链式成员/实参/绑定 pattern 计算
//! 键）预先登记到固定寄存器槽（builtin_reg_map），避免与临时寄存器池冲突。
//!
//! 排除集：当前语句列表直接子级的词法声明名（let/const/class）。这些名在本作用域
//! 是词法绑定并遮蔽同名全局属性，镜像槽不得为其登记——否则镜像占位先于词法预
//! 声明占 scope 0，词法声明撞名被拒（规范合法遮蔽形被编译期误拒），写臂谓词亦
//! 误把词法槽解析成全局内置槽。

use std::collections::HashSet;

use crate::{CompileCtx, Emitter};
use oxide_parser::{BindingPattern, Declaration, Expression, Statement};

impl Emitter {
    /// 递归遍历程序 AST，把内置全局标识符引用预登记到固定寄存器槽（内置槽在临时
    /// 寄存器池之前分配，两者隔离，槽位分配序稳定）。`excluded` 内的名（本语句
    /// 列表直接子级词法声明名）跳过登记。
    pub(crate) fn pre_register_builtin_references(
        &self, stmts: &[Statement], excluded: &HashSet<String>, ctx: &mut CompileCtx,
    ) {
        for stmt in stmts {
            self.pre_scan_builtin_stmt(stmt, excluded, ctx);
        }
    }

    /// 递归遍历语句树，登记其中的内置标识符引用：表达式语句、变量/函数/类声明、
    /// 控制流各分支与模块导出声明。
    fn pre_scan_builtin_stmt(&self, stmt: &Statement, excluded: &HashSet<String>, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => self.pre_scan_builtin_expr(&es.expression, excluded, ctx),
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    self.pre_scan_builtin_pattern(&d.id, excluded, ctx);
                    if let Some(init) = &d.init {
                        self.pre_scan_builtin_expr(init, excluded, ctx);
                    }
                }
            }
            Statement::ReturnStatement(rs) => {
                if let Some(a) = &rs.argument {
                    self.pre_scan_builtin_expr(a, excluded, ctx);
                }
            }
            Statement::IfStatement(is) => {
                self.pre_scan_builtin_expr(&is.test, excluded, ctx);
                self.pre_scan_builtin_stmt(&is.consequent, excluded, ctx);
                if let Some(alt) = &is.alternate {
                    self.pre_scan_builtin_stmt(alt, excluded, ctx);
                }
            }
            Statement::WhileStatement(wh) => {
                self.pre_scan_builtin_expr(&wh.test, excluded, ctx);
                self.pre_scan_builtin_stmt(&wh.body, excluded, ctx);
            }
            Statement::DoWhileStatement(dw) => {
                self.pre_scan_builtin_stmt(&dw.body, excluded, ctx);
                self.pre_scan_builtin_expr(&dw.test, excluded, ctx);
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.pre_scan_builtin_expr(e, excluded, ctx);
                    } else if let oxide_parser::ForStatementInit::VariableDeclaration(decl) = init {
                        for d in &decl.declarations {
                            self.pre_scan_builtin_pattern(&d.id, excluded, ctx);
                            if let Some(init_expr) = &d.init {
                                self.pre_scan_builtin_expr(init_expr, excluded, ctx);
                            }
                        }
                    }
                }
                if let Some(t) = &fs.test {
                    self.pre_scan_builtin_expr(t, excluded, ctx);
                }
                if let Some(u) = &fs.update {
                    self.pre_scan_builtin_expr(u, excluded, ctx);
                }
                self.pre_scan_builtin_stmt(&fs.body, excluded, ctx);
            }
            Statement::ForInStatement(fi) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fi.left {
                    for d in &vd.declarations {
                        self.pre_scan_builtin_pattern(&d.id, excluded, ctx);
                    }
                }
                self.pre_scan_builtin_expr(&fi.right, excluded, ctx);
                self.pre_scan_builtin_stmt(&fi.body, excluded, ctx);
            }
            Statement::ForOfStatement(fo) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fo.left {
                    for d in &vd.declarations {
                        self.pre_scan_builtin_pattern(&d.id, excluded, ctx);
                    }
                }
                self.pre_scan_builtin_expr(&fo.right, excluded, ctx);
                self.pre_scan_builtin_stmt(&fo.body, excluded, ctx);
            }
            Statement::SwitchStatement(sw) => {
                self.pre_scan_builtin_expr(&sw.discriminant, excluded, ctx);
                for case in &sw.cases {
                    if let Some(test) = &case.test {
                        self.pre_scan_builtin_expr(test, excluded, ctx);
                    }
                    for s in &case.consequent {
                        self.pre_scan_builtin_stmt(s, excluded, ctx);
                    }
                }
            }
            Statement::ThrowStatement(ts) => {
                self.pre_scan_builtin_expr(&ts.argument, excluded, ctx);
            }
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.pre_scan_builtin_stmt(s, excluded, ctx);
                }
                if let Some(handler) = &ts.handler {
                    if let Some(param) = &handler.param {
                        self.pre_scan_builtin_pattern(&param.pattern, excluded, ctx);
                    }
                    for s in &handler.body.body {
                        self.pre_scan_builtin_stmt(s, excluded, ctx);
                    }
                }
                if let Some(finalizer) = &ts.finalizer {
                    for s in &finalizer.body {
                        self.pre_scan_builtin_stmt(s, excluded, ctx);
                    }
                }
            }
            Statement::BlockStatement(bs) => {
                for s in &bs.body {
                    self.pre_scan_builtin_stmt(s, excluded, ctx);
                }
            }
            Statement::LabeledStatement(ls) => self.pre_scan_builtin_stmt(&ls.body, excluded, ctx),
            Statement::WithStatement(ws) => {
                self.pre_scan_builtin_expr(&ws.object, excluded, ctx);
                self.pre_scan_builtin_stmt(&ws.body, excluded, ctx);
            }
            Statement::ClassDeclaration(cd) => {
                if let Some(super_class) = &cd.super_class {
                    self.pre_scan_builtin_expr(super_class, excluded, ctx);
                }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(Declaration::VariableDeclaration(vd)) = &exp.declaration {
                    for d in &vd.declarations {
                        self.pre_scan_builtin_pattern(&d.id, excluded, ctx);
                        if let Some(init) = &d.init {
                            self.pre_scan_builtin_expr(init, excluded, ctx);
                        }
                    }
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                if let Some(e) = exp.declaration.as_expression() {
                    self.pre_scan_builtin_expr(e, excluded, ctx);
                }
            }
            Statement::FunctionDeclaration(_) | Statement::BreakStatement(_) | Statement::ContinueStatement(_) => {}
            _ => {}
        }
    }

    /// 递归遍历表达式树，登记其中的内置标识符引用：标识符、调用/构造、成员、可选链、
    /// 对象/数组字面量与各类运算表达式。
    fn pre_scan_builtin_expr(&self, expr: &Expression, excluded: &HashSet<String>, ctx: &mut CompileCtx) {
        match expr {
            Expression::Identifier(ident) => {
                // 本语句列表词法声明同名的 builtin 不登记镜像槽（词法绑定遮蔽，
                // 镜像占位会撞 TDZ 预声明并误导写臂全局解析）。
                if CompileCtx::is_known_builtin(ident.name.as_str()) && !excluded.contains(ident.name.as_str()) {
                    let _ = ctx.lookup_or_builtin(ident.name.as_str());
                }
            }
            Expression::BinaryExpression(bin) => {
                self.pre_scan_builtin_expr(&bin.left, excluded, ctx);
                self.pre_scan_builtin_expr(&bin.right, excluded, ctx);
            }
            Expression::UnaryExpression(un) => {
                self.pre_scan_builtin_expr(&un.argument, excluded, ctx);
            }
            Expression::CallExpression(call) => {
                self.pre_scan_builtin_expr(&call.callee, excluded, ctx);
                for arg in &call.arguments {
                    self.pre_scan_builtin_arg(arg, excluded, ctx);
                }
            }
            Expression::NewExpression(ne) => {
                self.pre_scan_builtin_expr(&ne.callee, excluded, ctx);
                for arg in &ne.arguments {
                    self.pre_scan_builtin_arg(arg, excluded, ctx);
                }
            }
            Expression::LogicalExpression(log) => {
                self.pre_scan_builtin_expr(&log.left, excluded, ctx);
                self.pre_scan_builtin_expr(&log.right, excluded, ctx);
            }
            Expression::ConditionalExpression(cond) => {
                self.pre_scan_builtin_expr(&cond.test, excluded, ctx);
                self.pre_scan_builtin_expr(&cond.consequent, excluded, ctx);
                self.pre_scan_builtin_expr(&cond.alternate, excluded, ctx);
            }
            Expression::PrivateInExpression(pin) => {
                self.pre_scan_builtin_expr(&pin.right, excluded, ctx);
            }
            Expression::SequenceExpression(seq) => {
                for e in &seq.expressions {
                    self.pre_scan_builtin_expr(e, excluded, ctx);
                }
            }
            Expression::AssignmentExpression(assign) => {
                if let Some(target) = assign.left.as_simple_assignment_target() {
                    self.pre_scan_builtin_target(target, excluded, ctx);
                }
                self.pre_scan_builtin_expr(&assign.right, excluded, ctx);
            }
            Expression::UpdateExpression(update) => {
                self.pre_scan_builtin_target(&update.argument, excluded, ctx);
            }
            Expression::TemplateLiteral(tl) => {
                for e in &tl.expressions {
                    self.pre_scan_builtin_expr(e, excluded, ctx);
                }
            }
            Expression::TaggedTemplateExpression(tt) => {
                self.pre_scan_builtin_expr(&tt.tag, excluded, ctx);
                for e in &tt.quasi.expressions {
                    self.pre_scan_builtin_expr(e, excluded, ctx);
                }
            }
            Expression::ObjectExpression(obj) => {
                for prop in &obj.properties {
                    match prop {
                        oxide_parser::ObjectPropertyKind::ObjectProperty(p) => {
                            if p.computed {
                                self.pre_scan_builtin_expr(p.key.to_expression(), excluded, ctx);
                            }
                            self.pre_scan_builtin_expr(&p.value, excluded, ctx);
                        }
                        oxide_parser::ObjectPropertyKind::SpreadProperty(spread) => {
                            self.pre_scan_builtin_expr(&spread.argument, excluded, ctx);
                        }
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                for e in &arr.elements {
                    if let Some(e) = e.as_expression() {
                        self.pre_scan_builtin_expr(e, excluded, ctx);
                    }
                }
            }
            Expression::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
            }
            Expression::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
            }
            Expression::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
                self.pre_scan_builtin_expr(&member.expression, excluded, ctx);
            }
            Expression::ChainExpression(chain) => {
                self.pre_scan_builtin_chain(&chain.expression, excluded, ctx);
            }
            Expression::ParenthesizedExpression(p) => self.pre_scan_builtin_expr(&p.expression, excluded, ctx),
            Expression::YieldExpression(ye) => {
                if let Some(a) = &ye.argument {
                    self.pre_scan_builtin_expr(a, excluded, ctx);
                }
            }
            Expression::AwaitExpression(ae) => self.pre_scan_builtin_expr(&ae.argument, excluded, ctx),
            Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ClassExpression(_) => {}
            _ => {}
        }
    }

    /// 遍历赋值目标，登记其中的内置标识符引用：标识符目标直接登记，成员目标只扫描
    /// 对象与计算键（属性名不是标识符引用）。
    fn pre_scan_builtin_target(
        &self, target: &oxide_parser::SimpleAssignmentTarget, excluded: &HashSet<String>, ctx: &mut CompileCtx,
    ) {
        match target {
            oxide_parser::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                // 标识符目标与表达式位置标识符引用同口径预登记：目标为某内置名的
                // 唯一出现处（无读侧引用）时缺预登记会解析成隐式全局新槽，旧值读
                // 与短路判定拿到未预载值。本语句列表词法声明同名不登记（同表达式臂）。
                if CompileCtx::is_known_builtin(id.name.as_str()) && !excluded.contains(id.name.as_str()) {
                    let _ = ctx.lookup_or_builtin(id.name.as_str());
                }
            }
            oxide_parser::SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
            }
            oxide_parser::SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
                self.pre_scan_builtin_expr(&member.expression, excluded, ctx);
            }
            oxide_parser::SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
            }
            _ => {}
        }
    }

    /// 遍历可选链元素，登记链上成员访问、调用与实参中的内置标识符引用。
    fn pre_scan_builtin_chain(
        &self, element: &oxide_parser::ChainElement, excluded: &HashSet<String>, ctx: &mut CompileCtx,
    ) {
        match element {
            oxide_parser::ChainElement::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
            }
            oxide_parser::ChainElement::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
                self.pre_scan_builtin_expr(&member.expression, excluded, ctx);
            }
            oxide_parser::ChainElement::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, excluded, ctx);
            }
            oxide_parser::ChainElement::CallExpression(call) => {
                self.pre_scan_builtin_expr(&call.callee, excluded, ctx);
                for arg in &call.arguments {
                    self.pre_scan_builtin_arg(arg, excluded, ctx);
                }
            }
            _ => {}
        }
    }

    /// 遍历调用实参：spread 内部表达式也纳入 builtin 预扫描。
    fn pre_scan_builtin_arg(&self, arg: &oxide_parser::Argument, excluded: &HashSet<String>, ctx: &mut CompileCtx) {
        if let Some(e) = arg.as_expression() {
            self.pre_scan_builtin_expr(e, excluded, ctx);
        } else if let oxide_parser::Argument::SpreadElement(sp) = arg {
            self.pre_scan_builtin_expr(&sp.argument, excluded, ctx);
        }
    }

    /// 遍历绑定 pattern 的计算键表达式（`{[k]: a}` 的键）里的 builtin 引用：
    /// 模式键是运行时求值表达式，键内 builtin 标识符须预先登记固定槽位
    /// （与对象字面量计算键预扫描同口径）。
    fn pre_scan_builtin_pattern(&self, pattern: &BindingPattern, excluded: &HashSet<String>, ctx: &mut CompileCtx) {
        match pattern {
            BindingPattern::BindingIdentifier(_) => {}
            BindingPattern::ArrayPattern(ap) => {
                for e in ap.elements.iter().flatten() {
                    self.pre_scan_builtin_pattern(e, excluded, ctx);
                }
                if let Some(rest) = &ap.rest {
                    self.pre_scan_builtin_pattern(&rest.argument, excluded, ctx);
                }
            }
            BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    if prop.computed {
                        self.pre_scan_builtin_expr(prop.key.to_expression(), excluded, ctx);
                    }
                    self.pre_scan_builtin_pattern(&prop.value, excluded, ctx);
                }
                if let Some(rest) = &op.rest {
                    self.pre_scan_builtin_pattern(&rest.argument, excluded, ctx);
                }
            }
            BindingPattern::AssignmentPattern(ap) => {
                self.pre_scan_builtin_pattern(&ap.left, excluded, ctx);
            }
        }
    }
}
