//! 语句 AST 节点的结构哈希。

use super::*;

pub(super) fn hash_statement(stmt: &Statement, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    hash_match!(HashDomain::Statement, stmt, h, {
        Statement::ExpressionStatement(es) => {
            expression::hash_expression(&es.expression, h, include_binding_names);
        }
        Statement::VariableDeclaration(decl) => {
            hash_variable_declaration(decl, h, include_binding_names);
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                expression::hash_expression(arg, h, include_binding_names);
            }
        }
        Statement::IfStatement(ifs) => {
            expression::hash_expression(&ifs.test, h, include_binding_names);
            hash_statement(&ifs.consequent, h, include_binding_names);
            if let Some(alt) = &ifs.alternate {
                hash_statement(alt, h, include_binding_names);
            }
        }
        Statement::WhileStatement(wh) => {
            expression::hash_expression(&wh.test, h, include_binding_names);
            hash_statement(&wh.body, h, include_binding_names);
        }
        Statement::ForStatement(fr) => {
            hash_for_statement(fr, h, include_binding_names);
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                hash_statement(s, h, include_binding_names);
            }
        }
        Statement::BreakStatement(b) => {
            if let Some(label) = &b.label {
                label.name.as_str().hash(h);
            }
        }
        Statement::ContinueStatement(c) => {
            if let Some(label) = &c.label {
                label.name.as_str().hash(h);
            }
        }
        Statement::LabeledStatement(ls) => {
            ls.label.name.as_str().hash(h);
            hash_statement(&ls.body, h, include_binding_names);
        }
        Statement::DoWhileStatement(dw) => {
            hash_statement(&dw.body, h, include_binding_names);
            expression::hash_expression(&dw.test, h, include_binding_names);
        }
        Statement::ForInStatement(fi) => {
            hash_for_statement_left(&fi.left, h, include_binding_names);
            expression::hash_expression(&fi.right, h, include_binding_names);
            hash_statement(&fi.body, h, include_binding_names);
        }
        Statement::ForOfStatement(fo) => {
            fo.r#await.hash(h);
            hash_for_statement_left(&fo.left, h, include_binding_names);
            expression::hash_expression(&fo.right, h, include_binding_names);
            hash_statement(&fo.body, h, include_binding_names);
        }
        Statement::WithStatement(ws) => {
            expression::hash_expression(&ws.object, h, include_binding_names);
            hash_statement(&ws.body, h, include_binding_names);
        }
        Statement::EmptyStatement(_) => {
            1u8.hash(h);
        }
        Statement::DebuggerStatement(_) => {
            2u8.hash(h);
        }
        Statement::SwitchStatement(sw) => {
            expression::hash_expression(&sw.discriminant, h, include_binding_names);
            for case in &sw.cases {
                if let Some(test) = &case.test {
                    expression::hash_expression(test, h, include_binding_names);
                }
                for s in &case.consequent {
                    hash_statement(s, h, include_binding_names);
                }
            }
        }
        Statement::FunctionDeclaration(fd) => {
            hash_function_declaration(fd, h, include_binding_names);
        }
        Statement::ClassDeclaration(class) => {
            hash_class(class, h, include_binding_names);
        }
        Statement::ThrowStatement(ts) => {
            expression::hash_expression(&ts.argument, h, include_binding_names);
        }
        Statement::TryStatement(ts) => {
            hash_try_statement(ts, h, include_binding_names);
        }
        Statement::ImportDeclaration(imp) => {
            imp.source.value.hash(h);
            if let Some(specifiers) = &imp.specifiers {
                (specifiers.len() as u32).hash(h);
                for spec in specifiers {
                    match spec {
                        ImportDeclarationSpecifier::ImportSpecifier(s) => {
                            hash_module_export_name(&s.imported, h);
                            if include_binding_names {
                                s.local.name.as_str().hash(h);
                            }
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                            if include_binding_names {
                                s.local.name.as_str().hash(h);
                            }
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                            if include_binding_names {
                                s.local.name.as_str().hash(h);
                            }
                        }
                    }
                }
            }
        }
        Statement::ExportNamedDeclaration(exp) => {
            if let Some(decl) = &exp.declaration {
                hash_declaration(decl, h, include_binding_names);
            }
            (exp.specifiers.len() as u32).hash(h);
            for spec in &exp.specifiers {
                hash_module_export_name(&spec.local, h);
                hash_module_export_name(&spec.exported, h);
            }
            if let Some(source) = &exp.source {
                source.value.hash(h);
            }
        }
        Statement::ExportDefaultDeclaration(exp) => {
            match &exp.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                    hash_function_declaration(fd, h, include_binding_names);
                }
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    hash_class(class, h, include_binding_names);
                }
                other => {
                    if let Some(expr) = other.as_expression() {
                        expression::hash_expression(expr, h, include_binding_names);
                    }
                }
            }
        }
        Statement::ExportAllDeclaration(exp) => {
            if let Some(exported) = &exp.exported {
                hash_module_export_name(exported, h);
            }
            exp.source.value.hash(h);
        }
        // TS 声明/JSX 等变体在 JS 模式（SourceType::unambiguous）下不可达，落入兜底。
        _ => {}
    });
}

pub(super) fn hash_variable_declaration(
    decl: &oxide_parser::VariableDeclaration<'_>, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
    std::mem::discriminant(&decl.kind).hash(h);
    (decl.declarations.len() as u32).hash(h);
    for d in &decl.declarations {
        if include_binding_names {
            hash_binding_pattern(&d.id, h);
        }
        if let Some(init) = &d.init {
            expression::hash_expression(init, h, include_binding_names);
        }
    }
}

fn hash_for_statement(fr: &oxide_parser::ForStatement<'_>, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    if let Some(init) = &fr.init {
        if let Some(expr) = init.as_expression() {
            expression::hash_expression(expr, h, include_binding_names);
        } else if let ForStatementInit::VariableDeclaration(decl) = init {
            hash_variable_declaration(decl, h, include_binding_names);
        }
    }
    if let Some(test) = &fr.test {
        expression::hash_expression(test, h, include_binding_names);
    }
    if let Some(update) = &fr.update {
        expression::hash_expression(update, h, include_binding_names);
    }
    hash_statement(&fr.body, h, include_binding_names);
}

fn hash_function_declaration(
    fd: &Function<'_>, h: &mut rustc_hash::FxHasher, include_binding_names: bool,
) {
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

fn hash_class(class: &Class<'_>, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    if let Some(id) = &class.id {
        id.name.as_str().hash(h);
    }
    class.super_class.is_some().hash(h);
    if let Some(super_class) = &class.super_class {
        expression::hash_expression(super_class, h, include_binding_names);
    }
    for element in &class.body.body {
        class::hash_class_element(element, h, include_binding_names);
    }
}

fn hash_declaration(decl: &Declaration<'_>, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    match decl {
        Declaration::VariableDeclaration(vd) => hash_variable_declaration(vd, h, include_binding_names),
        Declaration::FunctionDeclaration(fd) => hash_function_declaration(fd, h, include_binding_names),
        Declaration::ClassDeclaration(class) => hash_class(class, h, include_binding_names),
        _ => {}
    }
}

fn hash_module_export_name(name: &ModuleExportName<'_>, h: &mut rustc_hash::FxHasher) {
    match name {
        ModuleExportName::IdentifierName(ident) => ident.name.as_str().hash(h),
        ModuleExportName::IdentifierReference(ident) => ident.name.as_str().hash(h),
        ModuleExportName::StringLiteral(s) => s.value.hash(h),
    }
}

fn hash_for_statement_left(left: &ForStatementLeft<'_>, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
    std::mem::discriminant(left).hash(h);
    if let ForStatementLeft::VariableDeclaration(decl) = left {
        hash_variable_declaration(decl, h, include_binding_names);
    } else if let Some(target) = left.as_simple_assignment_target() {
        target::hash_simple_assignment_target(target, h, include_binding_names);
    }
}

fn hash_try_statement(ts: &oxide_parser::TryStatement<'_>, h: &mut rustc_hash::FxHasher, include_binding_names: bool) {
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
