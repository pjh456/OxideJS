use oxide_parser::{Expression, Statement};

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
            1u8.hash(h);
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
                } else if init.is_var_declaration() {
                    0u8.hash(h);
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
        _ => {
            std::mem::discriminant(stmt).hash(h);
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
        Expression::CallExpression(_) => {
            2u8.hash(h);
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
        _ => {
            std::mem::discriminant(expr).hash(h);
        }
    }
}
