//! 闭包捕获分析（AST 级，emit 前完成，时序无关）。
//!
//! 收集本函数绑定的名字（own_bindings）与被嵌套函数捕获的名字
//! （captured_bindings → cell_idx，按名排序稳定跨 run），为 MAKE_CELL /
//! CELL_GET / CELL_SET 与子函数 upvalue cell_idx 提供统一依据。

use std::collections::{BTreeMap, HashSet};

use oxide_bytecode::module::UpvalueCapture;
use oxide_parser::{ClassBody, ClassElement, Expression, Statement};

use crate::Emitter;

impl Emitter {
    fn collect_fn_param_names(&self, params: &oxide_parser::FormalParameters) -> HashSet<String> {
        let mut names = HashSet::new();
        for p in &params.items {
            self.collect_binding_pattern_names(&p.pattern, &mut names);
        }
        // rest 形参也是本函数作用域绑定，遮蔽外层同名变量。
        if let Some(rest) = &params.rest {
            self.collect_binding_pattern_names(&rest.rest.argument, &mut names);
        }
        names
    }

    /// 递归收集 binding pattern 内全部绑定标识符名（解构 `[a, b]` / `{x: y}` 嵌套）。
    pub(crate) fn collect_binding_pattern_names(
        &self, pattern: &oxide_parser::BindingPattern, out: &mut HashSet<String>,
    ) {
        match pattern {
            oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                out.insert(bi.name.to_string());
            }
            oxide_parser::BindingPattern::ArrayPattern(ap) => {
                for p in ap.elements.iter().flatten() {
                    self.collect_binding_pattern_names(p, out);
                }
                if let Some(rest) = &ap.rest {
                    self.collect_binding_pattern_names(&rest.argument, out);
                }
            }
            oxide_parser::BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.collect_binding_pattern_names(&prop.value, out);
                }
                if let Some(rest) = &op.rest {
                    self.collect_binding_pattern_names(&rest.argument, out);
                }
            }
            oxide_parser::BindingPattern::AssignmentPattern(ap) => {
                self.collect_binding_pattern_names(&ap.left, out);
            }
        }
    }

    /// 收集 for-in/for-of 头部声明（`var x` / 解构 pattern）绑定的名字。
    fn collect_for_left_decl_names(&self, left: &oxide_parser::ForStatementLeft, out: &mut HashSet<String>) {
        if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = left {
            for d in &vd.declarations {
                self.collect_binding_pattern_names(&d.id, out);
            }
        }
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
                        self.collect_binding_pattern_names(&d.id, out);
                    }
                }
                Statement::FunctionDeclaration(fd) => {
                    if let Some(id) = &fd.id {
                        out.insert(id.name.to_string());
                    }
                }
                // import 绑定是模块作用域 const 绑定，须计入 own_bindings 供
                // 嵌套函数 cell 捕获。
                Statement::ImportDeclaration(imp) => {
                    if let Some(specifiers) = &imp.specifiers {
                        for sp in specifiers {
                            match sp {
                                oxide_parser::ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                    out.insert(s.local.name.to_string());
                                }
                                oxide_parser::ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                    out.insert(s.local.name.to_string());
                                }
                                oxide_parser::ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                    out.insert(s.local.name.to_string());
                                }
                            }
                        }
                    }
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(decl) = &exp.declaration {
                        match decl {
                            oxide_parser::Declaration::VariableDeclaration(vd) => {
                                for d in &vd.declarations {
                                    self.collect_binding_pattern_names(&d.id, out);
                                }
                            }
                            oxide_parser::Declaration::FunctionDeclaration(fd) => {
                                if let Some(id) = &fd.id {
                                    out.insert(id.name.to_string());
                                }
                            }
                            oxide_parser::Declaration::ClassDeclaration(cd) => {
                                if let Some(id) = &cd.id {
                                    out.insert(id.name.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
                    oxide_parser::ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                        if let Some(id) = &fd.id {
                            out.insert(id.name.to_string());
                        }
                    }
                    oxide_parser::ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                        if let Some(id) = &cd.id {
                            out.insert(id.name.to_string());
                        }
                    }
                    _ => {}
                },
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
                // for-in / for-of 头部的 var 声明是本函数局部绑定（遮蔽外层同名），
                // 须计入 own_bindings，否则闭包捕获分析会误判为捕获外层变量。
                Statement::ForInStatement(fi) => {
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(_) = &fi.left {
                        self.collect_for_left_decl_names(&fi.left, out);
                    }
                    self.collect_decl_names_stmt(std::slice::from_ref(&fi.body), out);
                }
                Statement::ForOfStatement(fo) => {
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(_) = &fo.left {
                        self.collect_for_left_decl_names(&fo.left, out);
                    }
                    self.collect_decl_names_stmt(std::slice::from_ref(&fo.body), out);
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
                Statement::WithStatement(ws) => self.collect_decl_names_stmt(std::slice::from_ref(&ws.body), out),
                _ => {}
            }
        }
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
                self.collect_fn_default_names(&fd.params, ref_set, shadow, out);
                self.collect_capture_names_shadowed(body, ref_set, &inner, out);
            }
            Statement::ClassDeclaration(cd) => {
                // 类体（构造器/方法体、字段键与值、静态块）是嵌套作用域：
                // 类名遮蔽外层绑定，方法形参与方法体局部进一步遮蔽。
                let mut class_shadow = shadow.clone();
                if let Some(id) = &cd.id {
                    class_shadow.insert(id.name.as_str().to_string());
                }
                self.collect_class_capture_names(&cd.body, ref_set, &class_shadow, out);
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
                // left 的 var 声明遮蔽外层同名绑定，body 内引用不视为捕获外层。
                let mut for_shadow = shadow.clone();
                self.collect_for_left_decl_names(&fi.left, &mut for_shadow);
                self.collect_capture_names_shadowed(std::slice::from_ref(&fi.body), ref_set, &for_shadow, out);
            }
            Statement::ForOfStatement(fo) => {
                self.collect_capture_names_expr(&fo.right, ref_set, shadow, out);
                let mut for_shadow = shadow.clone();
                self.collect_for_left_decl_names(&fo.left, &mut for_shadow);
                self.collect_capture_names_shadowed(std::slice::from_ref(&fo.body), ref_set, &for_shadow, out);
            }
            Statement::BlockStatement(b) => self.collect_capture_names_shadowed(&b.body, ref_set, shadow, out),
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.collect_capture_names_stmt(s, ref_set, shadow, out);
                }
                if let Some(h) = &ts.handler {
                    // catch 参数解构模式内的表达式引用（默认值等）也会被内部闭包捕获；
                    // 参数声明名遮蔽 catch 作用域。
                    let mut catch_shadow = shadow.clone();
                    if let Some(param) = &h.param {
                        self.collect_capture_names_binding_pattern(
                            &param.pattern,
                            ref_set,
                            shadow,
                            out,
                            &mut catch_shadow,
                        );
                    }
                    for s in &h.body.body {
                        self.collect_capture_names_stmt(s, ref_set, &catch_shadow, out);
                    }
                }
                if let Some(f) = &ts.finalizer {
                    for s in &f.body {
                        self.collect_capture_names_stmt(s, ref_set, shadow, out);
                    }
                }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(decl) = &exp.declaration {
                    match decl {
                        oxide_parser::Declaration::VariableDeclaration(vd) => {
                            for d in &vd.declarations {
                                if let Some(init) = &d.init {
                                    self.collect_capture_names_expr(init, ref_set, shadow, out);
                                }
                            }
                        }
                        oxide_parser::Declaration::FunctionDeclaration(fd) => {
                            let body: &[Statement] =
                                fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                            let mut inner = shadow.clone();
                            inner.extend(self.collect_fn_param_names(&fd.params));
                            inner.extend(self.collect_own_binding_names(&[], body));
                            self.collect_fn_default_names(&fd.params, ref_set, shadow, out);
                            self.collect_capture_names_shadowed(body, ref_set, &inner, out);
                        }
                        oxide_parser::Declaration::ClassDeclaration(cd) => {
                            let mut class_shadow = shadow.clone();
                            if let Some(id) = &cd.id {
                                class_shadow.insert(id.name.as_str().to_string());
                            }
                            self.collect_class_capture_names(&cd.body, ref_set, &class_shadow, out);
                        }
                        _ => {}
                    }
                }
            }
            Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
                oxide_parser::ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                    let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                    let mut inner = shadow.clone();
                    inner.extend(self.collect_fn_param_names(&fd.params));
                    inner.extend(self.collect_own_binding_names(&[], body));
                    self.collect_fn_default_names(&fd.params, ref_set, shadow, out);
                    self.collect_capture_names_shadowed(body, ref_set, &inner, out);
                }
                oxide_parser::ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                    let mut class_shadow = shadow.clone();
                    if let Some(id) = &cd.id {
                        class_shadow.insert(id.name.as_str().to_string());
                    }
                    self.collect_class_capture_names(&cd.body, ref_set, &class_shadow, out);
                }
                other => {
                    if let Some(e) = other.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            },
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
            Statement::WithStatement(ws) => {
                self.collect_capture_names_expr(&ws.object, ref_set, shadow, out);
                self.collect_capture_names_stmt(&ws.body, ref_set, shadow, out);
            }
            _ => {}
        }
    }

    /// 收集类体对 `ref_set` 的引用：方法/构造器体、字段键与值、静态块。
    /// 方法形参与方法体局部声明遮蔽外层绑定（捕获判定用）。
    fn collect_class_capture_names(
        &self, class_body: &ClassBody, ref_set: &HashSet<String>, class_shadow: &HashSet<String>,
        out: &mut HashSet<String>,
    ) {
        for element in &class_body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let mut inner = class_shadow.clone();
                    inner.extend(self.collect_fn_param_names(&method.value.params));
                    let body: &[Statement] = method.value.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                    inner.extend(self.collect_own_binding_names(&[], body));
                    self.collect_fn_default_names(&method.value.params, ref_set, class_shadow, out);
                    self.collect_capture_names_shadowed(body, ref_set, &inner, out);
                }
                ClassElement::PropertyDefinition(prop) => {
                    if let Some(expr) = prop.key.as_expression() {
                        self.collect_capture_names_expr(expr, ref_set, class_shadow, out);
                    }
                    if let Some(value) = &prop.value {
                        self.collect_capture_names_expr(value, ref_set, class_shadow, out);
                    }
                }
                ClassElement::StaticBlock(block) => {
                    self.collect_capture_names_shadowed(&block.body, ref_set, class_shadow, out);
                }
                _ => {}
            }
        }
    }

    /// 遍历 catch 参数解构模式：收集模式内表达式引用（默认值等），声明名写入
    /// `catch_shadow` 遮蔽 catch 作用域。
    fn collect_capture_names_binding_pattern(
        &self, pattern: &oxide_parser::BindingPattern, ref_set: &HashSet<String>, shadow: &HashSet<String>,
        out: &mut HashSet<String>, catch_shadow: &mut HashSet<String>,
    ) {
        match pattern {
            oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                catch_shadow.insert(bi.name.as_str().to_string());
            }
            oxide_parser::BindingPattern::ArrayPattern(ap) => {
                for elem in &ap.elements {
                    if let Some(p) = elem {
                        self.collect_capture_names_binding_pattern(p, ref_set, shadow, out, catch_shadow);
                    }
                }
                if let Some(rest) = &ap.rest {
                    self.collect_capture_names_binding_pattern(&rest.argument, ref_set, shadow, out, catch_shadow);
                }
            }
            oxide_parser::BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.collect_capture_names_binding_pattern(&prop.value, ref_set, shadow, out, catch_shadow);
                }
                if let Some(rest) = &op.rest {
                    self.collect_capture_names_binding_pattern(&rest.argument, ref_set, shadow, out, catch_shadow);
                }
            }
            oxide_parser::BindingPattern::AssignmentPattern(ap) => {
                self.collect_capture_names_expr(&ap.right, ref_set, shadow, out);
                self.collect_capture_names_binding_pattern(&ap.left, ref_set, shadow, out, catch_shadow);
            }
        }
    }

    /// 扫描赋值目标中的引用（标识符写名，成员目标扫对象/键，解构目标递归元素）。
    fn collect_capture_names_assign_target(
        &self, target: &oxide_parser::AssignmentTarget, ref_set: &HashSet<String>, shadow: &HashSet<String>,
        out: &mut HashSet<String>,
    ) {
        match target {
            oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(ati) => {
                let name = ati.name.as_str();
                if ref_set.contains(name) && !shadow.contains(name) {
                    out.insert(ati.name.to_string());
                }
            }
            oxide_parser::AssignmentTarget::StaticMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
            }
            oxide_parser::AssignmentTarget::ComputedMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
                self.collect_capture_names_expr(&m.expression, ref_set, shadow, out);
            }
            oxide_parser::AssignmentTarget::ArrayAssignmentTarget(a) => {
                for elem in &a.elements {
                    if let Some(e) = elem {
                        self.collect_capture_names_maybe_default_target(e, ref_set, shadow, out);
                    }
                }
                if let Some(rest) = &a.rest {
                    self.collect_capture_names_assign_target(&rest.target, ref_set, shadow, out);
                }
            }
            oxide_parser::AssignmentTarget::ObjectAssignmentTarget(o) => {
                for prop in &o.properties {
                    if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) = prop {
                        if ref_set.contains(id.binding.name.as_str()) && !shadow.contains(id.binding.name.as_str()) {
                            out.insert(id.binding.name.to_string());
                        }
                        if let Some(init) = &id.init {
                            self.collect_capture_names_expr(init, ref_set, shadow, out);
                        }
                    } else if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) = prop {
                        if let Some(name_expr) = p.name.as_expression() {
                            self.collect_capture_names_expr(name_expr, ref_set, shadow, out);
                        }
                        self.collect_capture_names_maybe_default_target(&p.binding, ref_set, shadow, out);
                    }
                }
                if let Some(rest) = &o.rest {
                    self.collect_capture_names_assign_target(&rest.target, ref_set, shadow, out);
                }
            }
            _ => {}
        }
    }

    /// 解构赋值元素可能是 `AssignmentTarget` 或带默认值的包装（后者多一层 `init`）。
    fn collect_capture_names_maybe_default_target(
        &self, target: &oxide_parser::AssignmentTargetMaybeDefault, ref_set: &HashSet<String>,
        shadow: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        use oxide_parser::AssignmentTargetMaybeDefault as MaybeDefault;
        match target {
            MaybeDefault::AssignmentTargetWithDefault(d) => {
                self.collect_capture_names_expr(&d.init, ref_set, shadow, out);
                self.collect_capture_names_assign_target(&d.binding, ref_set, shadow, out);
            }
            MaybeDefault::AssignmentTargetIdentifier(ati) => {
                let name = ati.name.as_str();
                if ref_set.contains(name) && !shadow.contains(name) {
                    out.insert(ati.name.to_string());
                }
            }
            MaybeDefault::StaticMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
            }
            MaybeDefault::ComputedMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
                self.collect_capture_names_expr(&m.expression, ref_set, shadow, out);
            }
            MaybeDefault::ArrayAssignmentTarget(a) => {
                for elem in &a.elements {
                    if let Some(e) = elem {
                        self.collect_capture_names_maybe_default_target(e, ref_set, shadow, out);
                    }
                }
            }
            MaybeDefault::ObjectAssignmentTarget(o) => {
                for prop in &o.properties {
                    if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) = prop {
                        if ref_set.contains(id.binding.name.as_str()) && !shadow.contains(id.binding.name.as_str()) {
                            out.insert(id.binding.name.to_string());
                        }
                        if let Some(init) = &id.init {
                            self.collect_capture_names_expr(init, ref_set, shadow, out);
                        }
                    } else if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) = prop {
                        if let Some(name_expr) = p.name.as_expression() {
                            self.collect_capture_names_expr(name_expr, ref_set, shadow, out);
                        }
                        self.collect_capture_names_maybe_default_target(&p.binding, ref_set, shadow, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// 扫描一元更新目标（`++x` / `obj.x++`）中的引用。
    fn collect_capture_names_simple_target(
        &self, target: &oxide_parser::SimpleAssignmentTarget, ref_set: &HashSet<String>, shadow: &HashSet<String>,
        out: &mut HashSet<String>,
    ) {
        match target {
            oxide_parser::SimpleAssignmentTarget::AssignmentTargetIdentifier(ati) => {
                let name = ati.name.as_str();
                if ref_set.contains(name) && !shadow.contains(name) {
                    out.insert(ati.name.to_string());
                }
            }
            oxide_parser::SimpleAssignmentTarget::StaticMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
            }
            oxide_parser::SimpleAssignmentTarget::ComputedMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
                self.collect_capture_names_expr(&m.expression, ref_set, shadow, out);
            }
            _ => {}
        }
    }

    /// 收集函数参数默认值表达式里的引用（子层 upvalue 判定；参数名遮蔽）。
    fn collect_fn_default_names(
        &self, params: &oxide_parser::FormalParameters, ref_set: &HashSet<String>, shadow: &HashSet<String>,
        out: &mut HashSet<String>,
    ) {
        for p in &params.items {
            if let Some(init) = &p.initializer {
                self.collect_capture_names_expr(init, ref_set, shadow, out);
            }
            let mut param_shadow = shadow.clone();
            self.collect_capture_names_binding_pattern(&p.pattern, ref_set, shadow, out, &mut param_shadow);
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
                // 赋值目标里的引用也须捕获：标识符目标写名，成员/解构目标扫描其对象/元素。
                self.collect_capture_names_assign_target(&ae.left, ref_set, shadow, out);
                self.collect_capture_names_expr(&ae.right, ref_set, shadow, out);
            }
            Expression::UpdateExpression(ue) => {
                self.collect_capture_names_simple_target(&ue.argument, ref_set, shadow, out);
            }
            Expression::FunctionExpression(fe) => {
                let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let mut inner = shadow.clone();
                inner.extend(self.collect_fn_param_names(&fe.params));
                inner.extend(self.collect_own_binding_names(&[], body));
                self.collect_fn_default_names(&fe.params, ref_set, shadow, out);
                self.collect_capture_names_shadowed(body, ref_set, &inner, out);
            }
            Expression::ArrowFunctionExpression(ae) => {
                let mut inner = shadow.clone();
                inner.extend(self.collect_fn_param_names(&ae.params));
                inner.extend(self.collect_own_binding_names(&[], &ae.body.statements));
                self.collect_fn_default_names(&ae.params, ref_set, shadow, out);
                self.collect_capture_names_shadowed(&ae.body.statements, ref_set, &inner, out);
            }
            Expression::ClassExpression(class) => {
                // 类表达式：方法/构造器体与字段表达式是嵌套作用域，引用须捕获。
                let mut class_shadow = shadow.clone();
                if let Some(id) = &class.id {
                    class_shadow.insert(id.name.as_str().to_string());
                }
                self.collect_class_capture_names(&class.body, ref_set, &class_shadow, out);
            }
            Expression::BinaryExpression(be) => {
                self.collect_capture_names_expr(&be.left, ref_set, shadow, out);
                self.collect_capture_names_expr(&be.right, ref_set, shadow, out);
            }
            Expression::UnaryExpression(ue) => self.collect_capture_names_expr(&ue.argument, ref_set, shadow, out),
            Expression::CallExpression(ce) => {
                self.collect_capture_names_expr(&ce.callee, ref_set, shadow, out);
                for a in &ce.arguments {
                    self.collect_capture_names_arg(a, ref_set, shadow, out);
                }
            }
            Expression::NewExpression(ne) => {
                self.collect_capture_names_expr(&ne.callee, ref_set, shadow, out);
                for a in &ne.arguments {
                    self.collect_capture_names_arg(a, ref_set, shadow, out);
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
                    match prop {
                        oxide_parser::ObjectPropertyKind::ObjectProperty(p) => {
                            if p.computed {
                                self.collect_capture_names_expr(p.key.to_expression(), ref_set, shadow, out);
                            }
                            self.collect_capture_names_expr(&p.value, ref_set, shadow, out);
                        }
                        oxide_parser::ObjectPropertyKind::SpreadProperty(spread) => {
                            self.collect_capture_names_expr(&spread.argument, ref_set, shadow, out);
                        }
                    }
                }
            }
            // 生成器让出表达式：被让出的值里引用的父变量须纳入捕获，否则生成器体经
            // LOAD_VAR 读调用方寄存器残留（挂起恢复后寄存器已被覆盖）。
            Expression::YieldExpression(ye) => {
                if let Some(a) = &ye.argument {
                    self.collect_capture_names_expr(a, ref_set, shadow, out);
                }
            }
            // await 表达式：被等待的值里引用的父变量须纳入捕获（与 yield 同因——
            // 异步体挂起恢复后寄存器已被覆盖，只能经 cell 读取）。
            Expression::AwaitExpression(ae) => {
                self.collect_capture_names_expr(&ae.argument, ref_set, shadow, out);
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
                    self.collect_capture_names_arg(a, ref_set, shadow, out);
                }
            }
            _ => {}
        }
    }

    /// 遍历调用实参：静态实参与 spread 内部表达式都纳入捕获扫描。
    fn collect_capture_names_arg(
        &self, arg: &oxide_parser::Argument, ref_set: &HashSet<String>, shadow: &HashSet<String>,
        out: &mut HashSet<String>,
    ) {
        if let Some(e) = arg.as_expression() {
            self.collect_capture_names_expr(e, ref_set, shadow, out);
        } else if let oxide_parser::Argument::SpreadElement(sp) = arg {
            self.collect_capture_names_expr(&sp.argument, ref_set, shadow, out);
        }
    }

    /// 分析本函数：哪些绑定被任意深度嵌套函数捕获 → captured_bindings。
    pub(crate) fn collect_captured_bindings(
        &self, stmts: &[Statement], extra_exprs: &[&oxide_parser::Expression], own: &HashSet<String>,
    ) -> BTreeMap<String, u8> {
        let mut names = HashSet::new();
        for stmt in stmts {
            self.collect_captured_stmt(stmt, own, &mut names);
        }
        // 参数默认值表达式（不在 body_stmts 内）里的嵌套函数引用也要纳入捕获，
        // 否则默认值内 IIFE 引用全局/外层变量走 LOAD_VAR 读寄存器残留（B022 扩展）。
        for expr in extra_exprs {
            self.collect_captured_expr(expr, own, &mut names);
        }
        // 名字排序分配 cell_idx（稳定跨 run，父 MAKE_CELL 与子 upvalue 统一引用）
        let mut sorted: Vec<String> = names.into_iter().collect();
        sorted.sort();
        sorted.into_iter().enumerate().map(|(i, n)| (n, i as u8)).collect()
    }

    /// 收集函数参数默认值表达式里的嵌套函数引用（父层 captured/子层 upvalue 判定）。
    fn collect_fn_default_captured(
        &self, params: &oxide_parser::FormalParameters, own: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        for p in &params.items {
            if let Some(init) = &p.initializer {
                self.collect_captured_expr(init, own, out);
            }
            self.collect_captured_binding_pattern(&p.pattern, own, out);
        }
    }

    /// 只从嵌套函数节点进入扫描（本函数直接引用不算捕获）。
    fn collect_captured_stmt(&self, stmt: &Statement, own: &HashSet<String>, out: &mut HashSet<String>) {
        match stmt {
            Statement::FunctionDeclaration(fd) => {
                let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                self.collect_fn_default_captured(&fd.params, own, out);
                // 函数参数与体内部声明遮蔽父级绑定：体内对这些名字的引用不算捕获父级。
                let mut fn_shadow = HashSet::new();
                fn_shadow.extend(self.collect_fn_param_names(&fd.params));
                fn_shadow.extend(self.collect_own_binding_names(&[], body));
                self.collect_capture_names_shadowed(body, own, &fn_shadow, out);
            }
            Statement::ClassDeclaration(cd) => {
                // 类构造器/方法体与字段表达式引用的父级绑定须建 cell，供子模块 upvalue 捕获。
                let mut class_shadow = HashSet::new();
                if let Some(id) = &cd.id {
                    class_shadow.insert(id.name.as_str().to_string());
                }
                self.collect_class_capture_names(&cd.body, own, &class_shadow, out);
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
                    if let Some(param) = &h.param {
                        self.collect_captured_binding_pattern(&param.pattern, own, out);
                    }
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
            Statement::WithStatement(ws) => {
                self.collect_captured_expr(&ws.object, own, out);
                self.collect_captured_stmt(&ws.body, own, out);
            }
            _ => {}
        }
    }

    /// 遍历 catch 参数解构模式内表达式引用（默认值等），供父层 MAKE_CELL 判定。
    fn collect_captured_binding_pattern(
        &self, pattern: &oxide_parser::BindingPattern, own: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        match pattern {
            oxide_parser::BindingPattern::BindingIdentifier(_) => {}
            oxide_parser::BindingPattern::ArrayPattern(ap) => {
                for elem in &ap.elements {
                    if let Some(p) = elem {
                        self.collect_captured_binding_pattern(p, own, out);
                    }
                }
                if let Some(rest) = &ap.rest {
                    self.collect_captured_binding_pattern(&rest.argument, own, out);
                }
            }
            oxide_parser::BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.collect_captured_binding_pattern(&prop.value, own, out);
                }
                if let Some(rest) = &op.rest {
                    self.collect_captured_binding_pattern(&rest.argument, own, out);
                }
            }
            oxide_parser::BindingPattern::AssignmentPattern(ap) => {
                self.collect_captured_expr(&ap.right, own, out);
                self.collect_captured_binding_pattern(&ap.left, own, out);
            }
        }
    }

    fn collect_captured_expr(&self, expr: &Expression, own: &HashSet<String>, out: &mut HashSet<String>) {
        match expr {
            Expression::FunctionExpression(fe) => {
                let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let mut inner = self.collect_fn_param_names(&fe.params);
                inner.extend(self.collect_own_binding_names(&[], body));
                self.collect_fn_default_captured(&fe.params, own, out);
                self.collect_capture_names_shadowed(body, own, &inner, out);
            }
            Expression::ArrowFunctionExpression(ae) => {
                let mut inner = self.collect_fn_param_names(&ae.params);
                inner.extend(self.collect_own_binding_names(&[], &ae.body.statements));
                self.collect_fn_default_captured(&ae.params, own, out);
                self.collect_capture_names_shadowed(&ae.body.statements, own, &inner, out);
            }
            Expression::ClassExpression(class) => {
                // 类表达式：构造器/方法体与字段表达式引用的父级绑定须建 cell。
                let mut class_shadow = HashSet::new();
                if let Some(id) = &class.id {
                    class_shadow.insert(id.name.as_str().to_string());
                }
                self.collect_class_capture_names(&class.body, own, &class_shadow, out);
            }
            Expression::CallExpression(ce) => {
                self.collect_captured_expr(&ce.callee, own, out);
                for a in &ce.arguments {
                    self.collect_captured_arg(a, own, out);
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
            Expression::AssignmentExpression(ae) => {
                // 只扫右侧：左侧赋值目标是本函数绑定，不引用嵌套函数；
                // 若目标含嵌套函数表达式（如 `[f()] = ...`），由其自身递归处理。
                self.collect_captured_expr(&ae.right, own, out);
            }
            Expression::UpdateExpression(_ue) => {} // 一元更新目标不引用嵌套函数，无需捕获
            Expression::ArrayExpression(ae) => {
                for e in &ae.elements {
                    if let Some(e) = e.as_expression() {
                        self.collect_captured_expr(e, own, out);
                    }
                }
            }
            Expression::ObjectExpression(o) => {
                for prop in &o.properties {
                    match prop {
                        oxide_parser::ObjectPropertyKind::ObjectProperty(p) => {
                            if p.computed {
                                self.collect_captured_expr(p.key.to_expression(), own, out);
                            }
                            self.collect_captured_expr(&p.value, own, out);
                        }
                        oxide_parser::ObjectPropertyKind::SpreadProperty(spread) => {
                            self.collect_captured_expr(&spread.argument, own, out);
                        }
                    }
                }
            }
            Expression::NewExpression(ne) => {
                self.collect_captured_expr(&ne.callee, own, out);
                for a in &ne.arguments {
                    self.collect_captured_arg(a, own, out);
                }
            }
            Expression::ComputedMemberExpression(m) => {
                self.collect_captured_expr(&m.object, own, out);
                self.collect_captured_expr(&m.expression, own, out);
            }
            Expression::StaticMemberExpression(m) => self.collect_captured_expr(&m.object, own, out),
            Expression::PrivateFieldExpression(m) => self.collect_captured_expr(&m.object, own, out),
            Expression::TemplateLiteral(tl) => {
                for e in &tl.expressions {
                    self.collect_captured_expr(e, own, out);
                }
            }
            Expression::TaggedTemplateExpression(tt) => {
                self.collect_captured_expr(&tt.tag, own, out);
                for e in &tt.quasi.expressions {
                    self.collect_captured_expr(e, own, out);
                }
            }
            Expression::ParenthesizedExpression(p) => self.collect_captured_expr(&p.expression, own, out),
            Expression::YieldExpression(ye) => {
                if let Some(a) = &ye.argument {
                    self.collect_captured_expr(a, own, out);
                }
            }
            Expression::AwaitExpression(ae) => self.collect_captured_expr(&ae.argument, own, out),
            Expression::ChainExpression(c) => self.collect_captured_chain(&c.expression, own, out),
            _ => {}
        }
    }

    fn collect_captured_chain(
        &self, element: &oxide_parser::ChainElement, own: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        match element {
            oxide_parser::ChainElement::StaticMemberExpression(m) => {
                self.collect_captured_expr(&m.object, own, out);
            }
            oxide_parser::ChainElement::ComputedMemberExpression(m) => {
                self.collect_captured_expr(&m.object, own, out);
                self.collect_captured_expr(&m.expression, own, out);
            }
            oxide_parser::ChainElement::PrivateFieldExpression(m) => {
                self.collect_captured_expr(&m.object, own, out);
            }
            oxide_parser::ChainElement::CallExpression(c) => {
                self.collect_captured_expr(&c.callee, own, out);
                for a in &c.arguments {
                    self.collect_captured_arg(a, own, out);
                }
            }
            _ => {}
        }
    }

    /// 遍历调用实参：静态实参与 spread 内部表达式都纳入捕获判定。
    fn collect_captured_arg(&self, arg: &oxide_parser::Argument, own: &HashSet<String>, out: &mut HashSet<String>) {
        if let Some(e) = arg.as_expression() {
            self.collect_captured_expr(e, own, out);
        } else if let oxide_parser::Argument::SpreadElement(sp) = arg {
            self.collect_captured_expr(&sp.argument, own, out);
        }
    }

    /// 分析子函数 body：引用父级被捕获绑定的名字 → upvalue_captures。
    /// 名字若在父 `captured_bindings`（父 own cell）则 `cell_idx` 索引定义方表；
    /// 若在父 `upvalue_captures`（父自身从更外层捕获）则链式标记 `parent_uv_idx`，
    /// 运行时从父闭包 upvalues 取 cell。enclosing_reg 由父 emit 完成后填充。
    pub(crate) fn collect_upvalue_names(
        &self, body_stmts: &[Statement], extra_exprs: &[&oxide_parser::Expression],
        parent_captured: &BTreeMap<String, u8>, parent_upvalues: &[UpvalueCapture], sub_own: &HashSet<String>,
    ) -> Vec<UpvalueCapture> {
        let mut parent_names: HashSet<String> = parent_captured.keys().cloned().collect();
        for u in parent_upvalues {
            parent_names.insert(u.name.clone());
        }
        let mut names = HashSet::new();
        self.collect_capture_names_shadowed(body_stmts, &parent_names, sub_own, &mut names);
        // 参数默认值表达式（不在 body_stmts）里的嵌套函数引用也会捕获父变量。
        for expr in extra_exprs {
            self.collect_capture_names_expr(expr, &parent_names, sub_own, &mut names);
        }
        // HashSet 迭代序带随机种子（进程级非确定），必须排序使 upvalue_captures 的顺序与
        // 父 captured_bindings 的 cell_idx（BTreeMap 名字序）对齐——否则 LOAD_UPVALUE 的
        // a 槽 uv_idx 编码与 cell_idx 错位，读错 upvalue。
        let mut names: Vec<String> = names.into_iter().collect();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let parent_uv_idx = parent_upvalues.iter().position(|u| u.name == name).map(|i| i as u8);
                let cell_idx = parent_captured.get(&name).copied().unwrap_or(0);
                UpvalueCapture {
                    name,
                    enclosing_reg: 0, // assemble_ir 时从父符号表填充
                    cell_idx,
                    parent_uv_idx,
                }
            })
            .collect()
    }
}
