use super::*;

pub(super) fn hash_expression(expr: &Expression, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
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
        Expression::SequenceExpression(seq) => {
            (seq.expressions.len() as u32).hash(h);
            for e in &seq.expressions {
                hash_expression(e, h, include_binding_names);
            }
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
                target::hash_simple_assignment_target(target, h, include_binding_names);
            }
            hash_expression(&assign.right, h, include_binding_names);
        }
        Expression::UpdateExpression(update) => {
            std::mem::discriminant(&update.operator).hash(h);
            update.prefix.hash(h);
            target::hash_simple_assignment_target(&update.argument, h, include_binding_names);
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
                statement::hash_statement(s, h, include_binding_names);
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
                    statement::hash_statement(s, h, include_binding_names);
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
                class::hash_class_element(element, h, include_binding_names);
            }
        }
        Expression::ObjectExpression(obj) => {
            (obj.properties.len() as u32).hash(h);
            for prop in &obj.properties {
                property::hash_object_property_kind(prop, h, include_binding_names);
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

pub(super) fn hash_chain_element(element: &ChainElement, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
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
