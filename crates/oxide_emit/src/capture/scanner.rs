//! 闭包捕获分析，遮蔽感知引用收集：ref_set × shadow 双集驱动，遍历
//! 语句/表达式/赋值目标/类体 AST，嵌套函数/类/for 头/catch 参数逐层
//! 遮蔽，收集任意深度嵌套函数引用的名字。

use std::collections::HashSet;

use oxide_parser::{ClassBody, ClassElement, Expression, Statement};

use super::names::collect_own_binding_names;
use super::{collect_fn_param_names, collect_for_left_decl_names};

/// 以 `ref_set`（关注名集）× `shadow`（遮蔽名集）双集驱动，收集被任意深度嵌套
/// 函数引用、且未被遮蔽的名字写入 `out`。本族带显式 `ref_set`/`shadow` 参数，按
/// 过滤条件收集引用名；`captured.rs` 的 `collect_captured_*` 族则以本函数 own 绑定集
/// 为过滤条件做捕获最终判定。
pub(crate) fn collect_capture_names_shadowed(
    stmts: &[Statement], ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
) {
    for stmt in stmts {
        collect_capture_names_stmt(stmt, ref_set, shadow, out);
    }
}

/// 按语句类型递归扫描引用；进入嵌套函数/类/for 头/catch 时逐层扩充遮蔽集。
pub(crate) fn collect_capture_names_stmt(
    stmt: &Statement, ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
) {
    match stmt {
        Statement::ExpressionStatement(es) => collect_capture_names_expr(&es.expression, ref_set, shadow, out),
        Statement::ReturnStatement(rs) => {
            if let Some(a) = &rs.argument {
                collect_capture_names_expr(a, ref_set, shadow, out);
            }
        }
        Statement::VariableDeclaration(vd) => {
            for d in &vd.declarations {
                // 绑定 pattern 的计算键是运行时求值表达式，其引用须纳入捕获。
                collect_capture_names_binding_keys(&d.id, ref_set, shadow, out);
                if let Some(init) = &d.init {
                    collect_capture_names_expr(init, ref_set, shadow, out);
                }
            }
        }
        Statement::FunctionDeclaration(fd) => {
            let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
            let mut inner = shadow.clone();
            inner.extend(collect_fn_param_names(&fd.params));
            inner.extend(collect_own_binding_names(&[], body));
            collect_fn_default_names(&fd.params, ref_set, shadow, out);
            collect_capture_names_shadowed(body, ref_set, &inner, out);
        }
        Statement::ClassDeclaration(cd) => {
            // 类体（构造器/方法体、字段键与值、静态块）是嵌套作用域：
            // 类名遮蔽外层绑定，方法形参与方法体局部进一步遮蔽。
            let mut class_shadow = shadow.clone();
            if let Some(id) = &cd.id {
                class_shadow.insert(id.name.as_str().to_string());
            }
            collect_class_capture_names(&cd.body, ref_set, &class_shadow, out);
        }
        Statement::IfStatement(is) => {
            collect_capture_names_expr(&is.test, ref_set, shadow, out);
            collect_capture_names_stmt(&is.consequent, ref_set, shadow, out);
            if let Some(alt) = &is.alternate {
                collect_capture_names_stmt(alt, ref_set, shadow, out);
            }
        }
        Statement::ForStatement(fs) => {
            if let Some(init) = &fs.init {
                if let Some(e) = init.as_expression() {
                    collect_capture_names_expr(e, ref_set, shadow, out);
                }
                if let oxide_parser::ForStatementInit::VariableDeclaration(vd) = init {
                    for d in &vd.declarations {
                        collect_capture_names_binding_keys(&d.id, ref_set, shadow, out);
                        if let Some(i) = &d.init {
                            collect_capture_names_expr(i, ref_set, shadow, out);
                        }
                    }
                }
            }
            if let Some(t) = &fs.test {
                collect_capture_names_expr(t, ref_set, shadow, out);
            }
            if let Some(u) = &fs.update {
                collect_capture_names_expr(u, ref_set, shadow, out);
            }
            collect_capture_names_stmt(&fs.body, ref_set, shadow, out);
        }
        Statement::WhileStatement(w) => {
            collect_capture_names_expr(&w.test, ref_set, shadow, out);
            collect_capture_names_stmt(&w.body, ref_set, shadow, out);
        }
        Statement::DoWhileStatement(d) => {
            collect_capture_names_stmt(&d.body, ref_set, shadow, out);
            collect_capture_names_expr(&d.test, ref_set, shadow, out);
        }
        Statement::ForInStatement(fi) => {
            collect_capture_names_expr(&fi.right, ref_set, shadow, out);
            // left 的 var 声明遮蔽外层同名绑定，body 内引用不视为捕获外层。
            let mut for_shadow = shadow.clone();
            collect_for_left_decl_names(&fi.left, &mut for_shadow);
            if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fi.left {
                for d in &vd.declarations {
                    collect_capture_names_binding_keys(&d.id, ref_set, shadow, out);
                }
            }
            collect_capture_names_shadowed(std::slice::from_ref(&fi.body), ref_set, &for_shadow, out);
        }
        Statement::ForOfStatement(fo) => {
            collect_capture_names_expr(&fo.right, ref_set, shadow, out);
            let mut for_shadow = shadow.clone();
            collect_for_left_decl_names(&fo.left, &mut for_shadow);
            if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fo.left {
                for d in &vd.declarations {
                    collect_capture_names_binding_keys(&d.id, ref_set, shadow, out);
                }
            }
            collect_capture_names_shadowed(std::slice::from_ref(&fo.body), ref_set, &for_shadow, out);
        }
        Statement::BlockStatement(b) => collect_capture_names_shadowed(&b.body, ref_set, shadow, out),
        Statement::TryStatement(ts) => {
            for s in &ts.block.body {
                collect_capture_names_stmt(s, ref_set, shadow, out);
            }
            if let Some(h) = &ts.handler {
                // catch 参数解构模式内的表达式引用（默认值等）也会被内部闭包捕获；
                // 参数声明名遮蔽 catch 作用域。
                let mut catch_shadow = shadow.clone();
                if let Some(param) = &h.param {
                    collect_capture_names_binding_pattern(&param.pattern, ref_set, shadow, out, &mut catch_shadow);
                }
                for s in &h.body.body {
                    collect_capture_names_stmt(s, ref_set, &catch_shadow, out);
                }
            }
            if let Some(f) = &ts.finalizer {
                for s in &f.body {
                    collect_capture_names_stmt(s, ref_set, shadow, out);
                }
            }
        }
        Statement::ExportNamedDeclaration(exp) => {
            if let Some(decl) = &exp.declaration {
                match decl {
                    oxide_parser::Declaration::VariableDeclaration(vd) => {
                        for d in &vd.declarations {
                            collect_capture_names_binding_keys(&d.id, ref_set, shadow, out);
                            if let Some(init) = &d.init {
                                collect_capture_names_expr(init, ref_set, shadow, out);
                            }
                        }
                    }
                    oxide_parser::Declaration::FunctionDeclaration(fd) => {
                        let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                        let mut inner = shadow.clone();
                        inner.extend(collect_fn_param_names(&fd.params));
                        inner.extend(collect_own_binding_names(&[], body));
                        collect_fn_default_names(&fd.params, ref_set, shadow, out);
                        collect_capture_names_shadowed(body, ref_set, &inner, out);
                    }
                    oxide_parser::Declaration::ClassDeclaration(cd) => {
                        let mut class_shadow = shadow.clone();
                        if let Some(id) = &cd.id {
                            class_shadow.insert(id.name.as_str().to_string());
                        }
                        collect_class_capture_names(&cd.body, ref_set, &class_shadow, out);
                    }
                    _ => {}
                }
            }
        }
        Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
            oxide_parser::ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let mut inner = shadow.clone();
                inner.extend(collect_fn_param_names(&fd.params));
                inner.extend(collect_own_binding_names(&[], body));
                collect_fn_default_names(&fd.params, ref_set, shadow, out);
                collect_capture_names_shadowed(body, ref_set, &inner, out);
            }
            oxide_parser::ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                let mut class_shadow = shadow.clone();
                if let Some(id) = &cd.id {
                    class_shadow.insert(id.name.as_str().to_string());
                }
                collect_class_capture_names(&cd.body, ref_set, &class_shadow, out);
            }
            other => {
                if let Some(e) = other.as_expression() {
                    collect_capture_names_expr(e, ref_set, shadow, out);
                }
            }
        },
        Statement::ThrowStatement(ts) => collect_capture_names_expr(&ts.argument, ref_set, shadow, out),
        Statement::SwitchStatement(sw) => {
            collect_capture_names_expr(&sw.discriminant, ref_set, shadow, out);
            for case in &sw.cases {
                for s in &case.consequent {
                    collect_capture_names_stmt(s, ref_set, shadow, out);
                }
            }
        }
        Statement::LabeledStatement(ls) => collect_capture_names_stmt(&ls.body, ref_set, shadow, out),
        Statement::WithStatement(ws) => {
            collect_capture_names_expr(&ws.object, ref_set, shadow, out);
            collect_capture_names_stmt(&ws.body, ref_set, shadow, out);
        }
        _ => {}
    }
}

/// 收集类体对 `ref_set` 的引用：方法/构造器体、字段键与值、静态块。
/// 方法形参与方法体局部声明遮蔽外层绑定（捕获判定用）。
pub(crate) fn collect_class_capture_names(
    class_body: &ClassBody, ref_set: &HashSet<String>, class_shadow: &HashSet<String>, out: &mut HashSet<String>,
) {
    for element in &class_body.body {
        match element {
            ClassElement::MethodDefinition(method) => {
                let mut inner = class_shadow.clone();
                inner.extend(collect_fn_param_names(&method.value.params));
                let body: &[Statement] = method.value.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                inner.extend(collect_own_binding_names(&[], body));
                collect_fn_default_names(&method.value.params, ref_set, class_shadow, out);
                collect_capture_names_shadowed(body, ref_set, &inner, out);
            }
            ClassElement::PropertyDefinition(prop) => {
                if let Some(expr) = prop.key.as_expression() {
                    collect_capture_names_expr(expr, ref_set, class_shadow, out);
                }
                if let Some(value) = &prop.value {
                    collect_capture_names_expr(value, ref_set, class_shadow, out);
                }
            }
            ClassElement::StaticBlock(block) => {
                collect_capture_names_shadowed(&block.body, ref_set, class_shadow, out);
            }
            _ => {}
        }
    }
}

/// 遍历 catch 参数解构模式：收集模式内表达式引用（默认值等），声明名写入
/// `catch_shadow` 遮蔽 catch 作用域。
pub(crate) fn collect_capture_names_binding_pattern(
    pattern: &oxide_parser::BindingPattern, ref_set: &HashSet<String>, shadow: &HashSet<String>,
    out: &mut HashSet<String>, catch_shadow: &mut HashSet<String>,
) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(bi) => {
            catch_shadow.insert(bi.name.as_str().to_string());
        }
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_capture_names_binding_pattern(p, ref_set, shadow, out, catch_shadow);
            }
            if let Some(rest) = &ap.rest {
                collect_capture_names_binding_pattern(&rest.argument, ref_set, shadow, out, catch_shadow);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                // 计算键是运行时求值表达式，其引用须纳入捕获。
                if prop.computed {
                    collect_capture_names_expr(prop.key.to_expression(), ref_set, shadow, out);
                }
                collect_capture_names_binding_pattern(&prop.value, ref_set, shadow, out, catch_shadow);
            }
            if let Some(rest) = &op.rest {
                collect_capture_names_binding_pattern(&rest.argument, ref_set, shadow, out, catch_shadow);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            collect_capture_names_expr(&ap.right, ref_set, shadow, out);
            collect_capture_names_binding_pattern(&ap.left, ref_set, shadow, out, catch_shadow);
        }
    }
}

/// 遍历绑定 pattern 的运行时求值表达式引用：计算键（`{[k]: a}` 的键）与内嵌
/// 默认值（`{a = x}` 的 x）都是模式内求值的表达式，嵌套函数引用外层绑定时须
/// 纳入捕获（与赋值侧/对象字面量/类字段键遍历同口径）。
pub(crate) fn collect_capture_names_binding_keys(
    pattern: &oxide_parser::BindingPattern, ref_set: &HashSet<String>, shadow: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(_) => {}
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_capture_names_binding_keys(p, ref_set, shadow, out);
            }
            if let Some(rest) = &ap.rest {
                collect_capture_names_binding_keys(&rest.argument, ref_set, shadow, out);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                if prop.computed {
                    collect_capture_names_expr(prop.key.to_expression(), ref_set, shadow, out);
                }
                collect_capture_names_binding_keys(&prop.value, ref_set, shadow, out);
            }
            if let Some(rest) = &op.rest {
                collect_capture_names_binding_keys(&rest.argument, ref_set, shadow, out);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            // 内嵌默认值表达式引用外层绑定须捕获（与 binding_pattern/赋值侧
            // maybe_default 的 init 扫描同口径）；left 继续递归模式键。
            collect_capture_names_expr(&ap.right, ref_set, shadow, out);
            collect_capture_names_binding_keys(&ap.left, ref_set, shadow, out);
        }
    }
}

/// 扫描赋值目标中的引用（标识符写名，成员目标扫对象/键，解构目标递归元素）。
pub(crate) fn collect_capture_names_assign_target(
    target: &oxide_parser::AssignmentTarget, ref_set: &HashSet<String>, shadow: &HashSet<String>,
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
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
        }
        oxide_parser::AssignmentTarget::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
            collect_capture_names_expr(&m.expression, ref_set, shadow, out);
        }
        oxide_parser::AssignmentTarget::ArrayAssignmentTarget(a) => {
            for e in a.elements.iter().flatten() {
                collect_capture_names_maybe_default_target(e, ref_set, shadow, out);
            }
            if let Some(rest) = &a.rest {
                collect_capture_names_assign_target(&rest.target, ref_set, shadow, out);
            }
        }
        oxide_parser::AssignmentTarget::ObjectAssignmentTarget(o) => {
            for prop in &o.properties {
                if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) = prop {
                    if ref_set.contains(id.binding.name.as_str()) && !shadow.contains(id.binding.name.as_str()) {
                        out.insert(id.binding.name.to_string());
                    }
                    if let Some(init) = &id.init {
                        collect_capture_names_expr(init, ref_set, shadow, out);
                    }
                } else if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) = prop {
                    if let Some(name_expr) = p.name.as_expression() {
                        collect_capture_names_expr(name_expr, ref_set, shadow, out);
                    }
                    collect_capture_names_maybe_default_target(&p.binding, ref_set, shadow, out);
                }
            }
            if let Some(rest) = &o.rest {
                collect_capture_names_assign_target(&rest.target, ref_set, shadow, out);
            }
        }
        _ => {}
    }
}

/// 解构赋值元素可能是 `AssignmentTarget` 或带默认值的包装（后者多一层 `init`）。
pub(crate) fn collect_capture_names_maybe_default_target(
    target: &oxide_parser::AssignmentTargetMaybeDefault, ref_set: &HashSet<String>, shadow: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    use oxide_parser::AssignmentTargetMaybeDefault as MaybeDefault;
    match target {
        MaybeDefault::AssignmentTargetWithDefault(d) => {
            collect_capture_names_expr(&d.init, ref_set, shadow, out);
            collect_capture_names_assign_target(&d.binding, ref_set, shadow, out);
        }
        MaybeDefault::AssignmentTargetIdentifier(ati) => {
            let name = ati.name.as_str();
            if ref_set.contains(name) && !shadow.contains(name) {
                out.insert(ati.name.to_string());
            }
        }
        MaybeDefault::StaticMemberExpression(m) => {
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
        }
        MaybeDefault::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
            collect_capture_names_expr(&m.expression, ref_set, shadow, out);
        }
        MaybeDefault::ArrayAssignmentTarget(a) => {
            for e in a.elements.iter().flatten() {
                collect_capture_names_maybe_default_target(e, ref_set, shadow, out);
            }
        }
        MaybeDefault::ObjectAssignmentTarget(o) => {
            for prop in &o.properties {
                if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) = prop {
                    if ref_set.contains(id.binding.name.as_str()) && !shadow.contains(id.binding.name.as_str()) {
                        out.insert(id.binding.name.to_string());
                    }
                    if let Some(init) = &id.init {
                        collect_capture_names_expr(init, ref_set, shadow, out);
                    }
                } else if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) = prop {
                    if let Some(name_expr) = p.name.as_expression() {
                        collect_capture_names_expr(name_expr, ref_set, shadow, out);
                    }
                    collect_capture_names_maybe_default_target(&p.binding, ref_set, shadow, out);
                }
            }
        }
        _ => {}
    }
}

/// 扫描一元更新目标（`++x` / `obj.x++`）中的引用。
pub(crate) fn collect_capture_names_simple_target(
    target: &oxide_parser::SimpleAssignmentTarget, ref_set: &HashSet<String>, shadow: &HashSet<String>,
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
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
        }
        oxide_parser::SimpleAssignmentTarget::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
            collect_capture_names_expr(&m.expression, ref_set, shadow, out);
        }
        _ => {}
    }
}

/// 收集函数参数默认值表达式里的引用（子层 upvalue 判定；参数名遮蔽）。
pub(crate) fn collect_fn_default_names(
    params: &oxide_parser::FormalParameters, ref_set: &HashSet<String>, shadow: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    for p in &params.items {
        if let Some(init) = &p.initializer {
            collect_capture_names_expr(init, ref_set, shadow, out);
        }
        let mut param_shadow = shadow.clone();
        collect_capture_names_binding_pattern(&p.pattern, ref_set, shadow, out, &mut param_shadow);
    }
}

/// 按表达式类型递归扫描引用；进入嵌套函数体时以形参 + 体内声明扩充遮蔽集。与
/// `captured.rs` 的 `collect_captured_expr` 相比，本函数带显式 `ref_set`/`shadow`
/// 收集引用名，不做捕获最终判定。
pub(crate) fn collect_capture_names_expr(
    expr: &Expression, ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
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
            collect_capture_names_assign_target(&ae.left, ref_set, shadow, out);
            collect_capture_names_expr(&ae.right, ref_set, shadow, out);
        }
        Expression::UpdateExpression(ue) => {
            collect_capture_names_simple_target(&ue.argument, ref_set, shadow, out);
        }
        Expression::FunctionExpression(fe) => {
            let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
            let mut inner = shadow.clone();
            inner.extend(collect_fn_param_names(&fe.params));
            inner.extend(collect_own_binding_names(&[], body));
            collect_fn_default_names(&fe.params, ref_set, shadow, out);
            collect_capture_names_shadowed(body, ref_set, &inner, out);
        }
        Expression::ArrowFunctionExpression(ae) => {
            let mut inner = shadow.clone();
            inner.extend(collect_fn_param_names(&ae.params));
            inner.extend(collect_own_binding_names(&[], &ae.body.statements));
            collect_fn_default_names(&ae.params, ref_set, shadow, out);
            collect_capture_names_shadowed(&ae.body.statements, ref_set, &inner, out);
        }
        Expression::ClassExpression(class) => {
            // 类表达式：方法/构造器体与字段表达式是嵌套作用域，引用须捕获。
            let mut class_shadow = shadow.clone();
            if let Some(id) = &class.id {
                class_shadow.insert(id.name.as_str().to_string());
            }
            collect_class_capture_names(&class.body, ref_set, &class_shadow, out);
        }
        Expression::BinaryExpression(be) => {
            collect_capture_names_expr(&be.left, ref_set, shadow, out);
            collect_capture_names_expr(&be.right, ref_set, shadow, out);
        }
        Expression::UnaryExpression(ue) => collect_capture_names_expr(&ue.argument, ref_set, shadow, out),
        Expression::CallExpression(ce) => {
            collect_capture_names_expr(&ce.callee, ref_set, shadow, out);
            for a in &ce.arguments {
                collect_capture_names_arg(a, ref_set, shadow, out);
            }
        }
        Expression::NewExpression(ne) => {
            collect_capture_names_expr(&ne.callee, ref_set, shadow, out);
            for a in &ne.arguments {
                collect_capture_names_arg(a, ref_set, shadow, out);
            }
        }
        Expression::SequenceExpression(se) => {
            for e in &se.expressions {
                collect_capture_names_expr(e, ref_set, shadow, out);
            }
        }
        Expression::ConditionalExpression(ce) => {
            collect_capture_names_expr(&ce.test, ref_set, shadow, out);
            collect_capture_names_expr(&ce.consequent, ref_set, shadow, out);
            collect_capture_names_expr(&ce.alternate, ref_set, shadow, out);
        }
        Expression::ArrayExpression(ae) => {
            for e in &ae.elements {
                if let Some(e) = e.as_expression() {
                    collect_capture_names_expr(e, ref_set, shadow, out);
                }
            }
        }
        Expression::LogicalExpression(le) => {
            collect_capture_names_expr(&le.left, ref_set, shadow, out);
            collect_capture_names_expr(&le.right, ref_set, shadow, out);
        }
        Expression::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
            collect_capture_names_expr(&m.expression, ref_set, shadow, out);
        }
        Expression::StaticMemberExpression(m) => collect_capture_names_expr(&m.object, ref_set, shadow, out),
        Expression::PrivateFieldExpression(m) => collect_capture_names_expr(&m.object, ref_set, shadow, out),
        Expression::ParenthesizedExpression(p) => collect_capture_names_expr(&p.expression, ref_set, shadow, out),
        Expression::TemplateLiteral(tl) => {
            for e in &tl.expressions {
                collect_capture_names_expr(e, ref_set, shadow, out);
            }
        }
        Expression::TaggedTemplateExpression(tt) => {
            collect_capture_names_expr(&tt.tag, ref_set, shadow, out);
            for e in &tt.quasi.expressions {
                collect_capture_names_expr(e, ref_set, shadow, out);
            }
        }
        Expression::ObjectExpression(o) => {
            for prop in &o.properties {
                match prop {
                    oxide_parser::ObjectPropertyKind::ObjectProperty(p) => {
                        if p.computed {
                            collect_capture_names_expr(p.key.to_expression(), ref_set, shadow, out);
                        }
                        collect_capture_names_expr(&p.value, ref_set, shadow, out);
                    }
                    oxide_parser::ObjectPropertyKind::SpreadProperty(spread) => {
                        collect_capture_names_expr(&spread.argument, ref_set, shadow, out);
                    }
                }
            }
        }
        // 生成器让出表达式：被让出的值里引用的父变量须纳入捕获，否则生成器体经
        // LOAD_VAR 读调用方寄存器残留（挂起恢复后寄存器已被覆盖）。
        Expression::YieldExpression(ye) => {
            if let Some(a) = &ye.argument {
                collect_capture_names_expr(a, ref_set, shadow, out);
            }
        }
        // await 表达式：被等待的值里引用的父变量须纳入捕获（与 yield 同因——
        // 异步体挂起恢复后寄存器已被覆盖，只能经 cell 读取）。
        Expression::AwaitExpression(ae) => {
            collect_capture_names_expr(&ae.argument, ref_set, shadow, out);
        }
        Expression::ChainExpression(c) => collect_capture_names_chain(&c.expression, ref_set, shadow, out),
        _ => {}
    }
}

/// 遍历可选链元素，扫描链上成员访问、调用与实参中的引用。
pub(crate) fn collect_capture_names_chain(
    element: &oxide_parser::ChainElement, ref_set: &HashSet<String>, shadow: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    match element {
        oxide_parser::ChainElement::StaticMemberExpression(m) => {
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
        }
        oxide_parser::ChainElement::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
            collect_capture_names_expr(&m.expression, ref_set, shadow, out);
        }
        oxide_parser::ChainElement::PrivateFieldExpression(m) => {
            collect_capture_names_expr(&m.object, ref_set, shadow, out);
        }
        oxide_parser::ChainElement::CallExpression(call) => {
            collect_capture_names_expr(&call.callee, ref_set, shadow, out);
            for a in &call.arguments {
                collect_capture_names_arg(a, ref_set, shadow, out);
            }
        }
        _ => {}
    }
}

/// 遍历调用实参：静态实参与 spread 内部表达式都纳入捕获扫描。
pub(crate) fn collect_capture_names_arg(
    arg: &oxide_parser::Argument, ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
) {
    if let Some(e) = arg.as_expression() {
        collect_capture_names_expr(e, ref_set, shadow, out);
    } else if let oxide_parser::Argument::SpreadElement(sp) = arg {
        collect_capture_names_expr(&sp.argument, ref_set, shadow, out);
    }
}
