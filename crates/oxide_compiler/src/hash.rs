use oxide_parser::{
    ClassElement, Expression, ForStatementInit, ObjectPropertyKind, PropertyKey, SimpleAssignmentTarget, Statement,
};

pub fn structural_hash(program: &oxide_parser::Program) -> u64 {
    use std::hash::Hasher;

    let mut h = rustc_hash::FxHasher::default();

    for stmt in &program.body {
        hash_statement(stmt, &mut h);
    }

    h.finish()
}

fn hash_statement(stmt: &Statement, h: &mut rustc_hash::FxHasher) {
    use std::hash::Hash;

    match stmt {
        Statement::ExpressionStatement(es) => {
            0u8.hash(h);
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
            2u8.hash(h);
            if let Some(arg) = &ret.argument {
                hash_expression(arg, h);
            }
        }
        Statement::IfStatement(ifs) => {
            3u8.hash(h);
            hash_expression(&ifs.test, h);
            hash_statement(&ifs.consequent, h);
            if let Some(alt) = &ifs.alternate {
                hash_statement(alt, h);
            }
        }
        Statement::WhileStatement(wh) => {
            4u8.hash(h);
            hash_expression(&wh.test, h);
            hash_statement(&wh.body, h);
        }
        Statement::ForStatement(fr) => {
            5u8.hash(h);
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
            6u8.hash(h);
            for s in &block.body {
                hash_statement(s, h);
            }
        }
        Statement::BreakStatement(_) => {
            7u8.hash(h);
        }
        Statement::ContinueStatement(_) => {
            8u8.hash(h);
        }
        Statement::DoWhileStatement(dw) => {
            9u8.hash(h);
            hash_statement(&dw.body, h);
            hash_expression(&dw.test, h);
        }
        Statement::ForInStatement(fi) => {
            10u8.hash(h);
            hash_expression(&fi.right, h);
            hash_statement(&fi.body, h);
        }
        Statement::SwitchStatement(sw) => {
            11u8.hash(h);
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
            12u8.hash(h);
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
            13u8.hash(h);
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
            14u8.hash(h);
            hash_expression(&ts.argument, h);
        }
        Statement::TryStatement(ts) => {
            34u8.hash(h);
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
        _ => {
            std::mem::discriminant(stmt).hash(h);
        }
    }
}

fn hash_property_key(key: &PropertyKey, h: &mut rustc_hash::FxHasher) {
    use std::hash::Hash;

    match key {
        PropertyKey::StaticIdentifier(ident) => {
            0u8.hash(h);
            ident.name.as_str().hash(h);
        }
        PropertyKey::StringLiteral(s) => {
            1u8.hash(h);
            s.value.hash(h);
        }
        PropertyKey::PrivateIdentifier(pi) => {
            2u8.hash(h);
            pi.name.as_str().hash(h);
        }
        _ => {
            3u8.hash(h);
            std::mem::discriminant(key).hash(h);
        }
    }
}

fn hash_class_element(element: &ClassElement, h: &mut rustc_hash::FxHasher) {
    use std::hash::Hash;

    match element {
        ClassElement::MethodDefinition(method) => {
            0u8.hash(h);
            method.r#static.hash(h);
            method.computed.hash(h);
            std::mem::discriminant(&method.kind).hash(h);
            hash_property_key(&method.key, h);
            (method.value.params.items.len() as u32).hash(h);
            if let Some(body) = &method.value.body {
                for stmt in &body.statements {
                    hash_statement(stmt, h);
                }
            }
        }
        _ => {
            1u8.hash(h);
            std::mem::discriminant(element).hash(h);
        }
    }
}

fn hash_expression(expr: &Expression, h: &mut rustc_hash::FxHasher) {
    use std::hash::Hash;

    match expr {
        Expression::BinaryExpression(bin) => {
            0u8.hash(h);
            std::mem::discriminant(&bin.operator).hash(h);
            hash_expression(&bin.left, h);
            hash_expression(&bin.right, h);
        }
        Expression::UnaryExpression(un) => {
            1u8.hash(h);
            std::mem::discriminant(&un.operator).hash(h);
            hash_expression(&un.argument, h);
        }
        Expression::CallExpression(call) => {
            2u8.hash(h);
            (call.arguments.len() as u32).hash(h);
            hash_expression(&call.callee, h);
            for arg in &call.arguments {
                if let Some(expr) = arg.as_expression() {
                    hash_expression(expr, h);
                }
            }
        }
        Expression::LogicalExpression(log) => {
            3u8.hash(h);
            std::mem::discriminant(&log.operator).hash(h);
            hash_expression(&log.left, h);
            hash_expression(&log.right, h);
        }
        Expression::ConditionalExpression(cond) => {
            4u8.hash(h);
            hash_expression(&cond.test, h);
            hash_expression(&cond.consequent, h);
            hash_expression(&cond.alternate, h);
        }
        Expression::Identifier(_) => {
            5u8.hash(h);
        }
        Expression::NumericLiteral(num) => {
            6u8.hash(h);
            num.value.to_bits().hash(h);
        }
        Expression::StringLiteral(s) => {
            7u8.hash(h);
            s.value.hash(h);
        }
        Expression::BooleanLiteral(b) => {
            8u8.hash(h);
            b.value.hash(h);
        }
        Expression::AssignmentExpression(assign) => {
            11u8.hash(h);
            std::mem::discriminant(&assign.operator).hash(h);
            hash_expression(&assign.right, h);
        }
        Expression::UpdateExpression(update) => {
            12u8.hash(h);
            std::mem::discriminant(&update.operator).hash(h);
            update.prefix.hash(h);
            match &update.argument {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(_) => {
                    0u8.hash(h);
                }
                SimpleAssignmentTarget::StaticMemberExpression(member) => {
                    1u8.hash(h);
                    hash_expression(&member.object, h);
                }
                SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                    2u8.hash(h);
                    hash_expression(&member.object, h);
                    hash_expression(&member.expression, h);
                }
                _ => {}
            }
        }
        Expression::TemplateLiteral(tl) => {
            16u8.hash(h);
            (tl.quasis.len() as u32).hash(h);
            for expr in &tl.expressions {
                hash_expression(expr, h);
            }
        }
        Expression::TaggedTemplateExpression(tt) => {
            17u8.hash(h);
            hash_expression(&tt.tag, h);
            (tt.quasi.quasis.len() as u32).hash(h);
            for expr in &tt.quasi.expressions {
                hash_expression(expr, h);
            }
        }
        Expression::ArrowFunctionExpression(arrow) => {
            15u8.hash(h);
            (arrow.params.items.len() as u32).hash(h);
            for s in &arrow.body.statements {
                hash_statement(s, h);
            }
        }
        Expression::FunctionExpression(fe) => {
            13u8.hash(h);
            (fe.params.items.len() as u32).hash(h);
            if let Some(body) = &fe.body {
                for s in &body.statements {
                    hash_statement(s, h);
                }
            }
        }
        Expression::ClassExpression(class) => {
            18u8.hash(h);
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
            20u8.hash(h);
            (obj.properties.len() as u32).hash(h);
            for prop in &obj.properties {
                match prop {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        0u8.hash(h);
                        std::mem::discriminant(&p.kind).hash(h);
                        p.method.hash(h);
                        p.computed.hash(h);
                        hash_property_key(&p.key, h);
                        hash_expression(&p.value, h);
                    }
                    ObjectPropertyKind::SpreadProperty(_) => {
                        1u8.hash(h);
                    }
                }
            }
        }
        Expression::NewExpression(ne) => {
            14u8.hash(h);
            hash_expression(&ne.callee, h);
            (ne.arguments.len() as u32).hash(h);
            for arg in &ne.arguments {
                if let Some(expr) = arg.as_expression() {
                    hash_expression(expr, h);
                }
            }
        }
        Expression::Super(_) => {
            19u8.hash(h);
        }
        Expression::RegExpLiteral(lit) => {
            if let Some(raw) = &lit.raw {
                raw.to_string().hash(h);
            }
            std::mem::discriminant(expr).hash(h);
        }
        _ => {
            std::mem::discriminant(expr).hash(h);
        }
    }
}
