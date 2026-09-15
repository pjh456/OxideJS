//! emit 前置 pass：builtin 引用预扫描 + 声明预登记。
//!
//! 在生成临时寄存器前遍历 AST，把内置全局标识符预先登记到固定寄存器槽
//! （builtin_reg_map），避免与临时寄存器池冲突；同时预声明函数/var 声明，
//! 支持提升语义。

use std::collections::HashSet;

use crate::{CompileCtx, Emitter};
use oxide_parser::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, Statement, VariableDeclarationKind,
};

/// 脚本顶层 lexical 声明（let/const/class）禁止使用的受限全局名：3 名静态集。
/// 受限判定走规范自有属性臂：全局对象上 {configurable:false} 自有属性名（声明
/// 实例化无法建立遮蔽绑定）——现行规范下恰为三常量；eval 与各 builtin 属性皆
/// 可配置（合法遮蔽），不在此列。名基静态集（编译期不可查运行时描述符）。与
/// BUILTIN_GLOBALS 语义不同（后者是 put 写拦截/双写名单），不互相派生，交叠
/// 名由漂移守卫单测断言恒同步。
pub(crate) const RESTRICTED_GLOBAL_LEXICAL_NAMES: &[&str] = &["undefined", "NaN", "Infinity"];

/// 脚本顶层 lexical 声明撞受限全局名 → SyntaxError（声明实例化期拒绝，整程序
/// 编译失败）。错误消息与既有重复声明错同形；非顶层或名不在受限集 → Ok。
fn check_restricted_global_lexical(name: &str, global_lexical: bool) -> Result<(), String> {
    if global_lexical && RESTRICTED_GLOBAL_LEXICAL_NAMES.contains(&name) {
        return Err(format!("Identifier '{name}' has already been declared"));
    }
    Ok(())
}

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
                    self.pre_scan_builtin_pattern(&d.id, ctx);
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
                            self.pre_scan_builtin_pattern(&d.id, ctx);
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
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fi.left {
                    for d in &vd.declarations {
                        self.pre_scan_builtin_pattern(&d.id, ctx);
                    }
                }
                self.pre_scan_builtin_expr(&fi.right, ctx);
                self.pre_scan_builtin_stmt(&fi.body, ctx);
            }
            Statement::ForOfStatement(fo) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fo.left {
                    for d in &vd.declarations {
                        self.pre_scan_builtin_pattern(&d.id, ctx);
                    }
                }
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
                    if let Some(param) = &handler.param {
                        self.pre_scan_builtin_pattern(&param.pattern, ctx);
                    }
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
                        self.pre_scan_builtin_pattern(&d.id, ctx);
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
            oxide_parser::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                // 标识符目标与表达式位置标识符引用同口径预登记：目标为某内置名的
                // 唯一出现处（无读侧引用）时缺预登记会解析成隐式全局新槽，旧值读
                // 与短路判定拿到未预载值。
                if CompileCtx::is_known_builtin(id.name.as_str()) {
                    let _ = ctx.lookup_or_builtin(id.name.as_str());
                }
            }
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

    /// 遍历绑定 pattern 的计算键表达式（`{[k]: a}` 的键）里的 builtin 引用：
    /// 模式键是运行时求值表达式，键内 builtin 标识符须预先登记固定槽位
    /// （与对象字面量计算键预扫描同口径）。
    fn pre_scan_builtin_pattern(&self, pattern: &BindingPattern, ctx: &mut CompileCtx) {
        match pattern {
            BindingPattern::BindingIdentifier(_) => {}
            BindingPattern::ArrayPattern(ap) => {
                for e in ap.elements.iter().flatten() {
                    self.pre_scan_builtin_pattern(e, ctx);
                }
                if let Some(rest) = &ap.rest {
                    self.pre_scan_builtin_pattern(&rest.argument, ctx);
                }
            }
            BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    if prop.computed {
                        self.pre_scan_builtin_expr(prop.key.to_expression(), ctx);
                    }
                    self.pre_scan_builtin_pattern(&prop.value, ctx);
                }
                if let Some(rest) = &op.rest {
                    self.pre_scan_builtin_pattern(&rest.argument, ctx);
                }
            }
            BindingPattern::AssignmentPattern(ap) => {
                self.pre_scan_builtin_pattern(&ap.left, ctx);
            }
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

    /// 预声明单个 `var` 名：仅顶层（全局作用域）的 builtin 名落 builtin 镜像槽
    /// （run 起点预载全局属性值），使 GDI 序言与声明点同步都用入口原值而非
    /// fresh undefined 槽，现存值（NaN 等）不被抹；函数体内 builtin 名仍是局部
    /// var 遮蔽（fresh 槽，不命中只读内置拦截）；其余名 fresh var 槽。
    pub(crate) fn predeclare_var_name(&self, name: &str, ctx: &mut CompileCtx) {
        if ctx.is_global_scope && CompileCtx::is_known_builtin(name) {
            let _ = ctx.lookup_or_builtin(name);
        } else {
            let reg = ctx.alloc_reg();
            let _ = ctx.declare_initialized(name, reg, VariableDeclarationKind::Var, false);
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
                            self.predeclare_var_name(bi.name.as_str(), ctx);
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
                                    self.predeclare_var_name(bi.name.as_str(), ctx);
                                }
                            }
                        }
                    }
                    self.predeclare_var_declarations(std::slice::from_ref(&fs.body), ctx);
                }
                Statement::ForInStatement(fi) => {
                    // var 头名提升到函数/全局作用域（与 For 臂同规则），先于提升
                    // 函数声明发射预声明，使函数体按既有 var 绑定解析头名；
                    // let/const 头是迭代级绑定，不入提升面。
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(decl) = &fi.left {
                        if matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                    self.predeclare_var_name(bi.name.as_str(), ctx);
                                }
                            }
                        }
                    }
                    self.predeclare_var_declarations(std::slice::from_ref(&fi.body), ctx);
                }
                Statement::ForOfStatement(fo) => {
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(decl) = &fo.left {
                        if matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                    self.predeclare_var_name(bi.name.as_str(), ctx);
                                }
                            }
                        }
                    }
                    self.predeclare_var_declarations(std::slice::from_ref(&fo.body), ctx);
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
                                self.predeclare_var_name(bi.name.as_str(), ctx);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 预声明当前块直接子树中的函数声明（ES2015 块级绑定，kind=Let 落当前块），
    /// 使块内声明点之前的引用在编译期可见。递归进入嵌套块时压/弹作用域，
    /// 与 emit 阶段嵌套块 push_scope 的时机对齐。
    ///
    /// # 边界与前提
    /// - 只处理块内函数声明；switch 不推 scope（其 case 内函数声明随 switch
    ///   作用域修复一并处理）；export 声明不可能出现在块内。
    /// - 与 lexical 预声明的顺序：本函数在前，`{ let g; function g(){} }` 时
    ///   lexical 的 `declare_predeclared` 命中已存在的函数绑定自然报重复声明错，
    ///   避免函数预声明被 lexical 占位静默覆盖而破坏 let 的 TDZ。
    /// - var 与函数同名（`{ var g; function g(){} }`）：var 落函数作用域、
    ///   函数落当前块，不同 scope 互不冲突。
    pub(crate) fn predeclare_block_function_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            match statement {
                Statement::FunctionDeclaration(f) => {
                    let Some(identifier) = &f.id else {
                        continue;
                    };
                    let reg = ctx.alloc_reg();
                    let _ = ctx.declare_initialized(identifier.name.as_str(), reg, VariableDeclarationKind::Let, false);
                }
                Statement::BlockStatement(bs) => {
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(&bs.body, ctx);
                    ctx.pop_scope();
                }
                Statement::IfStatement(is) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&is.consequent), ctx);
                    if let Some(alt) = &is.alternate {
                        self.predeclare_block_function_declarations(std::slice::from_ref(alt), ctx);
                    }
                }
                Statement::WhileStatement(wh) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&wh.body), ctx);
                }
                Statement::DoWhileStatement(dw) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&dw.body), ctx);
                }
                Statement::ForStatement(fs) => {
                    // 循环推独立作用域（头声明所在），函数声明落该作用域与 emit 对齐。
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(std::slice::from_ref(&fs.body), ctx);
                    ctx.pop_scope();
                }
                Statement::ForInStatement(fi) => {
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(std::slice::from_ref(&fi.body), ctx);
                    ctx.pop_scope();
                }
                Statement::ForOfStatement(fo) => {
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(std::slice::from_ref(&fo.body), ctx);
                    ctx.pop_scope();
                }
                Statement::TryStatement(ts) => {
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(&ts.block.body, ctx);
                    ctx.pop_scope();
                    if let Some(handler) = &ts.handler {
                        ctx.push_scope();
                        self.predeclare_block_function_declarations(&handler.body.body, ctx);
                        ctx.pop_scope();
                    }
                    if let Some(finalizer) = &ts.finalizer {
                        ctx.push_scope();
                        self.predeclare_block_function_declarations(&finalizer.body, ctx);
                        ctx.pop_scope();
                    }
                }
                Statement::LabeledStatement(ls) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&ls.body), ctx);
                }
                Statement::WithStatement(ws) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&ws.body), ctx);
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
    ///   声明属于外层作用域）；不递归块/if/for/while body（嵌套块自预声明；
    ///   单语句 body 不接受 lexical 声明——lexical 属 Declaration、非 Statement
    ///   子产生式，parser 按语法错误直接拒绝，这些形状不会进入 emit）。
    /// - 跳过 for 头声明（循环作用域由 for 分支内联 declare）。
    /// - `global_lexical` 为 true 时（仅脚本顶层调用点传 `!is_eval_script`）：
    ///   lexical 声明撞受限全局名报 SyntaxError（脚本声明实例化对全局对象受限
    ///   自有属性名做检查，eval 代码声明实例化无此检查）；函数体/块/try/模块
    ///   调用点传 false，lexical 声明是局部绑定不查。
    pub(crate) fn predeclare_lexical_declarations(
        &self, statements: &[Statement], ctx: &mut CompileCtx, global_lexical: bool,
    ) -> Result<(), String> {
        for statement in statements {
            match statement {
                Statement::VariableDeclaration(decl) => {
                    if matches!(decl.kind, VariableDeclarationKind::Var) {
                        continue;
                    }
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    for d in &decl.declarations {
                        self.predeclare_lexical_pattern(&d.id, is_const, ctx, global_lexical)?;
                    }
                }
                Statement::ClassDeclaration(cd) => {
                    if let Some(id) = &cd.id {
                        check_restricted_global_lexical(id.name.as_str(), global_lexical)?;
                        let reg = ctx.alloc_reg();
                        let _ = ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                    }
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        for s in &case.consequent {
                            self.predeclare_lexical_stmt(s, ctx, global_lexical)?;
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
                                    self.predeclare_lexical_pattern(&d.id, is_const, ctx, global_lexical)?;
                                }
                            }
                            Declaration::ClassDeclaration(cd) => {
                                if let Some(id) = &cd.id {
                                    check_restricted_global_lexical(id.name.as_str(), global_lexical)?;
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
                Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
                    ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                        if let Some(id) = &cd.id {
                            check_restricted_global_lexical(id.name.as_str(), global_lexical)?;
                            let reg = ctx.alloc_reg();
                            let _ =
                                ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                        }
                    }
                    ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                        if let Some(id) = &fd.id {
                            let reg = ctx.alloc_reg();
                            let _ =
                                ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }

    /// 预声明单个语句中的 lexical 声明（供 switch case 递归；`let`/`const`/`class` 分支）。
    fn predeclare_lexical_stmt(
        &self, stmt: &Statement, ctx: &mut CompileCtx, global_lexical: bool,
    ) -> Result<(), String> {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                if matches!(decl.kind, VariableDeclarationKind::Var) {
                    return Ok(());
                }
                let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                for d in &decl.declarations {
                    self.predeclare_lexical_pattern(&d.id, is_const, ctx, global_lexical)?;
                }
            }
            Statement::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
                    check_restricted_global_lexical(id.name.as_str(), global_lexical)?;
                    let reg = ctx.alloc_reg();
                    let _ = ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 递归预声明绑定 pattern 内的全部标识符（含数组/对象/默认值解构）。
    fn predeclare_lexical_pattern(
        &self, pattern: &BindingPattern, is_const: bool, ctx: &mut CompileCtx, global_lexical: bool,
    ) -> Result<(), String> {
        match pattern {
            BindingPattern::BindingIdentifier(bi) => {
                check_restricted_global_lexical(bi.name.as_str(), global_lexical)?;
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
                    self.predeclare_lexical_pattern(e, is_const, ctx, global_lexical)?;
                }
                if let Some(rest) = &ap.rest {
                    self.predeclare_lexical_pattern(&rest.argument, is_const, ctx, global_lexical)?;
                }
            }
            BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.predeclare_lexical_pattern(&prop.value, is_const, ctx, global_lexical)?;
                }
                if let Some(rest) = &op.rest {
                    self.predeclare_lexical_pattern(&rest.argument, is_const, ctx, global_lexical)?;
                }
            }
            BindingPattern::AssignmentPattern(ap) => {
                self.predeclare_lexical_pattern(&ap.left, is_const, ctx, global_lexical)?;
            }
        }
        Ok(())
    }
}

impl Emitter {
    /// 递归收集语句树内块级函数声明名（源序去重），供 web-compat 外层 var 绑定
    /// 实例化。
    ///
    /// # 边界与前提
    /// - 调用点直接子级（函数体/程序顶层直接子句）是提升 var 声明，非块级
    ///   函数，不收集；进入嵌套语句（块/if/循环/switch case/try 各区）后的
    ///   函数声明才是块级。
    /// - 不递归进嵌套函数体：那是独立编译单元，其内部块函数名由该单元自身收集。
    /// - switch 不推作用域（case 内函数声明随外层块），收集须进入 case 才能到达
    ///   case 内的 for/块等块级形。
    pub(crate) fn collect_block_function_names(&self, statements: &[Statement]) -> Vec<String> {
        let mut names = Vec::new();
        self.collect_block_function_names_into(statements, &mut names, false);
        names
    }

    fn collect_block_function_names_into(&self, statements: &[Statement], names: &mut Vec<String>, block_level: bool) {
        for statement in statements {
            match statement {
                Statement::FunctionDeclaration(f) => {
                    if block_level {
                        if let Some(id) = &f.id {
                            let name = id.name.to_string();
                            if !names.contains(&name) {
                                names.push(name);
                            }
                        }
                    }
                }
                Statement::BlockStatement(bs) => self.collect_block_function_names_into(&bs.body, names, true),
                Statement::IfStatement(is) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&is.consequent), names, true);
                    if let Some(alt) = &is.alternate {
                        self.collect_block_function_names_into(std::slice::from_ref(alt), names, true);
                    }
                }
                Statement::WhileStatement(wh) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&wh.body), names, true)
                }
                Statement::DoWhileStatement(dw) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&dw.body), names, true)
                }
                Statement::ForStatement(fs) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&fs.body), names, true)
                }
                Statement::ForInStatement(fi) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&fi.body), names, true)
                }
                Statement::ForOfStatement(fo) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&fo.body), names, true)
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        self.collect_block_function_names_into(&case.consequent, names, true);
                    }
                }
                Statement::TryStatement(ts) => {
                    self.collect_block_function_names_into(&ts.block.body, names, true);
                    if let Some(handler) = &ts.handler {
                        self.collect_block_function_names_into(&handler.body.body, names, true);
                    }
                    if let Some(finalizer) = &ts.finalizer {
                        self.collect_block_function_names_into(&finalizer.body, names, true);
                    }
                }
                Statement::LabeledStatement(ls) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&ls.body), names, true)
                }
                Statement::WithStatement(ws) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&ws.body), names, true)
                }
                _ => {}
            }
        }
    }

    /// 收集语句树内的词法声明名（let/const/class 含解构叶、for 头词法名、catch
    /// 参数）：块级函数名与树内任意词法声明同名时退化为纯块作用域（无外层
    /// 绑定、无求值写回）。
    ///
    /// # 边界与前提
    /// - 不递归进嵌套函数体/class 元素体：独立编译单元，其词法名不撞本单元的
    ///   块函数名。
    /// - var 声明是 var 绑定，不撞词法环境，不入集。
    fn collect_lexical_name_suppressions(&self, statements: &[Statement], names: &mut HashSet<String>) {
        for statement in statements {
            match statement {
                Statement::VariableDeclaration(vd) => {
                    if !matches!(vd.kind, VariableDeclarationKind::Var) {
                        for d in &vd.declarations {
                            self.collect_binding_pattern_names(&d.id, names);
                        }
                    }
                }
                Statement::ClassDeclaration(cd) => {
                    if let Some(id) = &cd.id {
                        names.insert(id.name.to_string());
                    }
                }
                Statement::ForStatement(fs) => {
                    if let Some(oxide_parser::ForStatementInit::VariableDeclaration(decl)) = &fs.init {
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                self.collect_binding_pattern_names(&d.id, names);
                            }
                        }
                    }
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&fs.body), names);
                }
                Statement::ForInStatement(fi) => {
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(decl) = &fi.left {
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                self.collect_binding_pattern_names(&d.id, names);
                            }
                        }
                    }
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&fi.body), names);
                }
                Statement::ForOfStatement(fo) => {
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(decl) = &fo.left {
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                self.collect_binding_pattern_names(&d.id, names);
                            }
                        }
                    }
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&fo.body), names);
                }
                Statement::TryStatement(ts) => {
                    // catch 参数是 handler 环境的词法绑定。
                    if let Some(handler) = &ts.handler {
                        if let Some(param) = &handler.param {
                            self.collect_binding_pattern_names(&param.pattern, names);
                        }
                        self.collect_lexical_name_suppressions(&handler.body.body, names);
                    }
                    self.collect_lexical_name_suppressions(&ts.block.body, names);
                    if let Some(finalizer) = &ts.finalizer {
                        self.collect_lexical_name_suppressions(&finalizer.body, names);
                    }
                }
                Statement::BlockStatement(bs) => self.collect_lexical_name_suppressions(&bs.body, names),
                Statement::IfStatement(is) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&is.consequent), names);
                    if let Some(alt) = &is.alternate {
                        self.collect_lexical_name_suppressions(std::slice::from_ref(alt), names);
                    }
                }
                Statement::WhileStatement(wh) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&wh.body), names)
                }
                Statement::DoWhileStatement(dw) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&dw.body), names)
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        self.collect_lexical_name_suppressions(&case.consequent, names);
                    }
                }
                Statement::LabeledStatement(ls) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&ls.body), names)
                }
                Statement::WithStatement(ws) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&ws.body), names)
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(Declaration::VariableDeclaration(vd)) = &exp.declaration {
                        if !matches!(vd.kind, VariableDeclarationKind::Var) {
                            for d in &vd.declarations {
                                self.collect_binding_pattern_names(&d.id, names);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 块级函数名 web-compat 外层绑定抑制集：形参名（含解构叶/rest）∪ 语句树内
    /// 词法声明名。命中抑制的名字不建外层 var 绑定、不求值写回。
    pub(crate) fn collect_block_fn_suppressed_names(
        &self, statements: &[Statement], param_names: &HashSet<String>,
    ) -> HashSet<String> {
        let mut names = param_names.clone();
        self.collect_lexical_name_suppressions(statements, &mut names);
        names
    }
}
