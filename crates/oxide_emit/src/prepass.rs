//! emit 前置 pass：builtin 引用预扫描 + 声明预登记。
//!
//! 在生成临时寄存器前遍历 AST，把内置全局标识符预先登记到固定寄存器槽
//! （builtin_reg_map），避免与临时寄存器池冲突；同时预声明函数/var 声明，
//! 支持提升语义。

use crate::{CompileCtx, Emitter};
use oxide_parser::{BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, Statement, VariableDeclarationKind};

impl Emitter {
    /// 在临时寄存器池之前分配 builtin 槽位。
    pub(crate) fn pre_register_builtin_references(&self, stmts: &[Statement], ctx: &mut CompileCtx) {
        for stmt in stmts {
            self.pre_scan_builtin_stmt(stmt, ctx);
        }
    }

    fn pre_scan_builtin_stmt(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => self.pre_scan_builtin_expr(&es.expression, ctx),
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    if let Some(init) = &d.init {
                        self.pre_scan_builtin_expr(init, ctx);
                    }
                }
            }
            Statement::ReturnStatement(rs) => {
                if let Some(a) = &rs.argument {
                    self.pre_scan_builtin_expr(a, ctx);
                }
            }
            Statement::IfStatement(is) => {
                self.pre_scan_builtin_expr(&is.test, ctx);
                self.pre_scan_builtin_stmt(&is.consequent, ctx);
                if let Some(alt) = &is.alternate {
                    self.pre_scan_builtin_stmt(alt, ctx);
                }
            }
            Statement::WhileStatement(wh) => {
                self.pre_scan_builtin_expr(&wh.test, ctx);
                self.pre_scan_builtin_stmt(&wh.body, ctx);
            }
            Statement::DoWhileStatement(dw) => {
                self.pre_scan_builtin_stmt(&dw.body, ctx);
                self.pre_scan_builtin_expr(&dw.test, ctx);
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.pre_scan_builtin_expr(e, ctx);
                    } else if let oxide_parser::ForStatementInit::VariableDeclaration(decl) = init {
                        for d in &decl.declarations {
                            if let Some(init_expr) = &d.init {
                                self.pre_scan_builtin_expr(init_expr, ctx);
                            }
                        }
                    }
                }
                if let Some(t) = &fs.test {
                    self.pre_scan_builtin_expr(t, ctx);
                }
                if let Some(u) = &fs.update {
                    self.pre_scan_builtin_expr(u, ctx);
                }
                self.pre_scan_builtin_stmt(&fs.body, ctx);
            }
            Statement::ForInStatement(fi) => {
                self.pre_scan_builtin_expr(&fi.right, ctx);
                self.pre_scan_builtin_stmt(&fi.body, ctx);
            }
            Statement::ForOfStatement(fo) => {
                self.pre_scan_builtin_expr(&fo.right, ctx);
                self.pre_scan_builtin_stmt(&fo.body, ctx);
            }
            Statement::SwitchStatement(sw) => {
                self.pre_scan_builtin_expr(&sw.discriminant, ctx);
                for case in &sw.cases {
                    if let Some(test) = &case.test {
                        self.pre_scan_builtin_expr(test, ctx);
                    }
                    for s in &case.consequent {
                        self.pre_scan_builtin_stmt(s, ctx);
                    }
                }
            }
            Statement::ThrowStatement(ts) => {
                self.pre_scan_builtin_expr(&ts.argument, ctx);
            }
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.pre_scan_builtin_stmt(s, ctx);
                }
                if let Some(handler) = &ts.handler {
                    for s in &handler.body.body {
                        self.pre_scan_builtin_stmt(s, ctx);
                    }
                }
                if let Some(finalizer) = &ts.finalizer {
                    for s in &finalizer.body {
                        self.pre_scan_builtin_stmt(s, ctx);
                    }
                }
            }
            Statement::BlockStatement(bs) => {
                for s in &bs.body {
                    self.pre_scan_builtin_stmt(s, ctx);
                }
            }
            Statement::LabeledStatement(ls) => self.pre_scan_builtin_stmt(&ls.body, ctx),
            Statement::WithStatement(ws) => {
                self.pre_scan_builtin_expr(&ws.object, ctx);
                self.pre_scan_builtin_stmt(&ws.body, ctx);
            }
            Statement::ClassDeclaration(cd) => {
                if let Some(super_class) = &cd.super_class {
                    self.pre_scan_builtin_expr(super_class, ctx);
                }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(Declaration::VariableDeclaration(vd)) = &exp.declaration {
                    for d in &vd.declarations {
                        if let Some(init) = &d.init {
                            self.pre_scan_builtin_expr(init, ctx);
                        }
                    }
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                if let Some(e) = exp.declaration.as_expression() {
                    self.pre_scan_builtin_expr(e, ctx);
                }
            }
            Statement::FunctionDeclaration(_) | Statement::BreakStatement(_) | Statement::ContinueStatement(_) => {}
            _ => {}
        }
    }

    fn pre_scan_builtin_expr(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::Identifier(ident) => {
                if CompileCtx::is_known_builtin(ident.name.as_str()) {
                    let _ = ctx.lookup_or_builtin(ident.name.as_str());
                }
            }
            Expression::BinaryExpression(bin) => {
                self.pre_scan_builtin_expr(&bin.left, ctx);
                self.pre_scan_builtin_expr(&bin.right, ctx);
            }
            Expression::UnaryExpression(un) => {
                self.pre_scan_builtin_expr(&un.argument, ctx);
            }
            Expression::CallExpression(call) => {
                self.pre_scan_builtin_expr(&call.callee, ctx);
                for arg in &call.arguments {
                    self.pre_scan_builtin_arg(arg, ctx);
                }
            }
            Expression::NewExpression(ne) => {
                self.pre_scan_builtin_expr(&ne.callee, ctx);
                for arg in &ne.arguments {
                    self.pre_scan_builtin_arg(arg, ctx);
                }
            }
            Expression::LogicalExpression(log) => {
                self.pre_scan_builtin_expr(&log.left, ctx);
                self.pre_scan_builtin_expr(&log.right, ctx);
            }
            Expression::ConditionalExpression(cond) => {
                self.pre_scan_builtin_expr(&cond.test, ctx);
                self.pre_scan_builtin_expr(&cond.consequent, ctx);
                self.pre_scan_builtin_expr(&cond.alternate, ctx);
            }
            Expression::PrivateInExpression(pin) => {
                self.pre_scan_builtin_expr(&pin.right, ctx);
            }
            Expression::SequenceExpression(seq) => {
                for e in &seq.expressions {
                    self.pre_scan_builtin_expr(e, ctx);
                }
            }
            Expression::AssignmentExpression(assign) => {
                if let Some(target) = assign.left.as_simple_assignment_target() {
                    self.pre_scan_builtin_target(target, ctx);
                }
                self.pre_scan_builtin_expr(&assign.right, ctx);
            }
            Expression::UpdateExpression(update) => {
                self.pre_scan_builtin_target(&update.argument, ctx);
            }
            Expression::TemplateLiteral(tl) => {
                for e in &tl.expressions {
                    self.pre_scan_builtin_expr(e, ctx);
                }
            }
            Expression::TaggedTemplateExpression(tt) => {
                self.pre_scan_builtin_expr(&tt.tag, ctx);
                for e in &tt.quasi.expressions {
                    self.pre_scan_builtin_expr(e, ctx);
                }
            }
            Expression::ObjectExpression(obj) => {
                for prop in &obj.properties {
                    match prop {
                        oxide_parser::ObjectPropertyKind::ObjectProperty(p) => {
                            if p.computed {
                                self.pre_scan_builtin_expr(p.key.to_expression(), ctx);
                            }
                            self.pre_scan_builtin_expr(&p.value, ctx);
                        }
                        oxide_parser::ObjectPropertyKind::SpreadProperty(spread) => {
                            self.pre_scan_builtin_expr(&spread.argument, ctx);
                        }
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                for e in &arr.elements {
                    if let Some(e) = e.as_expression() {
                        self.pre_scan_builtin_expr(e, ctx);
                    }
                }
            }
            Expression::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            Expression::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            Expression::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
                self.pre_scan_builtin_expr(&member.expression, ctx);
            }
            Expression::ChainExpression(chain) => {
                self.pre_scan_builtin_chain(&chain.expression, ctx);
            }
            Expression::ParenthesizedExpression(p) => self.pre_scan_builtin_expr(&p.expression, ctx),
            Expression::YieldExpression(ye) => {
                if let Some(a) = &ye.argument {
                    self.pre_scan_builtin_expr(a, ctx);
                }
            }
            Expression::AwaitExpression(ae) => self.pre_scan_builtin_expr(&ae.argument, ctx),
            Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ClassExpression(_) => {}
            _ => {}
        }
    }

    fn pre_scan_builtin_target(&self, target: &oxide_parser::SimpleAssignmentTarget, ctx: &mut CompileCtx) {
        match target {
            oxide_parser::SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            oxide_parser::SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
                self.pre_scan_builtin_expr(&member.expression, ctx);
            }
            oxide_parser::SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            _ => {}
        }
    }

    fn pre_scan_builtin_chain(&self, element: &oxide_parser::ChainElement, ctx: &mut CompileCtx) {
        match element {
            oxide_parser::ChainElement::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            oxide_parser::ChainElement::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
                self.pre_scan_builtin_expr(&member.expression, ctx);
            }
            oxide_parser::ChainElement::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            oxide_parser::ChainElement::CallExpression(call) => {
                self.pre_scan_builtin_expr(&call.callee, ctx);
                for arg in &call.arguments {
                    self.pre_scan_builtin_arg(arg, ctx);
                }
            }
            _ => {}
        }
    }

    /// 遍历调用实参：spread 内部表达式也纳入 builtin 预扫描。
    fn pre_scan_builtin_arg(&self, arg: &oxide_parser::Argument, ctx: &mut CompileCtx) {
        if let Some(e) = arg.as_expression() {
            self.pre_scan_builtin_expr(e, ctx);
        } else if let oxide_parser::Argument::SpreadElement(sp) = arg {
            self.pre_scan_builtin_expr(&sp.argument, ctx);
        }
    }

    pub(crate) fn predeclare_function_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            let function = match statement {
                Statement::FunctionDeclaration(f) => f,
                Statement::ExportNamedDeclaration(exp) => match &exp.declaration {
                    Some(Declaration::FunctionDeclaration(f)) => f,
                    _ => continue,
                },
                // export default function foo(){}：foo 绑定由 lexical predeclare 以
                // Const 预声明（emit 侧 emit_bind_target(Const) 消费预登记槽）。
                _ => continue,
            };
            let Some(identifier) = &function.id else {
                continue;
            };
            let reg = ctx.alloc_reg();
            let _ = ctx.declare_initialized(identifier.name.as_str(), reg, VariableDeclarationKind::Var, false);
        }
    }

    /// 预声明语句列表中的全部 `var` 绑定，使先发的提升函数声明能解析它们。
    /// 顶层 `var` 名在编译闭包捕获它的函数体时必须可见。
    pub(crate) fn predeclare_var_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            match statement {
                Statement::VariableDeclaration(decl) => {
                    if !matches!(decl.kind, VariableDeclarationKind::Var) {
                        continue;
                    }
                    for d in &decl.declarations {
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                            let reg = ctx.alloc_reg();
                            let _ = ctx.declare_initialized(bi.name.as_str(), reg, VariableDeclarationKind::Var, false);
                        }
                    }
                }
                Statement::BlockStatement(bs) => self.predeclare_var_declarations(&bs.body, ctx),
                Statement::IfStatement(is) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&is.consequent), ctx);
                    if let Some(alt) = &is.alternate {
                        self.predeclare_var_declarations(std::slice::from_ref(alt), ctx);
                    }
                }
                Statement::WhileStatement(wh) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&wh.body), ctx);
                }
                Statement::DoWhileStatement(dw) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&dw.body), ctx);
                }
                Statement::ForStatement(fs) => {
                    if let Some(oxide_parser::ForStatementInit::VariableDeclaration(decl)) = &fs.init {
                        if matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                    let reg = ctx.alloc_reg();
                                    let _ = ctx.declare_initialized(
                                        bi.name.as_str(),
                                        reg,
                                        VariableDeclarationKind::Var,
                                        false,
                                    );
                                }
                            }
                        }
                    }
                    self.predeclare_var_declarations(std::slice::from_ref(&fs.body), ctx);
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        self.predeclare_var_declarations(&case.consequent, ctx);
                    }
                }
                Statement::TryStatement(ts) => {
                    self.predeclare_var_declarations(&ts.block.body, ctx);
                    if let Some(handler) = &ts.handler {
                        self.predeclare_var_declarations(&handler.body.body, ctx);
                    }
                    if let Some(finalizer) = &ts.finalizer {
                        self.predeclare_var_declarations(&finalizer.body, ctx);
                    }
                }
                Statement::LabeledStatement(ls) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&ls.body), ctx);
                }
                Statement::WithStatement(ws) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&ws.body), ctx);
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(Declaration::VariableDeclaration(decl)) = &exp.declaration {
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            continue;
                        }
                        for d in &decl.declarations {
                            if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                let reg = ctx.alloc_reg();
                                let _ = ctx.declare_initialized(
                                    bi.name.as_str(),
                                    reg,
                                    VariableDeclarationKind::Var,
                                    false,
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 预声明当前作用域直接子语句中的 `let`/`const`/`class` 绑定（未初始化，
    /// 建立 TDZ 占位）。使声明点之前的读取在编译期可分辨为 TDZ 而非隐式全局，
    /// 声明点复用预登记槽位。
    ///
    /// # 边界与前提
    /// - 只扫描直接子语句 + 递归 switch case（switch 不推 scope，case 内 lexical
    ///   声明属于外层作用域）；不递归块/if/for/while body（嵌套块自预声明，
    ///   单语句 body 不含 lexical 声明）。
    /// - 跳过 for 头声明（循环作用域由 for 分支内联 declare）。
    pub(crate) fn predeclare_lexical_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            match statement {
                Statement::VariableDeclaration(decl) => {
                    if matches!(decl.kind, VariableDeclarationKind::Var) {
                        continue;
                    }
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    for d in &decl.declarations {
                        self.predeclare_lexical_pattern(&d.id, is_const, ctx);
                    }
                }
                Statement::ClassDeclaration(cd) => {
                    if let Some(id) = &cd.id {
                        let reg = ctx.alloc_reg();
                        let _ = ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                    }
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        for s in &case.consequent {
                            self.predeclare_lexical_stmt(s, ctx);
                        }
                    }
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(decl) = &exp.declaration {
                        match decl {
                            Declaration::VariableDeclaration(vd) => {
                                if matches!(vd.kind, VariableDeclarationKind::Var) {
                                    continue;
                                }
                                let is_const = matches!(vd.kind, VariableDeclarationKind::Const);
                                for d in &vd.declarations {
                                    self.predeclare_lexical_pattern(&d.id, is_const, ctx);
                                }
                            }
                            Declaration::ClassDeclaration(cd) => {
                                if let Some(id) = &cd.id {
                                    let reg = ctx.alloc_reg();
                                    let _ = ctx.declare_predeclared(
                                        id.name.as_str(),
                                        reg,
                                        VariableDeclarationKind::Const,
                                        true,
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Statement::ExportDefaultDeclaration(exp) => {
                    match &exp.declaration {
                        ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                            if let Some(id) = &cd.id {
                                let reg = ctx.alloc_reg();
                                let _ = ctx.declare_predeclared(
                                    id.name.as_str(),
                                    reg,
                                    VariableDeclarationKind::Const,
                                    true,
                                );
                            }
                        }
                        ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                            if let Some(id) = &fd.id {
                                let reg = ctx.alloc_reg();
                                let _ = ctx.declare_predeclared(
                                    id.name.as_str(),
                                    reg,
                                    VariableDeclarationKind::Const,
                                    true,
                                );
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// 预声明单个语句中的 lexical 声明（供 switch case 递归；`let`/`const`/`class` 分支）。
    fn predeclare_lexical_stmt(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                if matches!(decl.kind, VariableDeclarationKind::Var) {
                    return;
                }
                let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                for d in &decl.declarations {
                    self.predeclare_lexical_pattern(&d.id, is_const, ctx);
                }
            }
            Statement::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
                    let reg = ctx.alloc_reg();
                    let _ = ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                }
            }
            _ => {}
        }
    }

    /// 递归预声明绑定 pattern 内的全部标识符（含数组/对象/默认值解构）。
    fn predeclare_lexical_pattern(&self, pattern: &BindingPattern, is_const: bool, ctx: &mut CompileCtx) {
        match pattern {
            BindingPattern::BindingIdentifier(bi) => {
                let reg = ctx.alloc_reg();
                let _ = ctx.declare_predeclared(
                    bi.name.as_str(),
                    reg,
                    if is_const {
                        VariableDeclarationKind::Const
                    } else {
                        VariableDeclarationKind::Let
                    },
                    is_const,
                );
            }
            BindingPattern::ArrayPattern(ap) => {
                for e in ap.elements.iter().flatten() {
                    self.predeclare_lexical_pattern(e, is_const, ctx);
                }
                if let Some(rest) = &ap.rest {
                    self.predeclare_lexical_pattern(&rest.argument, is_const, ctx);
                }
            }
            BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.predeclare_lexical_pattern(&prop.value, is_const, ctx);
                }
                if let Some(rest) = &op.rest {
                    self.predeclare_lexical_pattern(&rest.argument, is_const, ctx);
                }
            }
            BindingPattern::AssignmentPattern(ap) => {
                self.predeclare_lexical_pattern(&ap.left, is_const, ctx);
            }
        }
    }
}
