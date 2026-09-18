//! 闭包捕获分析，父层捕获分析：本函数哪些绑定被任意深度嵌套函数捕获
//! → BTreeMap<String, u8>（cell_idx 按名字排序分配，同一程序重编译得到相同索引，
//! 父 MAKE_CELL 与子函数 upvalue 引用同一 cell）。

use std::collections::{BTreeMap, HashSet};

use oxide_parser::{Declaration, ExportDefaultDeclarationKind, Expression, Statement};

use super::collect_fn_param_names;
use super::names::collect_own_binding_names;
use super::scanner::{collect_capture_names_expr, collect_capture_names_shadowed, collect_class_capture_names};

/// 分析本函数：哪些绑定被任意深度嵌套函数捕获 → captured_bindings。
///
/// 本族以本函数 own 绑定集为过滤条件做捕获集的最终判定；`scanner.rs` 的
/// `collect_capture_names_*` 族带显式 `ref_set`/`shadow`，按过滤条件收集引用名。
pub(crate) fn collect_captured_bindings(
    stmts: &[Statement], extra_exprs: &[&oxide_parser::Expression], own: &HashSet<String>,
) -> BTreeMap<String, u8> {
    let mut names = HashSet::new();
    for stmt in stmts {
        collect_captured_stmt(stmt, own, &mut names);
    }
    // 参数默认值表达式里的引用同样纳入捕获分析：默认值位于函数体之外，其中的
    // 嵌套函数引用全局或外层变量时，若未登记进捕获集，发射期会走 LOAD_VAR 读到
    // 寄存器残留值。
    for expr in extra_exprs {
        collect_captured_expr(expr, own, &mut names);
    }
    // 名字排序分配 cell_idx（稳定跨 run，父 MAKE_CELL 与子 upvalue 统一引用）
    let mut sorted: Vec<String> = names.into_iter().collect();
    sorted.sort();
    sorted.into_iter().enumerate().map(|(i, n)| (n, i as u8)).collect()
}

/// 收集函数参数默认值表达式里的嵌套函数引用（父层 captured/子层 upvalue 判定）。
pub(crate) fn collect_fn_default_captured(
    params: &oxide_parser::FormalParameters, own: &HashSet<String>, out: &mut HashSet<String>,
) {
    let empty_shadow = HashSet::new();
    for p in &params.items {
        if let Some(init) = &p.initializer {
            collect_capture_names_expr(init, own, &empty_shadow, out);
        }
        // 形参模式的运行时求值表达式（计算键/内嵌默认值）在子作用域求值，
        // 其标识符引用须纳入父层 MAKE_CELL 判定。
        collect_captured_pattern_runtime_exprs(&p.pattern, own, out);
    }
}

/// 遍历绑定 pattern 内全部运行时求值表达式（计算键与 AssignmentPattern 默认值）：
/// 这些表达式在子作用域（嵌套函数形参绑定时）求值，引用的父层绑定须建 cell 供
/// 子函数 upvalue 读取——与 `collect_capture_names_expr` 同口径（标识符比对）。
pub(crate) fn collect_captured_pattern_runtime_exprs(
    pattern: &oxide_parser::BindingPattern, own: &HashSet<String>, out: &mut HashSet<String>,
) {
    let empty_shadow = HashSet::new();
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(_) => {}
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_captured_pattern_runtime_exprs(p, own, out);
            }
            if let Some(rest) = &ap.rest {
                collect_captured_pattern_runtime_exprs(&rest.argument, own, out);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                if prop.computed {
                    collect_capture_names_expr(prop.key.to_expression(), own, &empty_shadow, out);
                }
                collect_captured_pattern_runtime_exprs(&prop.value, own, out);
            }
            if let Some(rest) = &op.rest {
                collect_captured_pattern_runtime_exprs(&rest.argument, own, out);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            collect_capture_names_expr(&ap.right, own, &empty_shadow, out);
            collect_captured_pattern_runtime_exprs(&ap.left, own, out);
        }
    }
}

/// 只从嵌套函数节点进入扫描（本函数直接引用不算捕获）。
pub(crate) fn collect_captured_stmt(stmt: &Statement, own: &HashSet<String>, out: &mut HashSet<String>) {
    match stmt {
        Statement::FunctionDeclaration(fd) => {
            let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
            collect_fn_default_captured(&fd.params, own, out);
            // 函数参数与体内部声明遮蔽父级绑定：体内对这些名字的引用不算捕获父级。
            let mut fn_shadow = HashSet::new();
            fn_shadow.extend(collect_fn_param_names(&fd.params));
            fn_shadow.extend(collect_own_binding_names(&[], body));
            collect_capture_names_shadowed(body, own, &fn_shadow, out);
        }
        Statement::ClassDeclaration(cd) => {
            // 类构造器/方法体与字段表达式引用的父级绑定须建 cell，供子模块 upvalue 捕获。
            let mut class_shadow = HashSet::new();
            if let Some(id) = &cd.id {
                class_shadow.insert(id.name.as_str().to_string());
            }
            collect_class_capture_names(&cd.body, own, &class_shadow, out);
        }
        // export 包裹的变量/函数/类声明：捕获分析须下探声明体，否则被导出函数体内
        // 引用模块绑定（含自引用）不会被识别为捕获，闭包内读写退化为隐式全局。
        Statement::ExportNamedDeclaration(exp) => {
            if let Some(decl) = &exp.declaration {
                match decl {
                    Declaration::VariableDeclaration(vd) => {
                        for d in &vd.declarations {
                            collect_captured_binding_keys(&d.id, own, out);
                            if let Some(init) = &d.init {
                                collect_captured_expr(init, own, out);
                            }
                        }
                    }
                    Declaration::FunctionDeclaration(fd) => {
                        let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                        collect_fn_default_captured(&fd.params, own, out);
                        let mut fn_shadow = HashSet::new();
                        fn_shadow.extend(collect_fn_param_names(&fd.params));
                        fn_shadow.extend(collect_own_binding_names(&[], body));
                        collect_capture_names_shadowed(body, own, &fn_shadow, out);
                    }
                    Declaration::ClassDeclaration(cd) => {
                        let mut class_shadow = HashSet::new();
                        if let Some(id) = &cd.id {
                            class_shadow.insert(id.name.as_str().to_string());
                        }
                        collect_class_capture_names(&cd.body, own, &class_shadow, out);
                    }
                    _ => {}
                }
            }
        }
        Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                collect_fn_default_captured(&fd.params, own, out);
                let mut fn_shadow = HashSet::new();
                fn_shadow.extend(collect_fn_param_names(&fd.params));
                fn_shadow.extend(collect_own_binding_names(&[], body));
                collect_capture_names_shadowed(body, own, &fn_shadow, out);
            }
            ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                let mut class_shadow = HashSet::new();
                if let Some(id) = &cd.id {
                    class_shadow.insert(id.name.as_str().to_string());
                }
                collect_class_capture_names(&cd.body, own, &class_shadow, out);
            }
            other => {
                if let Some(e) = other.as_expression() {
                    collect_captured_expr(e, own, out);
                }
            }
        },
        Statement::ExpressionStatement(es) => collect_captured_expr(&es.expression, own, out),
        Statement::ReturnStatement(rs) => {
            if let Some(a) = &rs.argument {
                collect_captured_expr(a, own, out);
            }
        }
        Statement::VariableDeclaration(vd) => {
            for d in &vd.declarations {
                // 绑定 pattern 的计算键是运行时求值表达式，其引用须纳入捕获判定。
                collect_captured_binding_keys(&d.id, own, out);
                if let Some(init) = &d.init {
                    collect_captured_expr(init, own, out);
                }
            }
        }
        Statement::IfStatement(is) => {
            collect_captured_expr(&is.test, own, out);
            collect_captured_stmt(&is.consequent, own, out);
            if let Some(alt) = &is.alternate {
                collect_captured_stmt(alt, own, out);
            }
        }
        Statement::ForStatement(fs) => {
            if let Some(init) = &fs.init {
                if let Some(e) = init.as_expression() {
                    collect_captured_expr(e, own, out);
                }
                if let oxide_parser::ForStatementInit::VariableDeclaration(vd) = init {
                    for d in &vd.declarations {
                        collect_captured_binding_keys(&d.id, own, out);
                        if let Some(i) = &d.init {
                            collect_captured_expr(i, own, out);
                        }
                    }
                }
            }
            if let Some(t) = &fs.test {
                collect_captured_expr(t, own, out);
            }
            if let Some(u) = &fs.update {
                collect_captured_expr(u, own, out);
            }
            collect_captured_stmt(&fs.body, own, out);
        }
        Statement::WhileStatement(w) => {
            collect_captured_expr(&w.test, own, out);
            collect_captured_stmt(&w.body, own, out);
        }
        Statement::DoWhileStatement(d) => {
            collect_captured_stmt(&d.body, own, out);
            collect_captured_expr(&d.test, own, out);
        }
        Statement::ForInStatement(fi) => {
            collect_captured_expr(&fi.right, own, out);
            if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fi.left {
                for d in &vd.declarations {
                    collect_captured_binding_keys(&d.id, own, out);
                }
            }
            collect_captured_stmt(&fi.body, own, out);
        }
        Statement::ForOfStatement(fo) => {
            collect_captured_expr(&fo.right, own, out);
            if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fo.left {
                for d in &vd.declarations {
                    collect_captured_binding_keys(&d.id, own, out);
                }
            }
            collect_captured_stmt(&fo.body, own, out);
        }
        Statement::BlockStatement(b) => {
            for s in &b.body {
                collect_captured_stmt(s, own, out);
            }
        }
        Statement::TryStatement(ts) => {
            for s in &ts.block.body {
                collect_captured_stmt(s, own, out);
            }
            if let Some(h) = &ts.handler {
                if let Some(param) = &h.param {
                    collect_captured_binding_pattern(&param.pattern, own, out);
                }
                for s in &h.body.body {
                    collect_captured_stmt(s, own, out);
                }
            }
            if let Some(f) = &ts.finalizer {
                for s in &f.body {
                    collect_captured_stmt(s, own, out);
                }
            }
        }
        Statement::ThrowStatement(ts) => collect_captured_expr(&ts.argument, own, out),
        Statement::SwitchStatement(sw) => {
            collect_captured_expr(&sw.discriminant, own, out);
            for case in &sw.cases {
                for s in &case.consequent {
                    collect_captured_stmt(s, own, out);
                }
            }
        }
        Statement::LabeledStatement(ls) => collect_captured_stmt(&ls.body, own, out),
        Statement::WithStatement(ws) => {
            collect_captured_expr(&ws.object, own, out);
            collect_captured_stmt(&ws.body, own, out);
        }
        _ => {}
    }
}

/// 遍历 catch 参数解构模式内表达式引用（默认值等），供父层 MAKE_CELL 判定。
pub(crate) fn collect_captured_binding_pattern(
    pattern: &oxide_parser::BindingPattern, own: &HashSet<String>, out: &mut HashSet<String>,
) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(_) => {}
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_captured_binding_pattern(p, own, out);
            }
            if let Some(rest) = &ap.rest {
                collect_captured_binding_pattern(&rest.argument, own, out);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                // 计算键表达式（本作用域 catch 参数求值，含 IIFE 等嵌套函数）引用
                // 本函数绑定走寄存器，只须扫描键内嵌套函数表达式。
                if prop.computed {
                    collect_captured_expr(prop.key.to_expression(), own, out);
                }
                collect_captured_binding_pattern(&prop.value, own, out);
            }
            if let Some(rest) = &op.rest {
                collect_captured_binding_pattern(&rest.argument, own, out);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            collect_captured_expr(&ap.right, own, out);
            collect_captured_binding_pattern(&ap.left, own, out);
        }
    }
}

/// 遍历绑定 pattern 的运行时求值表达式引用：计算键（`{[k]: a}` 的键）与内嵌
/// 默认值（`{a = x}` 的 x），供父层捕获判定（声明语句/for 头的模式与 catch/
/// 默认值路径同口径）。
pub(crate) fn collect_captured_binding_keys(
    pattern: &oxide_parser::BindingPattern, own: &HashSet<String>, out: &mut HashSet<String>,
) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(_) => {}
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_captured_binding_keys(p, own, out);
            }
            if let Some(rest) = &ap.rest {
                collect_captured_binding_keys(&rest.argument, own, out);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                if prop.computed {
                    collect_captured_expr(prop.key.to_expression(), own, out);
                }
                collect_captured_binding_keys(&prop.value, own, out);
            }
            if let Some(rest) = &op.rest {
                collect_captured_binding_keys(&rest.argument, own, out);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            // 内嵌默认值表达式引用外层绑定须建 cell（与 captured_binding_pattern
            // 同口径）；left 继续递归模式键。
            collect_captured_expr(&ap.right, own, out);
            collect_captured_binding_keys(&ap.left, own, out);
        }
    }
}

/// 以本函数 own 绑定集为过滤条件，判定表达式内嵌套函数捕获了哪些父绑定。与
/// `scanner.rs` 的 `collect_capture_names_expr` 相比，本函数以 own 为过滤条件做
/// 捕获最终判定，不接收显式 `ref_set`/`shadow`。
pub(crate) fn collect_captured_expr(expr: &Expression, own: &HashSet<String>, out: &mut HashSet<String>) {
    match expr {
        Expression::FunctionExpression(fe) => {
            let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
            let mut inner = collect_fn_param_names(&fe.params);
            inner.extend(collect_own_binding_names(&[], body));
            collect_fn_default_captured(&fe.params, own, out);
            collect_capture_names_shadowed(body, own, &inner, out);
        }
        Expression::ArrowFunctionExpression(ae) => {
            let mut inner = collect_fn_param_names(&ae.params);
            inner.extend(collect_own_binding_names(&[], &ae.body.statements));
            collect_fn_default_captured(&ae.params, own, out);
            collect_capture_names_shadowed(&ae.body.statements, own, &inner, out);
        }
        Expression::ClassExpression(class) => {
            // 类表达式：构造器/方法体与字段表达式引用的父级绑定须建 cell。
            let mut class_shadow = HashSet::new();
            if let Some(id) = &class.id {
                class_shadow.insert(id.name.as_str().to_string());
            }
            collect_class_capture_names(&class.body, own, &class_shadow, out);
        }
        Expression::CallExpression(ce) => {
            collect_captured_expr(&ce.callee, own, out);
            for a in &ce.arguments {
                collect_captured_arg(a, own, out);
            }
        }
        Expression::BinaryExpression(be) => {
            collect_captured_expr(&be.left, own, out);
            collect_captured_expr(&be.right, own, out);
        }
        Expression::UnaryExpression(ue) => collect_captured_expr(&ue.argument, own, out),
        Expression::LogicalExpression(le) => {
            collect_captured_expr(&le.left, own, out);
            collect_captured_expr(&le.right, own, out);
        }
        Expression::ConditionalExpression(ce) => {
            collect_captured_expr(&ce.test, own, out);
            collect_captured_expr(&ce.consequent, own, out);
            collect_captured_expr(&ce.alternate, own, out);
        }
        Expression::SequenceExpression(se) => {
            for e in &se.expressions {
                collect_captured_expr(e, own, out);
            }
        }
        Expression::AssignmentExpression(ae) => {
            // 只扫右侧：左侧赋值目标是本函数绑定，不引用嵌套函数；
            // 若目标含嵌套函数表达式（如 `[f()] = ...`），由其自身递归处理。
            collect_captured_expr(&ae.right, own, out);
        }
        Expression::UpdateExpression(_ue) => {} // 一元更新目标不引用嵌套函数，无需捕获
        Expression::ArrayExpression(ae) => {
            for e in &ae.elements {
                if let Some(e) = e.as_expression() {
                    collect_captured_expr(e, own, out);
                }
            }
        }
        Expression::ObjectExpression(o) => {
            for prop in &o.properties {
                match prop {
                    oxide_parser::ObjectPropertyKind::ObjectProperty(p) => {
                        if p.computed {
                            collect_captured_expr(p.key.to_expression(), own, out);
                        }
                        collect_captured_expr(&p.value, own, out);
                    }
                    oxide_parser::ObjectPropertyKind::SpreadProperty(spread) => {
                        collect_captured_expr(&spread.argument, own, out);
                    }
                }
            }
        }
        Expression::NewExpression(ne) => {
            collect_captured_expr(&ne.callee, own, out);
            for a in &ne.arguments {
                collect_captured_arg(a, own, out);
            }
        }
        Expression::ComputedMemberExpression(m) => {
            collect_captured_expr(&m.object, own, out);
            collect_captured_expr(&m.expression, own, out);
        }
        Expression::StaticMemberExpression(m) => collect_captured_expr(&m.object, own, out),
        Expression::PrivateFieldExpression(m) => collect_captured_expr(&m.object, own, out),
        Expression::TemplateLiteral(tl) => {
            for e in &tl.expressions {
                collect_captured_expr(e, own, out);
            }
        }
        Expression::TaggedTemplateExpression(tt) => {
            collect_captured_expr(&tt.tag, own, out);
            for e in &tt.quasi.expressions {
                collect_captured_expr(e, own, out);
            }
        }
        Expression::ParenthesizedExpression(p) => collect_captured_expr(&p.expression, own, out),
        Expression::YieldExpression(ye) => {
            if let Some(a) = &ye.argument {
                collect_captured_expr(a, own, out);
            }
        }
        Expression::AwaitExpression(ae) => collect_captured_expr(&ae.argument, own, out),
        Expression::ChainExpression(c) => collect_captured_chain(&c.expression, own, out),
        _ => {}
    }
}

/// 遍历可选链元素，判定链上表达式对父绑定的捕获。
pub(crate) fn collect_captured_chain(
    element: &oxide_parser::ChainElement, own: &HashSet<String>, out: &mut HashSet<String>,
) {
    match element {
        oxide_parser::ChainElement::StaticMemberExpression(m) => {
            collect_captured_expr(&m.object, own, out);
        }
        oxide_parser::ChainElement::ComputedMemberExpression(m) => {
            collect_captured_expr(&m.object, own, out);
            collect_captured_expr(&m.expression, own, out);
        }
        oxide_parser::ChainElement::PrivateFieldExpression(m) => {
            collect_captured_expr(&m.object, own, out);
        }
        oxide_parser::ChainElement::CallExpression(c) => {
            collect_captured_expr(&c.callee, own, out);
            for a in &c.arguments {
                collect_captured_arg(a, own, out);
            }
        }
        _ => {}
    }
}

/// 遍历调用实参：静态实参与 spread 内部表达式都纳入捕获判定。
pub(crate) fn collect_captured_arg(arg: &oxide_parser::Argument, own: &HashSet<String>, out: &mut HashSet<String>) {
    if let Some(e) = arg.as_expression() {
        collect_captured_expr(e, own, out);
    } else if let oxide_parser::Argument::SpreadElement(sp) = arg {
        collect_captured_expr(&sp.argument, own, out);
    }
}
