//! 闭包捕获分析早退守卫：扫描语句树与附加表达式，判定是否存在嵌套函数/类节点，
//! 无嵌套时使捕获集收集（collect_captured_bindings，仅从嵌套函数/类节点产出名字）
//! 整体跳过。
//!
//! 关键约定：守卫臂集须与 `captured.rs` / `scanner.rs` 两收集器同步——新增 parser
//! 臂引入函数/类节点入口时三处同改；守卫漏任一入口臂即假阴，跳过捕获集收集导致
//! 嵌套函数 upvalue 捕获丢失。

use oxide_parser::{
    Argument, AssignmentTarget, AssignmentTargetMaybeDefault, BindingPattern, ChainElement, Declaration,
    ExportDefaultDeclarationKind, Expression, ForStatementLeft, SimpleAssignmentTarget, Statement,
};

/// 扫描语句树与附加表达式，判定是否存在嵌套函数/类节点（早退）。
///
/// 命中首个函数/类节点（函数声明/表达式、箭头函数、类声明/表达式）立即返回真，
/// 不分配任何集合、不克隆遮蔽集，单趟只读遍历显著轻于捕获集收集（每层嵌套函数
/// 克隆遮蔽集并再遍历）。
///
/// # 边界与前提
/// - `extra_exprs` 为参数默认值与类字段值表达式（位于函数体之外），与体同臂扫描。
/// - 臂集与 `captured.rs` / `scanner.rs` 两收集器逐臂对齐（含 pattern 计算键、
///   for 赋值头、catch 参数、export 包裹臂）；新增 parser 臂引入函数/类节点入口
///   时三处同改。
///
/// # 注意事项
/// - 守卫是跳过捕获集收集的必要条件：假阴（漏扫）丢 upvalue 捕获、读错 cell；
///   假阳（多扫）仅使捕获集收集照跑，安全。
pub(crate) fn has_nested_function(stmts: &[Statement], extra_exprs: &[&Expression]) -> bool {
    for stmt in stmts {
        if scan_stmt(stmt) {
            return true;
        }
    }
    for expr in extra_exprs {
        if scan_expr(expr) {
            return true;
        }
    }
    false
}

/// 扫描语句树：函数/类声明节点立即返回真，其余臂递归其表达式子节点，臂集与
/// 两收集器一致。
fn scan_stmt(stmt: &Statement) -> bool {
    match stmt {
        Statement::FunctionDeclaration(_) | Statement::ClassDeclaration(_) => true,
        // export 包裹的声明：变量/函数/类声明体须下探。
        Statement::ExportNamedDeclaration(exp) => match &exp.declaration {
            Some(Declaration::VariableDeclaration(vd)) => scan_var_decl(vd),
            Some(Declaration::FunctionDeclaration(_)) | Some(Declaration::ClassDeclaration(_)) => true,
            _ => false,
        },
        Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(_)
            | ExportDefaultDeclarationKind::ClassDeclaration(_) => true,
            other => other.as_expression().is_some_and(scan_expr),
        },
        Statement::ExpressionStatement(es) => scan_expr(&es.expression),
        Statement::ReturnStatement(rs) => rs.argument.as_ref().is_some_and(scan_expr),
        Statement::VariableDeclaration(vd) => scan_var_decl(vd),
        Statement::IfStatement(is) => {
            scan_expr(&is.test) || scan_stmt(&is.consequent) || is.alternate.as_ref().is_some_and(scan_stmt)
        }
        Statement::ForStatement(fs) => {
            let init = fs.init.as_ref().is_some_and(|init| {
                if let Some(e) = init.as_expression() {
                    scan_expr(e)
                } else if let oxide_parser::ForStatementInit::VariableDeclaration(vd) = init {
                    scan_var_decl(vd)
                } else {
                    false
                }
            });
            init || fs.test.as_ref().is_some_and(scan_expr)
                || fs.update.as_ref().is_some_and(scan_expr)
                || scan_stmt(&fs.body)
        }
        Statement::WhileStatement(w) => scan_expr(&w.test) || scan_stmt(&w.body),
        Statement::DoWhileStatement(d) => scan_stmt(&d.body) || scan_expr(&d.test),
        Statement::ForInStatement(fi) => scan_expr(&fi.right) || scan_for_left(&fi.left) || scan_stmt(&fi.body),
        Statement::ForOfStatement(fo) => scan_expr(&fo.right) || scan_for_left(&fo.left) || scan_stmt(&fo.body),
        Statement::BlockStatement(b) => b.body.iter().any(scan_stmt),
        Statement::TryStatement(ts) => {
            ts.block.body.iter().any(scan_stmt)
                || ts.handler.as_ref().is_some_and(|h| {
                    // catch 参数 pattern 的内嵌默认值可含嵌套函数。
                    h.param.as_ref().is_some_and(|p| scan_binding_keys(&p.pattern)) || h.body.body.iter().any(scan_stmt)
                })
                || ts.finalizer.as_ref().is_some_and(|f| f.body.iter().any(scan_stmt))
        }
        Statement::ThrowStatement(ts) => scan_expr(&ts.argument),
        Statement::SwitchStatement(sw) => {
            scan_expr(&sw.discriminant) || sw.cases.iter().any(|c| c.consequent.iter().any(scan_stmt))
        }
        Statement::LabeledStatement(ls) => scan_stmt(&ls.body),
        Statement::WithStatement(ws) => scan_expr(&ws.object) || scan_stmt(&ws.body),
        _ => false,
    }
}

/// 变量声明：绑定 pattern 的计算键与内嵌默认值是运行时求值表达式，与初始化式
/// 同臂扫描。
fn scan_var_decl(vd: &oxide_parser::VariableDeclaration) -> bool {
    vd.declarations
        .iter()
        .any(|d| scan_binding_keys(&d.id) || d.init.as_ref().is_some_and(scan_expr))
}

/// 遍历绑定 pattern 内运行时求值表达式：计算键（`{[k]: a}` 的键）与内嵌默认值
/// （`[x = expr]`），与 catch 参数 pattern 同形。
fn scan_binding_keys(pattern: &BindingPattern) -> bool {
    match pattern {
        BindingPattern::BindingIdentifier(_) => false,
        BindingPattern::ArrayPattern(ap) => {
            ap.elements.iter().flatten().any(scan_binding_keys)
                || ap.rest.as_ref().is_some_and(|rest| scan_binding_keys(&rest.argument))
        }
        BindingPattern::ObjectPattern(op) => {
            op.properties
                .iter()
                .any(|prop| (prop.computed && scan_expr(prop.key.to_expression())) || scan_binding_keys(&prop.value))
                || op.rest.as_ref().is_some_and(|rest| scan_binding_keys(&rest.argument))
        }
        BindingPattern::AssignmentPattern(ap) => scan_expr(&ap.right) || scan_binding_keys(&ap.left),
    }
}

/// for-in/for-of 头：声明头扫 pattern 键，赋值头扫解构目标元素（含默认值）。
fn scan_for_left(left: &ForStatementLeft) -> bool {
    match left {
        ForStatementLeft::VariableDeclaration(vd) => scan_var_decl(vd),
        ForStatementLeft::ArrayAssignmentTarget(a) => scan_array_target(a),
        ForStatementLeft::ObjectAssignmentTarget(o) => scan_object_target(o),
        _ => false,
    }
}

/// 赋值目标：成员目标扫对象/键，解构目标扫元素（含默认值）。
fn scan_assign_target(target: &AssignmentTarget) -> bool {
    match target {
        AssignmentTarget::StaticMemberExpression(m) => scan_expr(&m.object),
        AssignmentTarget::ComputedMemberExpression(m) => scan_expr(&m.object) || scan_expr(&m.expression),
        AssignmentTarget::ArrayAssignmentTarget(a) => scan_array_target(a),
        AssignmentTarget::ObjectAssignmentTarget(o) => scan_object_target(o),
        _ => false,
    }
}

/// 数组赋值目标：元素（含默认值）与 rest 目标。
fn scan_array_target(a: &oxide_parser::ArrayAssignmentTarget) -> bool {
    a.elements.iter().flatten().any(scan_maybe_default_target)
        || a.rest.as_ref().is_some_and(|rest| scan_assign_target(&rest.target))
}

/// 对象赋值目标：属性标识符默认值、属性名表达式、绑定目标与 rest。
fn scan_object_target(o: &oxide_parser::ObjectAssignmentTarget) -> bool {
    o.properties.iter().any(|prop| match prop {
        oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
            id.init.as_ref().is_some_and(scan_expr)
        }
        oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
            p.name.as_expression().is_some_and(scan_expr) || scan_maybe_default_target(&p.binding)
        }
    }) || o.rest.as_ref().is_some_and(|rest| scan_assign_target(&rest.target))
}

/// 解构赋值元素：带默认值的元素多一层（默认值表达式加绑定目标）。
fn scan_maybe_default_target(target: &AssignmentTargetMaybeDefault) -> bool {
    match target {
        AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
            scan_expr(&d.init) || scan_assign_target(&d.binding)
        }
        AssignmentTargetMaybeDefault::StaticMemberExpression(m) => scan_expr(&m.object),
        AssignmentTargetMaybeDefault::ComputedMemberExpression(m) => scan_expr(&m.object) || scan_expr(&m.expression),
        AssignmentTargetMaybeDefault::ArrayAssignmentTarget(a) => scan_array_target(a),
        AssignmentTargetMaybeDefault::ObjectAssignmentTarget(o) => scan_object_target(o),
        _ => false,
    }
}

/// 一元更新目标：成员目标扫对象/键。
fn scan_simple_target(target: &SimpleAssignmentTarget) -> bool {
    match target {
        SimpleAssignmentTarget::StaticMemberExpression(m) => scan_expr(&m.object),
        SimpleAssignmentTarget::ComputedMemberExpression(m) => scan_expr(&m.object) || scan_expr(&m.expression),
        _ => false,
    }
}

/// 扫描表达式树：函数/类表达式节点立即返回真，其余臂递归，臂集与两收集器一致。
fn scan_expr(expr: &Expression) -> bool {
    match expr {
        Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_) | Expression::ClassExpression(_) => {
            true
        }
        Expression::CallExpression(ce) => scan_expr(&ce.callee) || ce.arguments.iter().any(scan_arg),
        Expression::NewExpression(ne) => scan_expr(&ne.callee) || ne.arguments.iter().any(scan_arg),
        Expression::BinaryExpression(be) => scan_expr(&be.left) || scan_expr(&be.right),
        Expression::UnaryExpression(ue) => scan_expr(&ue.argument),
        Expression::LogicalExpression(le) => scan_expr(&le.left) || scan_expr(&le.right),
        Expression::ConditionalExpression(ce) => {
            scan_expr(&ce.test) || scan_expr(&ce.consequent) || scan_expr(&ce.alternate)
        }
        Expression::SequenceExpression(se) => se.expressions.iter().any(scan_expr),
        // 赋值目标侧也可含嵌套函数（解构目标默认值），左右同扫。
        Expression::AssignmentExpression(ae) => scan_assign_target(&ae.left) || scan_expr(&ae.right),
        Expression::UpdateExpression(ue) => scan_simple_target(&ue.argument),
        Expression::ArrayExpression(ae) => ae.elements.iter().any(|e| e.as_expression().is_some_and(scan_expr)),
        Expression::ObjectExpression(o) => o.properties.iter().any(|prop| match prop {
            oxide_parser::ObjectPropertyKind::ObjectProperty(p) => {
                (p.computed && scan_expr(p.key.to_expression())) || scan_expr(&p.value)
            }
            oxide_parser::ObjectPropertyKind::SpreadProperty(spread) => scan_expr(&spread.argument),
        }),
        Expression::ComputedMemberExpression(m) => scan_expr(&m.object) || scan_expr(&m.expression),
        Expression::StaticMemberExpression(m) => scan_expr(&m.object),
        Expression::PrivateFieldExpression(m) => scan_expr(&m.object),
        Expression::TemplateLiteral(tl) => tl.expressions.iter().any(scan_expr),
        Expression::TaggedTemplateExpression(tt) => scan_expr(&tt.tag) || tt.quasi.expressions.iter().any(scan_expr),
        Expression::ParenthesizedExpression(p) => scan_expr(&p.expression),
        Expression::YieldExpression(ye) => ye.argument.as_ref().is_some_and(scan_expr),
        Expression::AwaitExpression(ae) => scan_expr(&ae.argument),
        Expression::ChainExpression(c) => scan_chain(&c.expression),
        _ => false,
    }
}

/// 遍历可选链元素：成员访问、调用与实参。
fn scan_chain(element: &ChainElement) -> bool {
    match element {
        ChainElement::StaticMemberExpression(m) => scan_expr(&m.object),
        ChainElement::ComputedMemberExpression(m) => scan_expr(&m.object) || scan_expr(&m.expression),
        ChainElement::PrivateFieldExpression(m) => scan_expr(&m.object),
        ChainElement::CallExpression(c) => scan_expr(&c.callee) || c.arguments.iter().any(scan_arg),
        _ => false,
    }
}

/// 调用实参：静态实参与 spread 内部表达式。
fn scan_arg(arg: &Argument) -> bool {
    match arg {
        Argument::SpreadElement(sp) => scan_expr(&sp.argument),
        _ => arg.as_expression().is_some_and(scan_expr),
    }
}
