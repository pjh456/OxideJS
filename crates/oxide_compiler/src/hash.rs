use oxide_parser::{
    ClassElement, Expression, ForStatementInit, ObjectPropertyKind, PropertyKey, SimpleAssignmentTarget, Statement,
};
use std::hash::Hash;

#[derive(Hash)]
enum HashDomain {
    Statement,
    Expression,
    ClassElement,
    PropertyKey,
    ObjectPropertyKind,
    SimpleAssignmentTarget,
}

macro_rules! hash_match {
    ($domain:expr, $value:expr, $h:expr, { $($arms:tt)* }) => {{
        $domain.hash($h);
        std::mem::discriminant($value).hash($h);
        match $value {
            $($arms)*
        }
    }};
}

pub fn structural_hash(program: &oxide_parser::Program) -> u64 {
    use std::hash::Hasher;

    let mut h = rustc_hash::FxHasher::default();

    for stmt in &program.body {
        hash_statement(stmt, &mut h);
    }

    h.finish()
}

fn hash_statement(stmt: &Statement, h: &mut rustc_hash::FxHasher) {
    hash_match!(HashDomain::Statement, stmt, h, {
        Statement::ExpressionStatement(es) => {
            hash_expression(&es.expression, h);
        }
        Statement::VariableDeclaration(decl) => {
            std::mem::discriminant(&decl.kind).hash(h);
            (decl.declarations.len() as u32).hash(h);
            for d in &decl.declarations {
                if let Some(init) = &d.init {
                    hash_expression(init, h);
                }
            }
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                hash_expression(arg, h);
            }
        }
        Statement::IfStatement(ifs) => {
            hash_expression(&ifs.test, h);
            hash_statement(&ifs.consequent, h);
            if let Some(alt) = &ifs.alternate {
                hash_statement(alt, h);
            }
        }
        Statement::WhileStatement(wh) => {
            hash_expression(&wh.test, h);
            hash_statement(&wh.body, h);
        }
        Statement::ForStatement(fr) => {
            if let Some(init) = &fr.init {
                if let Some(expr) = init.as_expression() {
                    hash_expression(expr, h);
                } else if let ForStatementInit::VariableDeclaration(decl) = init {
                    std::mem::discriminant(&decl.kind).hash(h);
                    (decl.declarations.len() as u32).hash(h);
                    for d in &decl.declarations {
                        if let Some(init_expr) = &d.init {
                            hash_expression(init_expr, h);
                        }
                    }
                }
            }
            if let Some(test) = &fr.test {
                hash_expression(test, h);
            }
            if let Some(update) = &fr.update {
                hash_expression(update, h);
            }
            hash_statement(&fr.body, h);
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                hash_statement(s, h);
            }
        }
        Statement::BreakStatement(_) => {}
        Statement::ContinueStatement(_) => {}
        Statement::DoWhileStatement(dw) => {
            hash_statement(&dw.body, h);
            hash_expression(&dw.test, h);
        }
        Statement::ForInStatement(fi) => {
            hash_expression(&fi.right, h);
            hash_statement(&fi.body, h);
        }
        Statement::SwitchStatement(sw) => {
            hash_expression(&sw.discriminant, h);
            for case in &sw.cases {
                if let Some(test) = &case.test {
                    hash_expression(test, h);
                }
                for s in &case.consequent {
                    hash_statement(s, h);
                }
            }
        }
        Statement::FunctionDeclaration(fd) => {
            if let Some(id) = &fd.id {
                id.name.as_str().hash(h);
            }
            (fd.params.items.len() as u32).hash(h);
            if let Some(body) = &fd.body {
                for s in &body.statements {
                    hash_statement(s, h);
                }
            }
        }
        Statement::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                id.name.as_str().hash(h);
            }
            class.super_class.is_some().hash(h);
            if let Some(super_class) = &class.super_class {
                hash_expression(super_class, h);
            }
            for element in &class.body.body {
                hash_class_element(element, h);
            }
        }
        Statement::ThrowStatement(ts) => {
            hash_expression(&ts.argument, h);
        }
        Statement::TryStatement(ts) => {
            for s in &ts.block.body {
                hash_statement(s, h);
            }
            if let Some(catch) = &ts.handler {
                if let Some(param) = &catch.param {
                    if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                        bi.name.as_str().hash(h);
                    }
                }
                for s in &catch.body.body {
                    hash_statement(s, h);
                }
            }
            if let Some(finally) = &ts.finalizer {
                for s in &finally.body {
                    hash_statement(s, h);
                }
            }
        }
        _ => {}
    });
}

fn hash_property_key(key: &PropertyKey, h: &mut rustc_hash::FxHasher) {
    hash_match!(HashDomain::PropertyKey, key, h, {
        PropertyKey::StaticIdentifier(ident) => {
            ident.name.as_str().hash(h);
        }
        PropertyKey::Identifier(ident) => {
            ident.name.as_str().hash(h);
        }
        PropertyKey::StringLiteral(s) => {
            s.value.hash(h);
        }
        PropertyKey::NumericLiteral(n) => {
            n.value.to_bits().hash(h);
        }
        PropertyKey::PrivateIdentifier(pi) => {
            pi.name.as_str().hash(h);
        }
        _ => {
            hash_expression(key.to_expression(), h);
        }
    });
}

fn hash_class_element(element: &ClassElement, h: &mut rustc_hash::FxHasher) {
    hash_match!(HashDomain::ClassElement, element, h, {
        ClassElement::MethodDefinition(method) => {
            method.r#static.hash(h);
            method.computed.hash(h);
            std::mem::discriminant(&method.kind).hash(h);
            hash_property_key(&method.key, h);
            if method.computed {
                hash_property_key(&method.key, h);
            }
            (method.value.params.items.len() as u32).hash(h);
            if let Some(body) = &method.value.body {
                for stmt in &body.statements {
                    hash_statement(stmt, h);
                }
            }
        }
        ClassElement::PropertyDefinition(prop) => {
            prop.r#static.hash(h);
            prop.computed.hash(h);
            hash_property_key(&prop.key, h);
            if let Some(value) = &prop.value {
                hash_expression(value, h);
            }
        }
        ClassElement::StaticBlock(block) => {
            for stmt in &block.body {
                hash_statement(stmt, h);
            }
        }
        _ => {}
    });
}

fn hash_expression(expr: &Expression, h: &mut rustc_hash::FxHasher) {
    hash_match!(HashDomain::Expression, expr, h, {
        Expression::BinaryExpression(bin) => {
            std::mem::discriminant(&bin.operator).hash(h);
            hash_expression(&bin.left, h);
            hash_expression(&bin.right, h);
        }
        Expression::UnaryExpression(un) => {
            std::mem::discriminant(&un.operator).hash(h);
            hash_expression(&un.argument, h);
        }
        Expression::CallExpression(call) => {
            (call.arguments.len() as u32).hash(h);
            hash_expression(&call.callee, h);
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    hash_expression(expr, h);
                }
            }
        }
        Expression::LogicalExpression(log) => {
            std::mem::discriminant(&log.operator).hash(h);
            hash_expression(&log.left, h);
            hash_expression(&log.right, h);
        }
        Expression::ConditionalExpression(cond) => {
            hash_expression(&cond.test, h);
            hash_expression(&cond.consequent, h);
            hash_expression(&cond.alternate, h);
        }
        Expression::Identifier(_) => {}
        Expression::NumericLiteral(num) => {
            num.value.to_bits().hash(h);
        }
        Expression::StringLiteral(s) => {
            s.value.hash(h);
        }
        Expression::BooleanLiteral(b) => {
            b.value.hash(h);
        }
        Expression::AssignmentExpression(assign) => {
            std::mem::discriminant(&assign.operator).hash(h);
            hash_expression(&assign.right, h);
        }
        Expression::UpdateExpression(update) => {
            std::mem::discriminant(&update.operator).hash(h);
            update.prefix.hash(h);
            hash_simple_assignment_target(&update.argument, h);
        }
        Expression::TemplateLiteral(tl) => {
            (tl.quasis.len() as u32).hash(h);
            for expr in &tl.expressions {
                hash_expression(expr, h);
            }
        }
        Expression::TaggedTemplateExpression(tt) => {
            hash_expression(&tt.tag, h);
            (tt.quasi.quasis.len() as u32).hash(h);
            for expr in &tt.quasi.expressions {
                hash_expression(expr, h);
            }
        }
        Expression::ArrowFunctionExpression(arrow) => {
            (arrow.params.items.len() as u32).hash(h);
            for s in &arrow.body.statements {
                hash_statement(s, h);
            }
        }
        Expression::FunctionExpression(fe) => {
            (fe.params.items.len() as u32).hash(h);
            if let Some(body) = &fe.body {
                for s in &body.statements {
                    hash_statement(s, h);
                }
            }
        }
        Expression::ClassExpression(class) => {
            if let Some(id) = &class.id {
                id.name.as_str().hash(h);
            }
            class.super_class.is_some().hash(h);
            if let Some(super_class) = &class.super_class {
                hash_expression(super_class, h);
            }
            for element in &class.body.body {
                hash_class_element(element, h);
            }
        }
        Expression::ObjectExpression(obj) => {
            (obj.properties.len() as u32).hash(h);
            for prop in &obj.properties {
                hash_object_property_kind(prop, h);
            }
        }
        Expression::NewExpression(ne) => {
            hash_expression(&ne.callee, h);
            (ne.arguments.len() as u32).hash(h);
            for arg in &ne.arguments {
                if let Some(expr) = arg.as_expression() {
                    hash_expression(expr, h);
                }
            }
        }
        Expression::Super(_) => {}
        Expression::RegExpLiteral(lit) => {
            if let Some(raw) = &lit.raw {
                raw.to_string().hash(h);
            }
        }
        _ => {}
    });
}

fn hash_object_property_kind(prop: &ObjectPropertyKind, h: &mut rustc_hash::FxHasher) {
    hash_match!(HashDomain::ObjectPropertyKind, prop, h, {
        ObjectPropertyKind::ObjectProperty(p) => {
            std::mem::discriminant(&p.kind).hash(h);
            p.method.hash(h);
            p.computed.hash(h);
            hash_property_key(&p.key, h);
            hash_expression(&p.value, h);
        }
        ObjectPropertyKind::SpreadProperty(_) => {}
    });
}

fn hash_simple_assignment_target(target: &SimpleAssignmentTarget, h: &mut rustc_hash::FxHasher) {
    hash_match!(HashDomain::SimpleAssignmentTarget, target, h, {
        SimpleAssignmentTarget::AssignmentTargetIdentifier(_) => {}
        SimpleAssignmentTarget::StaticMemberExpression(member) => {
            hash_expression(&member.object, h);
        }
        SimpleAssignmentTarget::ComputedMemberExpression(member) => {
            hash_expression(&member.object, h);
            hash_expression(&member.expression, h);
        }
        _ => {}
    });
}
