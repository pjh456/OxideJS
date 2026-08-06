//! 闭包捕获分析（AST 级，emit 前完成，时序无关）。
//!
//! 收集本函数绑定的名字（own_bindings）与被嵌套函数捕获的名字
//! （captured_bindings → cell_idx，按名排序稳定跨 run），为 MAKE_CELL /
//! CELL_GET / CELL_SET 与子函数 upvalue cell_idx 提供统一依据。

use std::collections::{BTreeMap, HashSet};

use oxide_bytecode::module::UpvalueCapture;
use oxide_parser::{Expression, Statement};

use crate::Emitter;

impl Emitter {
    fn collect_fn_param_names(&self, params: &oxide_parser::FormalParameters) -> HashSet<String> {
        let mut names = HashSet::new();
        for p in &params.items {
            if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &p.pattern {
                names.insert(bi.name.to_string());
            }
        }
        names
    }

    /// 收集当前函数作用域声明的绑定名（参数 + 变量/函数声明，含嵌套 block，不含嵌套函数体）。
    pub(crate) fn collect_own_binding_names(&self, param_names: &[&str], stmts: &[Statement]) -> HashSet<String> {
        let mut names = HashSet::new();
        for p in param_names {
            names.insert(p.to_string());
        }
        self.collect_decl_names_stmt(stmts, &mut names);
        names
    }

    fn collect_decl_names_stmt(&self, stmts: &[Statement], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                Statement::VariableDeclaration(vd) => {
                    for d in &vd.declarations {
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                            out.insert(bi.name.to_string());
                        }
                    }
                }
                Statement::FunctionDeclaration(fd) => {
                    if let Some(id) = &fd.id {
                        out.insert(id.name.to_string());
                    }
                }
                Statement::BlockStatement(b) => self.collect_decl_names_stmt(&b.body, out),
                Statement::IfStatement(is) => {
                    self.collect_decl_names_stmt(std::slice::from_ref(&is.consequent), out);
                    if let Some(alt) = &is.alternate {
                        self.collect_decl_names_stmt(std::slice::from_ref(alt), out);
                    }
                }
                Statement::WhileStatement(w) => self.collect_decl_names_stmt(std::slice::from_ref(&w.body), out),
                Statement::DoWhileStatement(d) => self.collect_decl_names_stmt(std::slice::from_ref(&d.body), out),
                Statement::ForStatement(f) => {
                    if let Some(oxide_parser::ForStatementInit::VariableDeclaration(vd)) = &f.init {
                        for d in &vd.declarations {
                            if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                out.insert(bi.name.to_string());
                            }
                        }
                    }
                    self.collect_decl_names_stmt(std::slice::from_ref(&f.body), out);
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        self.collect_decl_names_stmt(&case.consequent, out);
                    }
                }
                Statement::TryStatement(ts) => {
                    self.collect_decl_names_stmt(&ts.block.body, out);
                    if let Some(h) = &ts.handler {
                        self.collect_decl_names_stmt(&h.body.body, out);
                    }
                    if let Some(f) = &ts.finalizer {
                        self.collect_decl_names_stmt(&f.body, out);
                    }
                }
                Statement::LabeledStatement(ls) => self.collect_decl_names_stmt(std::slice::from_ref(&ls.body), out),
                _ => {}
            }
        }
    }

    /// 扫描 stmts 内（含任意深度嵌套函数）对 `ref_set` 的引用，写入 out。
    /// 递归进入嵌套函数时累加其局部绑定为遮蔽集，避免把内层局部误判为捕获。
    fn collect_capture_names(&self, stmts: &[Statement], ref_set: &HashSet<String>, out: &mut HashSet<String>) {
        let shadow = HashSet::new();
        self.collect_capture_names_shadowed(stmts, ref_set, &shadow, out);
    }

    fn collect_capture_names_shadowed(
        &self, stmts: &[Statement], ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        for stmt in stmts {
            self.collect_capture_names_stmt(stmt, ref_set, shadow, out);
        }
    }

    fn collect_capture_names_stmt(
        &self, stmt: &Statement, ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::ExpressionStatement(es) => self.collect_capture_names_expr(&es.expression, ref_set, shadow, out),
            Statement::ReturnStatement(rs) => {
                if let Some(a) = &rs.argument {
                    self.collect_capture_names_expr(a, ref_set, shadow, out);
                }
            }
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    if let Some(init) = &d.init {
                        self.collect_capture_names_expr(init, ref_set, shadow, out);
                    }
                }
            }
            Statement::FunctionDeclaration(fd) => {
                let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let mut inner = shadow.clone();
                inner.extend(self.collect_fn_param_names(&fd.params));
                inner.extend(self.collect_own_binding_names(&[], body));
                self.collect_capture_names_shadowed(body, ref_set, &inner, out);
            }
            Statement::IfStatement(is) => {
                self.collect_capture_names_expr(&is.test, ref_set, shadow, out);
                self.collect_capture_names_stmt(&is.consequent, ref_set, shadow, out);
                if let Some(alt) = &is.alternate {
                    self.collect_capture_names_stmt(alt, ref_set, shadow, out);
                }
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                    if let oxide_parser::ForStatementInit::VariableDeclaration(vd) = init {
                        for d in &vd.declarations {
                            if let Some(i) = &d.init {
                                self.collect_capture_names_expr(i, ref_set, shadow, out);
                            }
                        }
                    }
                }
                if let Some(t) = &fs.test {
                    self.collect_capture_names_expr(t, ref_set, shadow, out);
                }
                if let Some(u) = &fs.update {
                    self.collect_capture_names_expr(u, ref_set, shadow, out);
                }
                self.collect_capture_names_stmt(&fs.body, ref_set, shadow, out);
            }
            Statement::WhileStatement(w) => {
                self.collect_capture_names_expr(&w.test, ref_set, shadow, out);
                self.collect_capture_names_stmt(&w.body, ref_set, shadow, out);
            }
            Statement::DoWhileStatement(d) => {
                self.collect_capture_names_stmt(&d.body, ref_set, shadow, out);
                self.collect_capture_names_expr(&d.test, ref_set, shadow, out);
            }
            Statement::ForInStatement(fi) => {
                self.collect_capture_names_expr(&fi.right, ref_set, shadow, out);
                self.collect_capture_names_stmt(&fi.body, ref_set, shadow, out);
            }
            Statement::ForOfStatement(fo) => {
                self.collect_capture_names_expr(&fo.right, ref_set, shadow, out);
                self.collect_capture_names_stmt(&fo.body, ref_set, shadow, out);
            }
            Statement::BlockStatement(b) => self.collect_capture_names_shadowed(&b.body, ref_set, shadow, out),
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.collect_capture_names_stmt(s, ref_set, shadow, out);
                }
                if let Some(h) = &ts.handler {
                    for s in &h.body.body {
                        self.collect_capture_names_stmt(s, ref_set, shadow, out);
                    }
                }
                if let Some(f) = &ts.finalizer {
                    for s in &f.body {
                        self.collect_capture_names_stmt(s, ref_set, shadow, out);
                    }
                }
            }
            Statement::ThrowStatement(ts) => self.collect_capture_names_expr(&ts.argument, ref_set, shadow, out),
            Statement::SwitchStatement(sw) => {
                self.collect_capture_names_expr(&sw.discriminant, ref_set, shadow, out);
                for case in &sw.cases {
                    for s in &case.consequent {
                        self.collect_capture_names_stmt(s, ref_set, shadow, out);
                    }
                }
            }
            Statement::LabeledStatement(ls) => self.collect_capture_names_stmt(&ls.body, ref_set, shadow, out),
            _ => {}
        }
    }

    fn collect_capture_names_expr(
        &self, expr: &Expression, ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        match expr {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if ref_set.contains(name) && !shadow.contains(name) {
                    out.insert(id.name.to_string());
                }
            }
            Expression::AssignmentExpression(ae) => {
                if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(ati) = &ae.left {
                    let name = ati.name.as_str();
                    if ref_set.contains(name) && !shadow.contains(name) {
                        out.insert(ati.name.to_string());
                    }
                }
                self.collect_capture_names_expr(&ae.right, ref_set, shadow, out);
            }
            Expression::UpdateExpression(ue) => {
                if let oxide_parser::SimpleAssignmentTarget::AssignmentTargetIdentifier(ati) = &ue.argument {
                    let name = ati.name.as_str();
                    if ref_set.contains(name) && !shadow.contains(name) {
                        out.insert(ati.name.to_string());
                    }
                }
            }
            Expression::FunctionExpression(fe) => {
                let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let mut inner = shadow.clone();
                inner.extend(self.collect_fn_param_names(&fe.params));
                inner.extend(self.collect_own_binding_names(&[], body));
                self.collect_capture_names_shadowed(body, ref_set, &inner, out);
            }
            Expression::ArrowFunctionExpression(ae) => {
                let mut inner = shadow.clone();
                inner.extend(self.collect_fn_param_names(&ae.params));
                inner.extend(self.collect_own_binding_names(&[], &ae.body.statements));
                self.collect_capture_names_shadowed(&ae.body.statements, ref_set, &inner, out);
            }
            Expression::BinaryExpression(be) => {
                self.collect_capture_names_expr(&be.left, ref_set, shadow, out);
                self.collect_capture_names_expr(&be.right, ref_set, shadow, out);
            }
            Expression::UnaryExpression(ue) => self.collect_capture_names_expr(&ue.argument, ref_set, shadow, out),
            Expression::CallExpression(ce) => {
                self.collect_capture_names_expr(&ce.callee, ref_set, shadow, out);
                for a in &ce.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            }
            Expression::NewExpression(ne) => {
                self.collect_capture_names_expr(&ne.callee, ref_set, shadow, out);
                for a in &ne.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            }
            Expression::SequenceExpression(se) => {
                for e in &se.expressions {
                    self.collect_capture_names_expr(e, ref_set, shadow, out);
                }
            }
            Expression::ConditionalExpression(ce) => {
                self.collect_capture_names_expr(&ce.test, ref_set, shadow, out);
                self.collect_capture_names_expr(&ce.consequent, ref_set, shadow, out);
                self.collect_capture_names_expr(&ce.alternate, ref_set, shadow, out);
            }
            Expression::ArrayExpression(ae) => {
                for e in &ae.elements {
                    if let Some(e) = e.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            }
            Expression::LogicalExpression(le) => {
                self.collect_capture_names_expr(&le.left, ref_set, shadow, out);
                self.collect_capture_names_expr(&le.right, ref_set, shadow, out);
            }
            Expression::ComputedMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
                self.collect_capture_names_expr(&m.expression, ref_set, shadow, out);
            }
            Expression::StaticMemberExpression(m) => self.collect_capture_names_expr(&m.object, ref_set, shadow, out),
            Expression::PrivateFieldExpression(m) => self.collect_capture_names_expr(&m.object, ref_set, shadow, out),
            Expression::ParenthesizedExpression(p) => {
                self.collect_capture_names_expr(&p.expression, ref_set, shadow, out)
            }
            Expression::TemplateLiteral(tl) => {
                for e in &tl.expressions {
                    self.collect_capture_names_expr(e, ref_set, shadow, out);
                }
            }
            Expression::TaggedTemplateExpression(tt) => {
                self.collect_capture_names_expr(&tt.tag, ref_set, shadow, out);
                for e in &tt.quasi.expressions {
                    self.collect_capture_names_expr(e, ref_set, shadow, out);
                }
            }
            Expression::ObjectExpression(o) => {
                for prop in &o.properties {
                    if let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop {
                        self.collect_capture_names_expr(&p.value, ref_set, shadow, out);
                    }
                }
            }
            Expression::ChainExpression(c) => self.collect_capture_names_chain(&c.expression, ref_set, shadow, out),
            _ => {}
        }
    }

    fn collect_capture_names_chain(
        &self, element: &oxide_parser::ChainElement, ref_set: &HashSet<String>, shadow: &HashSet<String>,
        out: &mut HashSet<String>,
    ) {
        match element {
            oxide_parser::ChainElement::StaticMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
            }
            oxide_parser::ChainElement::ComputedMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
                self.collect_capture_names_expr(&m.expression, ref_set, shadow, out);
            }
            oxide_parser::ChainElement::PrivateFieldExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
            }
            oxide_parser::ChainElement::CallExpression(call) => {
                self.collect_capture_names_expr(&call.callee, ref_set, shadow, out);
                for a in &call.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// 分析本函数：哪些绑定被任意深度嵌套函数捕获 → captured_bindings。
    pub(crate) fn collect_captured_bindings(&self, stmts: &[Statement], own: &HashSet<String>) -> BTreeMap<String, u8> {
        let mut names = HashSet::new();
        for stmt in stmts {
            self.collect_captured_stmt(stmt, own, &mut names);
        }
        // 名字排序分配 cell_idx（稳定跨 run，父 MAKE_CELL 与子 upvalue 统一引用）
        let mut sorted: Vec<String> = names.into_iter().collect();
        sorted.sort();
        sorted.into_iter().enumerate().map(|(i, n)| (n, i as u8)).collect()
    }

    /// 只从嵌套函数节点进入扫描（本函数直接引用不算捕获）。
    fn collect_captured_stmt(&self, stmt: &Statement, own: &HashSet<String>, out: &mut HashSet<String>) {
        match stmt {
            Statement::FunctionDeclaration(fd) => {
                let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                self.collect_capture_names(body, own, out);
            }
            Statement::ExpressionStatement(es) => self.collect_captured_expr(&es.expression, own, out),
            Statement::ReturnStatement(rs) => {
                if let Some(a) = &rs.argument {
                    self.collect_captured_expr(a, own, out);
                }
            }
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    if let Some(init) = &d.init {
                        self.collect_captured_expr(init, own, out);
                    }
                }
            }
            Statement::IfStatement(is) => {
                self.collect_captured_expr(&is.test, own, out);
                self.collect_captured_stmt(&is.consequent, own, out);
                if let Some(alt) = &is.alternate {
                    self.collect_captured_stmt(alt, own, out);
                }
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.collect_captured_expr(e, own, out);
                    }
                }
                if let Some(t) = &fs.test {
                    self.collect_captured_expr(t, own, out);
                }
                if let Some(u) = &fs.update {
                    self.collect_captured_expr(u, own, out);
                }
                self.collect_captured_stmt(&fs.body, own, out);
            }
            Statement::WhileStatement(w) => {
                self.collect_captured_expr(&w.test, own, out);
                self.collect_captured_stmt(&w.body, own, out);
            }
            Statement::DoWhileStatement(d) => {
                self.collect_captured_stmt(&d.body, own, out);
                self.collect_captured_expr(&d.test, own, out);
            }
            Statement::ForInStatement(fi) => {
                self.collect_captured_expr(&fi.right, own, out);
                self.collect_captured_stmt(&fi.body, own, out);
            }
            Statement::ForOfStatement(fo) => {
                self.collect_captured_expr(&fo.right, own, out);
                self.collect_captured_stmt(&fo.body, own, out);
            }
            Statement::BlockStatement(b) => {
                for s in &b.body {
                    self.collect_captured_stmt(s, own, out);
                }
            }
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.collect_captured_stmt(s, own, out);
                }
                if let Some(h) = &ts.handler {
                    for s in &h.body.body {
                        self.collect_captured_stmt(s, own, out);
                    }
                }
                if let Some(f) = &ts.finalizer {
                    for s in &f.body {
                        self.collect_captured_stmt(s, own, out);
                    }
                }
            }
            Statement::ThrowStatement(ts) => self.collect_captured_expr(&ts.argument, own, out),
            Statement::SwitchStatement(sw) => {
                self.collect_captured_expr(&sw.discriminant, own, out);
                for case in &sw.cases {
                    for s in &case.consequent {
                        self.collect_captured_stmt(s, own, out);
                    }
                }
            }
            Statement::LabeledStatement(ls) => self.collect_captured_stmt(&ls.body, own, out),
            _ => {}
        }
    }

    fn collect_captured_expr(&self, expr: &Expression, own: &HashSet<String>, out: &mut HashSet<String>) {
        match expr {
            Expression::FunctionExpression(fe) => {
                let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let mut inner = self.collect_fn_param_names(&fe.params);
                inner.extend(self.collect_own_binding_names(&[], body));
                self.collect_capture_names_shadowed(body, own, &inner, out);
            }
            Expression::ArrowFunctionExpression(ae) => {
                let mut inner = self.collect_fn_param_names(&ae.params);
                inner.extend(self.collect_own_binding_names(&[], &ae.body.statements));
                self.collect_capture_names_shadowed(&ae.body.statements, own, &inner, out);
            }
            Expression::CallExpression(ce) => {
                self.collect_captured_expr(&ce.callee, own, out);
                for a in &ce.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_captured_expr(e, own, out);
                    }
                }
            }
            Expression::BinaryExpression(be) => {
                self.collect_captured_expr(&be.left, own, out);
                self.collect_captured_expr(&be.right, own, out);
            }
            Expression::UnaryExpression(ue) => self.collect_captured_expr(&ue.argument, own, out),
            Expression::LogicalExpression(le) => {
                self.collect_captured_expr(&le.left, own, out);
                self.collect_captured_expr(&le.right, own, out);
            }
            Expression::ConditionalExpression(ce) => {
                self.collect_captured_expr(&ce.test, own, out);
                self.collect_captured_expr(&ce.consequent, own, out);
                self.collect_captured_expr(&ce.alternate, own, out);
            }
            Expression::SequenceExpression(se) => {
                for e in &se.expressions {
                    self.collect_captured_expr(e, own, out);
                }
            }
            Expression::AssignmentExpression(ae) => self.collect_captured_expr(&ae.right, own, out),
            Expression::ArrayExpression(ae) => {
                for e in &ae.elements {
                    if let Some(e) = e.as_expression() {
                        self.collect_captured_expr(e, own, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// 分析子函数 body：引用父级被捕获绑定的名字 → upvalue_captures。
    /// cell_idx 直接取父 captured_bindings 映射（父 emit 前已确定，索引一致）；
    /// enclosing_reg 由父 emit 完成后填充（assemble_ir）。
    pub(crate) fn collect_upvalue_names(
        &self, body_stmts: &[Statement], parent_captured: &BTreeMap<String, u8>, sub_own: &HashSet<String>,
    ) -> Vec<UpvalueCapture> {
        let parent_names: HashSet<String> = parent_captured.keys().cloned().collect();
        let mut names = HashSet::new();
        self.collect_capture_names_shadowed(body_stmts, &parent_names, sub_own, &mut names);
        // HashSet 迭代序带随机种子（进程级非确定），必须排序使 upvalue_captures 的顺序与
        // 父 captured_bindings 的 cell_idx（BTreeMap 名字序）对齐——否则 LOAD_UPVALUE 的
        // a 槽 uv_idx 编码与 cell_idx 错位，读错 upvalue。
        let mut names: Vec<String> = names.into_iter().collect();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let cell_idx = parent_captured.get(&name).copied().unwrap_or(0);
                UpvalueCapture {
                    name,
                    enclosing_reg: 0, // assemble_ir 时从父符号表填充
                    cell_idx,
                }
            })
            .collect()
    }
}
