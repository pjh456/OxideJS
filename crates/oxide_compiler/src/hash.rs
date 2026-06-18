use oxide_parser::{
    BindingPattern, ChainElement, ClassElement, Expression, ForStatementInit, ObjectPropertyKind, PropertyKey,
    SimpleAssignmentTarget, Statement,
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
    hash_program(program, false)
}

pub fn compiled_module_hash(program: &oxide_parser::Program) -> u64 {
    hash_program(program, true)
}

fn hash_program(program: &oxide_parser::Program, include_binding_names: bool) -> u64 {
    use std::hash::Hasher;

    let mut h = rustc_hash::FxHasher::default();

    for stmt in &program.body {
        hash_statement(stmt, &mut h, include_binding_names);
    }

    h.finish()
}

fn hash_binding_pattern(pattern: &BindingPattern, h: &mut rustc_hash::FxHasher) {
    if let BindingPattern::BindingIdentifier(ident) = pattern {
        ident.name.as_str().hash(h);
    }
}

fn hash_statement(stmt: &Statement, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    hash_match!(HashDomain::Statement, stmt, h, {
        Statement::ExpressionStatement(es) => {
            hash_expression(&es.expression, h, include_binding_names);
        }
        Statement::VariableDeclaration(decl) => {
            std::mem::discriminant(&decl.kind).hash(h);
            (decl.declarations.len() as u32).hash(h);
            for d in &decl.declarations {
                if include_binding_names {
                    hash_binding_pattern(&d.id, h);
                }
                if let Some(init) = &d.init {
                    hash_expression(init, h, include_binding_names);
                }
            }
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                hash_expression(arg, h, include_binding_names);
            }
        }
        Statement::IfStatement(ifs) => {
            hash_expression(&ifs.test, h, include_binding_names);
            hash_statement(&ifs.consequent, h, include_binding_names);
            if let Some(alt) = &ifs.alternate {
                hash_statement(alt, h, include_binding_names);
            }
        }
        Statement::WhileStatement(wh) => {
            hash_expression(&wh.test, h, include_binding_names);
            hash_statement(&wh.body, h, include_binding_names);
        }
        Statement::ForStatement(fr) => {
            if let Some(init) = &fr.init {
                if let Some(expr) = init.as_expression() {
                    hash_expression(expr, h, include_binding_names);
                } else if let ForStatementInit::VariableDeclaration(decl) = init {
                    std::mem::discriminant(&decl.kind).hash(h);
                    (decl.declarations.len() as u32).hash(h);
                    for d in &decl.declarations {
                        if include_binding_names {
                            hash_binding_pattern(&d.id, h);
                        }
                        if let Some(init_expr) = &d.init {
                            hash_expression(init_expr, h, include_binding_names);
                        }
                    }
                }
            }
            if let Some(test) = &fr.test {
                hash_expression(test, h, include_binding_names);
            }
            if let Some(update) = &fr.update {
                hash_expression(update, h, include_binding_names);
            }
            hash_statement(&fr.body, h, include_binding_names);
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                hash_statement(s, h, include_binding_names);
            }
        }
        Statement::BreakStatement(_) => {}
        Statement::ContinueStatement(_) => {}
        Statement::DoWhileStatement(dw) => {
            hash_statement(&dw.body, h, include_binding_names);
            hash_expression(&dw.test, h, include_binding_names);
        }
        Statement::ForInStatement(fi) => {
            hash_expression(&fi.right, h, include_binding_names);
            hash_statement(&fi.body, h, include_binding_names);
        }
        Statement::SwitchStatement(sw) => {
            hash_expression(&sw.discriminant, h, include_binding_names);
            for case in &sw.cases {
                if let Some(test) = &case.test {
                    hash_expression(test, h, include_binding_names);
                }
                for s in &case.consequent {
                    hash_statement(s, h, include_binding_names);
                }
            }
        }
        Statement::FunctionDeclaration(fd) => {
            if include_binding_names {
                if let Some(id) = &fd.id {
                    id.name.as_str().hash(h);
                }
            } else if let Some(id) = &fd.id {
                id.name.as_str().hash(h);
            }
            (fd.params.items.len() as u32).hash(h);
            if include_binding_names {
                for param in &fd.params.items {
                    hash_binding_pattern(&param.pattern, h);
                }
            }
            if let Some(body) = &fd.body {
                for s in &body.statements {
                    hash_statement(s, h, include_binding_names);
                }
            }
        }
        Statement::ClassDeclaration(class) => {
            if let Some(id) = &class.id {
                id.name.as_str().hash(h);
            }
            class.super_class.is_some().hash(h);
            if let Some(super_class) = &class.super_class {
                hash_expression(super_class, h, include_binding_names);
            }
            for element in &class.body.body {
                hash_class_element(element, h, include_binding_names);
            }
        }
        Statement::ThrowStatement(ts) => {
            hash_expression(&ts.argument, h, include_binding_names);
        }
        Statement::TryStatement(ts) => {
            for s in &ts.block.body {
                hash_statement(s, h, include_binding_names);
            }
            if let Some(catch) = &ts.handler {
                if let Some(param) = &catch.param {
                    if include_binding_names {
                        hash_binding_pattern(&param.pattern, h);
                    } else if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &param.pattern {
                        bi.name.as_str().hash(h);
                    }
                }
                for s in &catch.body.body {
                    hash_statement(s, h, include_binding_names);
                }
            }
            if let Some(finally) = &ts.finalizer {
                for s in &finally.body {
                    hash_statement(s, h, include_binding_names);
                }
            }
        }
        _ => {}
    });
}

fn hash_property_key(key: &PropertyKey, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
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
            hash_expression(key.to_expression(), h, include_binding_names);
        }
    });
}

fn hash_class_element(element: &ClassElement, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    hash_match!(HashDomain::ClassElement, element, h, {
        ClassElement::MethodDefinition(method) => {
            method.r#static.hash(h);
            method.computed.hash(h);
            std::mem::discriminant(&method.kind).hash(h);
            hash_property_key(&method.key, h, include_binding_names);
            if method.computed {
                hash_property_key(&method.key, h, include_binding_names);
            }
            (method.value.params.items.len() as u32).hash(h);
            if include_binding_names {
                for param in &method.value.params.items {
                    hash_binding_pattern(&param.pattern, h);
                }
            }
            if let Some(body) = &method.value.body {
                for stmt in &body.statements {
                    hash_statement(stmt, h, include_binding_names);
                }
            }
        }
        ClassElement::PropertyDefinition(prop) => {
            prop.r#static.hash(h);
            prop.computed.hash(h);
            hash_property_key(&prop.key, h, include_binding_names);
            if let Some(value) = &prop.value {
                hash_expression(value, h, include_binding_names);
            }
        }
        ClassElement::StaticBlock(block) => {
            for stmt in &block.body {
                hash_statement(stmt, h, include_binding_names);
            }
        }
        _ => {}
    });
}

fn hash_expression(expr: &Expression, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    hash_match!(HashDomain::Expression, expr, h, {
        Expression::BinaryExpression(bin) => {
            std::mem::discriminant(&bin.operator).hash(h);
            hash_expression(&bin.left, h, include_binding_names);
            hash_expression(&bin.right, h, include_binding_names);
        }
        Expression::UnaryExpression(un) => {
            std::mem::discriminant(&un.operator).hash(h);
            hash_expression(&un.argument, h, include_binding_names);
        }
        Expression::CallExpression(call) => {
            (call.arguments.len() as u32).hash(h);
            hash_expression(&call.callee, h, include_binding_names);
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    hash_expression(expr, h, include_binding_names);
                }
            }
        }
        Expression::LogicalExpression(log) => {
            std::mem::discriminant(&log.operator).hash(h);
            hash_expression(&log.left, h, include_binding_names);
            hash_expression(&log.right, h, include_binding_names);
        }
        Expression::ConditionalExpression(cond) => {
            hash_expression(&cond.test, h, include_binding_names);
            hash_expression(&cond.consequent, h, include_binding_names);
            hash_expression(&cond.alternate, h, include_binding_names);
        }
        Expression::PrivateInExpression(pin) => {
            pin.left.name.as_str().hash(h);
            hash_expression(&pin.right, h, include_binding_names);
        }
        Expression::Identifier(ident) => {
            if include_binding_names {
                ident.name.as_str().hash(h);
            }
        }
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
            if let Some(target) = assign.left.as_simple_assignment_target() {
                hash_simple_assignment_target(target, h, include_binding_names);
            }
            hash_expression(&assign.right, h, include_binding_names);
        }
        Expression::UpdateExpression(update) => {
            std::mem::discriminant(&update.operator).hash(h);
            update.prefix.hash(h);
            hash_simple_assignment_target(&update.argument, h, include_binding_names);
        }
        Expression::TemplateLiteral(tl) => {
            (tl.quasis.len() as u32).hash(h);
            for expr in &tl.expressions {
                hash_expression(expr, h, include_binding_names);
            }
        }
        Expression::TaggedTemplateExpression(tt) => {
            hash_expression(&tt.tag, h, include_binding_names);
            (tt.quasi.quasis.len() as u32).hash(h);
            for expr in &tt.quasi.expressions {
                hash_expression(expr, h, include_binding_names);
            }
        }
        Expression::ArrowFunctionExpression(arrow) => {
            (arrow.params.items.len() as u32).hash(h);
            if include_binding_names {
                for param in &arrow.params.items {
                    hash_binding_pattern(&param.pattern, h);
                }
            }
            for s in &arrow.body.statements {
                hash_statement(s, h, include_binding_names);
            }
        }
        Expression::FunctionExpression(fe) => {
            (fe.params.items.len() as u32).hash(h);
            if include_binding_names {
                if let Some(id) = &fe.id {
                    id.name.as_str().hash(h);
                }
                for param in &fe.params.items {
                    hash_binding_pattern(&param.pattern, h);
                }
            }
            if let Some(body) = &fe.body {
                for s in &body.statements {
                    hash_statement(s, h, include_binding_names);
                }
            }
        }
        Expression::ClassExpression(class) => {
            if let Some(id) = &class.id {
                id.name.as_str().hash(h);
            }
            class.super_class.is_some().hash(h);
            if let Some(super_class) = &class.super_class {
                hash_expression(super_class, h, include_binding_names);
            }
            for element in &class.body.body {
                hash_class_element(element, h, include_binding_names);
            }
        }
        Expression::ObjectExpression(obj) => {
            (obj.properties.len() as u32).hash(h);
            for prop in &obj.properties {
                hash_object_property_kind(prop, h, include_binding_names);
            }
        }
        Expression::NewExpression(ne) => {
            hash_expression(&ne.callee, h, include_binding_names);
            (ne.arguments.len() as u32).hash(h);
            for arg in &ne.arguments {
                if let Some(expr) = arg.as_expression() {
                    hash_expression(expr, h, include_binding_names);
                }
            }
        }
        Expression::PrivateFieldExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
            member.field.name.as_str().hash(h);
            member.optional.hash(h);
        }
        Expression::StaticMemberExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
            member.property.name.as_str().hash(h);
            member.optional.hash(h);
        }
        Expression::ComputedMemberExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
            hash_expression(&member.expression, h, include_binding_names);
            member.optional.hash(h);
        }
        Expression::ChainExpression(chain) => {
            hash_chain_element(&chain.expression, h, include_binding_names);
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

fn hash_chain_element(element: &ChainElement, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    std::mem::discriminant(element).hash(h);
    match element {
        ChainElement::StaticMemberExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
            member.property.name.as_str().hash(h);
            member.optional.hash(h);
        }
        ChainElement::ComputedMemberExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
            hash_expression(&member.expression, h, include_binding_names);
            member.optional.hash(h);
        }
        ChainElement::PrivateFieldExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
            member.field.name.as_str().hash(h);
            member.optional.hash(h);
        }
        ChainElement::CallExpression(call) => {
            hash_expression(&call.callee, h, include_binding_names);
            call.optional.hash(h);
            (call.arguments.len() as u32).hash(h);
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    hash_expression(expr, h, include_binding_names);
                }
            }
        }
        ChainElement::TSNonNullExpression(expr) => {
            hash_expression(&expr.expression, h, include_binding_names);
        }
    }
}

fn hash_object_property_kind(prop: &ObjectPropertyKind, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    hash_match!(HashDomain::ObjectPropertyKind, prop, h, {
        ObjectPropertyKind::ObjectProperty(p) => {
            std::mem::discriminant(&p.kind).hash(h);
            p.method.hash(h);
            p.computed.hash(h);
            hash_property_key(&p.key, h, include_binding_names);
            hash_expression(&p.value, h, include_binding_names);
        }
        ObjectPropertyKind::SpreadProperty(_) => {}
    });
}

fn hash_simple_assignment_target(
    target: &SimpleAssignmentTarget, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    hash_match!(HashDomain::SimpleAssignmentTarget, target, h, {
        SimpleAssignmentTarget::AssignmentTargetIdentifier(ident) => {
            if include_binding_names {
                ident.name.as_str().hash(h);
            }
        }
        SimpleAssignmentTarget::StaticMemberExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
        }
        SimpleAssignmentTarget::ComputedMemberExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
            hash_expression(&member.expression, h, include_binding_names);
        }
        SimpleAssignmentTarget::PrivateFieldExpression(member) => {
            hash_expression(&member.object, h, include_binding_names);
            member.field.name.as_str().hash(h);
        }
        _ => {}
    });
}
