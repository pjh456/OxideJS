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

/// 收集语句列表**直接子级**词法声明名（let/const，含解构 pattern 叶）：
/// 入口 TDZ 占位 cell 名集（被捕获的词法绑定在入口未初始化）。
///
/// 遍历面与 `predeclare_lexical_declarations` 一致——只走直接子级，不递归块/
/// if/for/switch case/try 区（块级名不提升到函数/程序作用域，不建入口
/// cell）；for 头声明是循环作用域绑定，不纳；var 声明不纳（var 入口实例化
/// 独立覆盖）；import 绑定由模块导入初始化的发射点建 cell，不纳。
pub(crate) fn collect_direct_lexical_names(stmts: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                if matches!(decl.kind, VariableDeclarationKind::Var) {
                    continue;
                }
                for d in &decl.declarations {
                    collect_binding_pattern_names(&d.id, &mut names);
                }
            }
            Statement::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
                    names.insert(id.name.to_string());
                }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(decl) = &exp.declaration {
                    match decl {
                        oxide_parser::Declaration::VariableDeclaration(vd) => {
                            if matches!(vd.kind, VariableDeclarationKind::Var) {
                                continue;
                            }
                            for d in &vd.declarations {
                                collect_binding_pattern_names(&d.id, &mut names);
                            }
                        }
                        oxide_parser::Declaration::ClassDeclaration(cd) => {
                            if let Some(id) = &cd.id {
                                names.insert(id.name.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                if let oxide_parser::ExportDefaultDeclarationKind::ClassDeclaration(cd) = &exp.declaration {
                    if let Some(id) = &cd.id {
                        names.insert(id.name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    names
}

/// 收集块级声明名（块内 let/const/class、catch 参数、for 头 let/const）：
/// 这些名字是块作用域绑定，遮蔽同名顶层 tier 名时，tier 名须保留在捕获集，
/// 块级绑定才能追加新索引供闭包捕获。顶层直接子级 let/const/class 是顶层
/// 词法绑定，不纳；var 声明不纳（var 是函数/全局作用域）。
pub(crate) fn collect_block_level_decl_names(stmts: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_block_level_names_stmt(stmts, &mut names, true);
    names
}

/// 递归遍历语句树收集块级声明名。`top` 标记当前层是否为顶层（顶层直接子级
/// let/const/class 是顶层词法绑定，非块级）。
fn collect_block_level_names_stmt(stmts: &[Statement], out: &mut HashSet<String>, top: bool) {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(vd) => {
                if matches!(vd.kind, VariableDeclarationKind::Var) || top {
                    continue;
                }
                for d in &vd.declarations {
                    collect_binding_pattern_names(&d.id, out);
                }
            }
            Statement::ClassDeclaration(cd) => {
                if !top {
                    if let Some(id) = &cd.id {
                        out.insert(id.name.to_string());
                    }
                }
            }
            Statement::BlockStatement(b) => collect_block_level_names_stmt(&b.body, out, false),
            Statement::IfStatement(is) => {
                collect_block_level_names_stmt(std::slice::from_ref(&is.consequent), out, false);
                if let Some(alt) = &is.alternate {
                    collect_block_level_names_stmt(std::slice::from_ref(alt), out, false);
                }
            }
            Statement::WhileStatement(w) => collect_block_level_names_stmt(std::slice::from_ref(&w.body), out, false),
            Statement::DoWhileStatement(d) => collect_block_level_names_stmt(std::slice::from_ref(&d.body), out, false),
            Statement::ForStatement(f) => {
                if let Some(oxide_parser::ForStatementInit::VariableDeclaration(vd)) = &f.init {
                    if !matches!(vd.kind, VariableDeclarationKind::Var) {
                        for d in &vd.declarations {
                            collect_binding_pattern_names(&d.id, out);
                        }
                    }
                }
                collect_block_level_names_stmt(std::slice::from_ref(&f.body), out, false);
            }
            Statement::ForInStatement(fi) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fi.left {
                    if !matches!(vd.kind, VariableDeclarationKind::Var) {
                        collect_for_left_decl_names(&fi.left, out);
                    }
                }
                collect_block_level_names_stmt(std::slice::from_ref(&fi.body), out, false);
            }
            Statement::ForOfStatement(fo) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fo.left {
                    if !matches!(vd.kind, VariableDeclarationKind::Var) {
                        collect_for_left_decl_names(&fo.left, out);
                    }
                }
                collect_block_level_names_stmt(std::slice::from_ref(&fo.body), out, false);
            }
            Statement::SwitchStatement(sw) => {
                for case in &sw.cases {
                    collect_block_level_names_stmt(&case.consequent, out, false);
                }
            }
            Statement::TryStatement(ts) => {
                collect_block_level_names_stmt(&ts.block.body, out, false);
                if let Some(h) = &ts.handler {
                    if let Some(param) = &h.param {
                        collect_binding_pattern_names(&param.pattern, out);
                    }
                    collect_block_level_names_stmt(&h.body.body, out, false);
                }
                if let Some(f) = &ts.finalizer {
                    collect_block_level_names_stmt(&f.body, out, false);
                }
            }
            Statement::LabeledStatement(ls) => {
                collect_block_level_names_stmt(std::slice::from_ref(&ls.body), out, false)
            }
            Statement::WithStatement(ws) => collect_block_level_names_stmt(std::slice::from_ref(&ws.body), out, false),
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(decl) = &exp.declaration {
                    match decl {
                        oxide_parser::Declaration::VariableDeclaration(vd) => {
                            if !matches!(vd.kind, VariableDeclarationKind::Var) && !top {
                                for d in &vd.declarations {
                                    collect_binding_pattern_names(&d.id, out);
                                }
                            }
                        }
                        oxide_parser::Declaration::ClassDeclaration(cd) if !top => {
                            if let Some(id) = &cd.id {
                                out.insert(id.name.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
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

/// 函数作用域名收集结果：单趟遍历同时填六个输出，与旧的六个独立收集函数逐位一致。
///
/// 字段含义与旧函数一一对应（var 提升名 / own 绑定名 / 块级声明名 / 直接子词法名 /
/// 顶层函数名集 / 顶层函数声明序）。调用点改经本结构取各面，旧收集器保留待删。
pub(crate) struct FnScopeNames {
    /// var 声明名（函数/程序作用域提升目标，递归进嵌套块）。
    pub(crate) var_binding_names: HashSet<String>,
    /// 当前函数作用域绑定名（参数 + 变量/函数/类/import/export 声明，含嵌套块，不含嵌套函数体）。
    pub(crate) own_binding_names: HashSet<String>,
    /// 块级声明名（块内 let/const/class、catch 参数、for 头 let/const；顶层直接子不纳）。
    pub(crate) block_level_decl_names: HashSet<String>,
    /// 直接子级词法声明名（let/const/class，入口 TDZ 占位 cell 名集）。
    pub(crate) direct_lexical_names: HashSet<String>,
    /// 直接子级函数声明名（无序集）。
    pub(crate) top_level_function_names: HashSet<String>,
    /// 直接子级函数声明名（声明序，不排序不去重）。
    pub(crate) top_level_function_names_ordered: Vec<String>,
}

/// 单趟收集函数作用域六个名字输出。
///
/// 步骤：
/// 1. 形参名注入 `own_binding_names`（须在遍历前，own 面是遮蔽集，形参须先入）。
/// 2. 以 `top=true` 递归遍历顶层语句列表，直接子级填直接子词法/顶层函数名输出，
///    各递归层填 var/own/块级输出。
///
/// 边界：`param_names` 含解构叶名与 rest 名（规范 `paramNames` 口径）；顶层语句列表
/// 不递归进嵌套函数体（嵌套函数是独立编译单元）。
/// 副作用：无（纯收集）。
/// 注意事项：六个字段与旧六函数逐位一致，调用点切换后发射产物不变。
pub(crate) fn collect_fn_scope_names(param_names: &[&str], stmts: &[Statement]) -> FnScopeNames {
    let mut out = FnScopeNames {
        var_binding_names: HashSet::new(),
        own_binding_names: HashSet::new(),
        block_level_decl_names: HashSet::new(),
        direct_lexical_names: HashSet::new(),
        top_level_function_names: HashSet::new(),
        top_level_function_names_ordered: Vec::new(),
    };
    for p in param_names {
        out.own_binding_names.insert(p.to_string());
    }
    collect_fn_scope_names_impl(stmts, &mut out, true);
    out
}

/// 递归遍历语句树单趟填六个输出。`top` 标记当前层是否为顶层（直接子级）：顶层填
/// 直接子词法与顶层函数名输出，且顶层 let/const/class 是顶层词法绑定非块级；
/// 各递归层（块/if/循环/switch case/try/标签/with）以 `top=false` 进入。
fn collect_fn_scope_names_impl(stmts: &[Statement], out: &mut FnScopeNames, top: bool) {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(vd) => {
                let is_var = matches!(vd.kind, VariableDeclarationKind::Var);
                // var 面：仅 var 名提升到函数作用域。
                if is_var {
                    for d in &vd.declarations {
                        collect_binding_pattern_names(&d.id, &mut out.var_binding_names);
                    }
                }
                // own 面：全部声明种类计入当前作用域绑定。
                for d in &vd.declarations {
                    collect_binding_pattern_names(&d.id, &mut out.own_binding_names);
                }
                // 块级面：非顶层的非 var（顶层词法是顶层绑定，非块级）。
                if !is_var && !top {
                    for d in &vd.declarations {
                        collect_binding_pattern_names(&d.id, &mut out.block_level_decl_names);
                    }
                }
                // 直接子词法面：仅顶层非 var。
                if top && !is_var {
                    for d in &vd.declarations {
                        collect_binding_pattern_names(&d.id, &mut out.direct_lexical_names);
                    }
                }
            }
            Statement::FunctionDeclaration(fd) => {
                if let Some(id) = &fd.id {
                    // own 面：函数声明是当前作用域绑定。
                    out.own_binding_names.insert(id.name.to_string());
                    if top {
                        // 顶层函数名集与声明序列表（顶层函数声明是全局 var 绑定）。
                        out.top_level_function_names.insert(id.name.to_string());
                        out.top_level_function_names_ordered.push(id.name.to_string());
                    }
                }
            }
            Statement::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
                    let name = id.name.to_string();
                    // own 面：类名是本作用域 const 绑定。
                    out.own_binding_names.insert(name.clone());
                    if top {
                        // 直接子词法面：顶层类是顶层词法绑定。
                        out.direct_lexical_names.insert(name);
                    } else {
                        // 块级面：块内类是块作用域绑定。
                        out.block_level_decl_names.insert(name);
                    }
                }
            }
            Statement::ImportDeclaration(imp) => {
                // own 面：import 绑定是模块作用域 const 绑定。
                if let Some(specifiers) = &imp.specifiers {
                    for sp in specifiers {
                        match sp {
                            oxide_parser::ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                out.own_binding_names.insert(s.local.name.to_string());
                            }
                            oxide_parser::ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                out.own_binding_names.insert(s.local.name.to_string());
                            }
                            oxide_parser::ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                out.own_binding_names.insert(s.local.name.to_string());
                            }
                        }
                    }
                }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(decl) = &exp.declaration {
                    match decl {
                        oxide_parser::Declaration::VariableDeclaration(vd) => {
                            let is_var = matches!(vd.kind, VariableDeclarationKind::Var);
                            if is_var {
                                for d in &vd.declarations {
                                    collect_binding_pattern_names(&d.id, &mut out.var_binding_names);
                                }
                            }
                            for d in &vd.declarations {
                                collect_binding_pattern_names(&d.id, &mut out.own_binding_names);
                            }
                            if !is_var && !top {
                                for d in &vd.declarations {
                                    collect_binding_pattern_names(&d.id, &mut out.block_level_decl_names);
                                }
                            }
                            if top && !is_var {
                                for d in &vd.declarations {
                                    collect_binding_pattern_names(&d.id, &mut out.direct_lexical_names);
                                }
                            }
                        }
                        oxide_parser::Declaration::FunctionDeclaration(fd) => {
                            if let Some(id) = &fd.id {
                                out.own_binding_names.insert(id.name.to_string());
                            }
                        }
                        oxide_parser::Declaration::ClassDeclaration(cd) => {
                            if let Some(id) = &cd.id {
                                let name = id.name.to_string();
                                out.own_binding_names.insert(name.clone());
                                if top {
                                    out.direct_lexical_names.insert(name);
                                } else {
                                    out.block_level_decl_names.insert(name);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                match &exp.declaration {
                    oxide_parser::ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                        if let Some(id) = &fd.id {
                            out.own_binding_names.insert(id.name.to_string());
                        }
                    }
                    oxide_parser::ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                        if let Some(id) = &cd.id {
                            let name = id.name.to_string();
                            out.own_binding_names.insert(name.clone());
                            // 直接子词法面仅顶层：export default class 是 lexical 绑定。
                            if top {
                                out.direct_lexical_names.insert(name);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Statement::BlockStatement(b) => collect_fn_scope_names_impl(&b.body, out, false),
            Statement::IfStatement(is) => {
                collect_fn_scope_names_impl(std::slice::from_ref(&is.consequent), out, false);
                if let Some(alt) = &is.alternate {
                    collect_fn_scope_names_impl(std::slice::from_ref(alt), out, false);
                }
            }
            Statement::WhileStatement(w) => collect_fn_scope_names_impl(std::slice::from_ref(&w.body), out, false),
            Statement::DoWhileStatement(d) => collect_fn_scope_names_impl(std::slice::from_ref(&d.body), out, false),
            Statement::ForStatement(f) => {
                if let Some(oxide_parser::ForStatementInit::VariableDeclaration(vd)) = &f.init {
                    let is_var = matches!(vd.kind, VariableDeclarationKind::Var);
                    if is_var {
                        for d in &vd.declarations {
                            collect_binding_pattern_names(&d.id, &mut out.var_binding_names);
                        }
                    }
                    // own 面：for 头声明是本函数局部绑定（仅标识符形态，与旧趟一致）。
                    for d in &vd.declarations {
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                            out.own_binding_names.insert(bi.name.to_string());
                        }
                    }
                    // 块级面：for 头 let/const 是循环作用域绑定（各层均收集）。
                    if !is_var {
                        for d in &vd.declarations {
                            collect_binding_pattern_names(&d.id, &mut out.block_level_decl_names);
                        }
                    }
                }
                collect_fn_scope_names_impl(std::slice::from_ref(&f.body), out, false);
            }
            Statement::ForInStatement(fi) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fi.left {
                    let is_var = matches!(vd.kind, VariableDeclarationKind::Var);
                    if is_var {
                        // 仅 var 头名提升到函数作用域，let/const 头名是迭代级绑定。
                        collect_for_left_decl_names(&fi.left, &mut out.var_binding_names);
                    }
                    collect_for_left_decl_names(&fi.left, &mut out.own_binding_names);
                    if !is_var {
                        collect_for_left_decl_names(&fi.left, &mut out.block_level_decl_names);
                    }
                }
                collect_fn_scope_names_impl(std::slice::from_ref(&fi.body), out, false);
            }
            Statement::ForOfStatement(fo) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fo.left {
                    let is_var = matches!(vd.kind, VariableDeclarationKind::Var);
                    if is_var {
                        collect_for_left_decl_names(&fo.left, &mut out.var_binding_names);
                    }
                    collect_for_left_decl_names(&fo.left, &mut out.own_binding_names);
                    if !is_var {
                        collect_for_left_decl_names(&fo.left, &mut out.block_level_decl_names);
                    }
                }
                collect_fn_scope_names_impl(std::slice::from_ref(&fo.body), out, false);
            }
            Statement::SwitchStatement(sw) => {
                for case in &sw.cases {
                    collect_fn_scope_names_impl(&case.consequent, out, false);
                }
            }
            Statement::TryStatement(ts) => {
                // catch 参数是本作用域的词法绑定：own 与块级两面均计入。
                if let Some(h) = &ts.handler {
                    if let Some(param) = &h.param {
                        collect_binding_pattern_names(&param.pattern, &mut out.own_binding_names);
                        collect_binding_pattern_names(&param.pattern, &mut out.block_level_decl_names);
                    }
                    collect_fn_scope_names_impl(&h.body.body, out, false);
                }
                collect_fn_scope_names_impl(&ts.block.body, out, false);
                if let Some(f) = &ts.finalizer {
                    collect_fn_scope_names_impl(&f.body, out, false);
                }
            }
            Statement::LabeledStatement(ls) => collect_fn_scope_names_impl(std::slice::from_ref(&ls.body), out, false),
            Statement::WithStatement(ws) => collect_fn_scope_names_impl(std::slice::from_ref(&ws.body), out, false),
            _ => {}
        }
    }
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
                    // catch 参数是本函数作用域的词法绑定：计入 own_bindings，
                    // 嵌套函数引用它时捕获分析才会建 cell 供 upvalue 读取。
                    if let Some(param) = &h.param {
                        collect_binding_pattern_names(&param.pattern, out);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 夹具：parse 源串（脚本口径）后，六输出逐一断言新单趟与旧六函数逐位一致。
    fn assert_bitwise(src: &str, params: &[&str]) {
        let alloc = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&alloc, src).unwrap_or_else(|e| panic!("parse 失败 {src:?}: {e:?}"));
        let body = &program.body;

        let names = collect_fn_scope_names(params, body);
        assert_eq!(names.var_binding_names, collect_var_binding_names(body), "var 面漂移：{src:?}");
        assert_eq!(names.own_binding_names, collect_own_binding_names(params, body), "own 面漂移：{src:?}");
        assert_eq!(names.block_level_decl_names, collect_block_level_decl_names(body), "块级面漂移：{src:?}");
        assert_eq!(
            names.direct_lexical_names,
            collect_direct_lexical_names(body),
            "直接子词法面漂移：{src:?}"
        );
        assert_eq!(
            names.top_level_function_names,
            collect_top_level_function_names(body),
            "顶层函数名面漂移：{src:?}"
        );
        assert_eq!(
            names.top_level_function_names_ordered,
            collect_top_level_function_names_ordered(body),
            "顶层函数声明序漂移：{src:?}"
        );
    }

    /// 夹具：parse 源串（模块口径）后，六输出逐一断言新单趟与旧六函数逐位一致。
    fn assert_bitwise_module(src: &str, params: &[&str]) {
        let alloc = oxide_parser::Allocator::default();
        let program =
            oxide_parser::parse_module(&alloc, src).unwrap_or_else(|e| panic!("parse_module 失败 {src:?}: {e:?}"));
        let body = &program.body;

        let names = collect_fn_scope_names(params, body);
        assert_eq!(names.var_binding_names, collect_var_binding_names(body), "var 面漂移：{src:?}");
        assert_eq!(names.own_binding_names, collect_own_binding_names(params, body), "own 面漂移：{src:?}");
        assert_eq!(names.block_level_decl_names, collect_block_level_decl_names(body), "块级面漂移：{src:?}");
        assert_eq!(
            names.direct_lexical_names,
            collect_direct_lexical_names(body),
            "直接子词法面漂移：{src:?}"
        );
        assert_eq!(
            names.top_level_function_names,
            collect_top_level_function_names(body),
            "顶层函数名面漂移：{src:?}"
        );
        assert_eq!(
            names.top_level_function_names_ordered,
            collect_top_level_function_names_ordered(body),
            "顶层函数声明序漂移：{src:?}"
        );
    }

    /// 顶层声明面：var/let/const/函数/类声明与解构 pattern 叶名。
    #[test]
    fn fn_scope_names_bitwise_parity_declarations() {
        assert_bitwise("var a; let b = 1; const c = 2; function f() {} class K {}", &[]);
        assert_bitwise("var a = {x: 1, y: [z]}; let [p, ...q] = arr; let {m, n: o} = obj;", &[]);
        assert_bitwise("function f() { var inner; } function g() {} var f2;", &[]);
        assert_bitwise("var a; var a; function f() {} function f() {}", &[]);
    }

    /// 控制流面：if/while/do-while/for/for-in/for-of/switch/try/labeled/with 各分支。
    #[test]
    fn fn_scope_names_bitwise_parity_control_flow() {
        assert_bitwise("if (x) { var a; let b; } else { var c; let d; }", &[]);
        assert_bitwise("while (x) { var a; let b; } do { var c; } while (x);", &[]);
        assert_bitwise("for (var i = 0; i < n; i++) { var a; let b; }", &[]);
        assert_bitwise("for (let j of it) { var a; } for (var k in obj) { let b; }", &[]);
        assert_bitwise("switch (x) { case 1: { var a; let b; } default: { var c; } }", &[]);
        assert_bitwise("try { var a; } catch (e) { let b; } finally { var c; }", &[]);
        assert_bitwise("try { var a; } catch { let b; }", &[]);
        assert_bitwise("L: { var a; let b; }", &[]);
        assert_bitwise("with (obj) { var a; let b; }", &[]);
    }

    /// 嵌套块面：块内 let/const/class 是块级绑定，顶层直接子不纳。
    #[test]
    fn fn_scope_names_bitwise_parity_nested_blocks() {
        assert_bitwise("{ { var a; let b; class K {} } }", &[]);
        assert_bitwise("if (x) { { let a; const b = 1; } }", &[]);
        assert_bitwise("var a; { let b; } function f() { var c; }", &[]);
        assert_bitwise("if (x) while (y) { let a; } for (var i;;) { const b = 1; }", &[]);
    }

    /// 形参注入面：param_names 进 own 面，须与旧 collect_own_binding_names 同口径。
    #[test]
    fn fn_scope_names_bitwise_parity_params_injection() {
        assert_bitwise("var p; let q; function f() {}", &["p", "q", "r"]);
        assert_bitwise("var x = 1;", &["x", "y"]);
        assert_bitwise("function f() { var p; }", &["p"]);
    }

    /// 模块声明面：import/export 包装的声明计入各面（模块口径 parse）。
    #[test]
    fn fn_scope_names_bitwise_parity_module_decls() {
        assert_bitwise_module("import a from 'm'; import * as ns from 'm'; import { b, c as d } from 'm';", &[]);
        assert_bitwise_module("export var e = 1; export let f = 2; export const g = 3;", &[]);
        assert_bitwise_module("export function h() {} export class J {}", &[]);
        assert_bitwise_module("export default function named() {}", &[]);
        assert_bitwise_module("export default class K {}", &[]);
        assert_bitwise_module("import a from 'm'; export var b = 1; function c() {}", &[]);
        assert_bitwise_module("export { x } from 'n';", &[]);
    }

    /// for 头面：var/let/const 头名分面归属（var 提升、let/const 块级）。
    #[test]
    fn fn_scope_names_bitwise_parity_for_heads() {
        assert_bitwise("for (var [a, b] of it) {} for (let {x, ...y} in obj) {}", &[]);
        assert_bitwise("for (var i;;) {} for (let j;;) {} for (const k = 0;;) {}", &[]);
        assert_bitwise("for (var i = 0;;) { for (let j;;) { var a; } }", &[]);
    }
}
