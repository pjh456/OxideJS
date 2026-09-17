//! 闭包捕获分析，own/var/顶层声明名收集：本函数 own_bindings 名集
//! （参数名 + 变量/函数/import/export 声明，含嵌套块）、var 提升入口
//! 名集、顶层函数声明名集（无序集 + 声明序列表）。

use std::collections::HashSet;

use oxide_parser::{Statement, VariableDeclarationKind};

use super::{collect_binding_pattern_names, collect_for_left_decl_names};

/// 收集语句树中全部 `var` 声明名（var 提升到函数/程序作用域，递归进嵌套块）。
/// 供函数/程序入口批量 MAKE_CELL(undefined) 使用（var 入口实例化语义）。
pub(crate) fn collect_var_binding_names(stmts: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_var_names_stmt(stmts, &mut names);
    names
}

/// 递归遍历语句树收集 `var` 声明名（函数/程序级提升目标），含嵌套块与控制流各分支。
pub(crate) fn collect_var_names_stmt(stmts: &[Statement], out: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(vd) => {
                if matches!(vd.kind, VariableDeclarationKind::Var) {
                    for d in &vd.declarations {
                        collect_binding_pattern_names(&d.id, out);
                    }
                }
            }
            Statement::BlockStatement(b) => collect_var_names_stmt(&b.body, out),
            Statement::IfStatement(is) => {
                collect_var_names_stmt(std::slice::from_ref(&is.consequent), out);
                if let Some(alt) = &is.alternate {
                    collect_var_names_stmt(std::slice::from_ref(alt), out);
                }
            }
            Statement::WhileStatement(w) => collect_var_names_stmt(std::slice::from_ref(&w.body), out),
            Statement::DoWhileStatement(d) => collect_var_names_stmt(std::slice::from_ref(&d.body), out),
            Statement::ForStatement(f) => {
                if let Some(oxide_parser::ForStatementInit::VariableDeclaration(vd)) = &f.init {
                    if matches!(vd.kind, VariableDeclarationKind::Var) {
                        for d in &vd.declarations {
                            collect_binding_pattern_names(&d.id, out);
                        }
                    }
                }
                collect_var_names_stmt(std::slice::from_ref(&f.body), out);
            }
            Statement::ForInStatement(fi) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fi.left {
                    // 仅 var 头名提升到函数作用域，let/const 头名是迭代级绑定。
                    if matches!(vd.kind, VariableDeclarationKind::Var) {
                        collect_for_left_decl_names(&fi.left, out);
                    }
                }
                collect_var_names_stmt(std::slice::from_ref(&fi.body), out);
            }
            Statement::ForOfStatement(fo) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fo.left {
                    // 仅 var 头名提升到函数作用域，let/const 头名是迭代级绑定。
                    if matches!(vd.kind, VariableDeclarationKind::Var) {
                        collect_for_left_decl_names(&fo.left, out);
                    }
                }
                collect_var_names_stmt(std::slice::from_ref(&fo.body), out);
            }
            Statement::SwitchStatement(sw) => {
                for case in &sw.cases {
                    collect_var_names_stmt(&case.consequent, out);
                }
            }
            Statement::TryStatement(ts) => {
                collect_var_names_stmt(&ts.block.body, out);
                if let Some(h) = &ts.handler {
                    collect_var_names_stmt(&h.body.body, out);
                }
                if let Some(f) = &ts.finalizer {
                    collect_var_names_stmt(&f.body, out);
                }
            }
            Statement::LabeledStatement(ls) => collect_var_names_stmt(std::slice::from_ref(&ls.body), out),
            Statement::WithStatement(ws) => collect_var_names_stmt(std::slice::from_ref(&ws.body), out),
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(oxide_parser::Declaration::VariableDeclaration(vd)) = &exp.declaration {
                    if matches!(vd.kind, VariableDeclarationKind::Var) {
                        for d in &vd.declarations {
                            collect_binding_pattern_names(&d.id, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// 收集语句列表**直接子级**函数声明的绑定名（顶层函数声明是全局 var 绑定，
/// 裸读/裸写与 var 名同路由全局对象属性）。不递归进块：块内函数声明是块
/// 作用域，不参与全局对象同步；`export default function` 是 lexical 绑定，不纳。
pub(crate) fn collect_top_level_function_names(stmts: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in stmts {
        if let Statement::FunctionDeclaration(fd) = stmt {
            if let Some(id) = &fd.id {
                names.insert(id.name.to_string());
            }
        }
    }
    names
}

/// 同 [`collect_top_level_function_names`] 的收集面，但按**声明序**返回 Vec
/// （供 GlobalDeclarationInstantiation（脚本顶层声明实例化）检查阶段逆序遍历逐名
/// 去重，规范"逆序首见 = 源序最后声明"；重名只查一次，取源序最后一次声明）。
pub(crate) fn collect_top_level_function_names_ordered(stmts: &[Statement]) -> Vec<String> {
    let mut names = Vec::new();
    for stmt in stmts {
        if let Statement::FunctionDeclaration(fd) = stmt {
            if let Some(id) = &fd.id {
                names.push(id.name.to_string());
            }
        }
    }
    names
}

/// 收集当前函数作用域声明的绑定名（参数 + 变量/函数声明，含嵌套 block，不含嵌套函数体）。
pub(crate) fn collect_own_binding_names(param_names: &[&str], stmts: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    for p in param_names {
        names.insert(p.to_string());
    }
    collect_decl_names_stmt(stmts, &mut names);
    names
}

/// 递归遍历语句树收集当前函数作用域声明的绑定名（变量/函数/类/import/export，
/// 含嵌套块，不进嵌套函数体）。
pub(crate) fn collect_decl_names_stmt(stmts: &[Statement], out: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    collect_binding_pattern_names(&d.id, out);
                }
            }
            Statement::FunctionDeclaration(fd) => {
                if let Some(id) = &fd.id {
                    out.insert(id.name.to_string());
                }
            }
            // class 名是本作用域 const 绑定：须计入 own_bindings，否则嵌套函数
            // 引用类名时捕获分析缺失 → 落符号表兜底（TDZ 误报或读父寄存器残留）。
            Statement::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
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
                                collect_binding_pattern_names(&d.id, out);
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
            Statement::BlockStatement(b) => collect_decl_names_stmt(&b.body, out),
            Statement::IfStatement(is) => {
                collect_decl_names_stmt(std::slice::from_ref(&is.consequent), out);
                if let Some(alt) = &is.alternate {
                    collect_decl_names_stmt(std::slice::from_ref(alt), out);
                }
            }
            Statement::WhileStatement(w) => collect_decl_names_stmt(std::slice::from_ref(&w.body), out),
            Statement::DoWhileStatement(d) => collect_decl_names_stmt(std::slice::from_ref(&d.body), out),
            Statement::ForStatement(f) => {
                if let Some(oxide_parser::ForStatementInit::VariableDeclaration(vd)) = &f.init {
                    for d in &vd.declarations {
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                            out.insert(bi.name.to_string());
                        }
                    }
                }
                collect_decl_names_stmt(std::slice::from_ref(&f.body), out);
            }
            // for-in / for-of 头部的 var 声明是本函数局部绑定（遮蔽外层同名），
            // 须计入 own_bindings，否则闭包捕获分析会误判为捕获外层变量。
            Statement::ForInStatement(fi) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(_) = &fi.left {
                    collect_for_left_decl_names(&fi.left, out);
                }
                collect_decl_names_stmt(std::slice::from_ref(&fi.body), out);
            }
            Statement::ForOfStatement(fo) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(_) = &fo.left {
                    collect_for_left_decl_names(&fo.left, out);
                }
                collect_decl_names_stmt(std::slice::from_ref(&fo.body), out);
            }
            Statement::SwitchStatement(sw) => {
                for case in &sw.cases {
                    collect_decl_names_stmt(&case.consequent, out);
                }
            }
            Statement::TryStatement(ts) => {
                collect_decl_names_stmt(&ts.block.body, out);
                if let Some(h) = &ts.handler {
                    collect_decl_names_stmt(&h.body.body, out);
                }
                if let Some(f) = &ts.finalizer {
                    collect_decl_names_stmt(&f.body, out);
                }
            }
            Statement::LabeledStatement(ls) => collect_decl_names_stmt(std::slice::from_ref(&ls.body), out),
            Statement::WithStatement(ws) => collect_decl_names_stmt(std::slice::from_ref(&ws.body), out),
            _ => {}
        }
    }
}
