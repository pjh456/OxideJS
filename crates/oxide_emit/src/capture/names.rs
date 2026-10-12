//! 闭包捕获分析，函数作用域名收集：单趟收集八个名字面（var 提升名 / own 绑定名 /
//! 块级声明名 / 直接子词法名 / 顶层函数名集 / 顶层函数声明序 / 块级函数名 /
//! 块函数抑制集），供各入口（程序/模块/函数体）的 GDI 检查、builtin 排除、
//! 入口实例化与块函数 web-compat 各调用点共用；另保留捕获扫描器内部依赖的
//! own 绑定名集收集（`collect_own_binding_names_into`）。

use std::collections::{HashMap, HashSet};

use oxide_parser::{Statement, VariableDeclarationKind};

use super::{
    collect_binding_pattern_names, collect_binding_pattern_names_into, collect_for_left_decl_names,
    collect_for_left_decl_names_into,
};

/// 收集当前函数作用域声明的绑定名（参数 + 变量/函数声明，含嵌套 block，不含嵌套函数体），
/// 写入影子计次映射（计次加一）。
pub(crate) fn collect_own_binding_names_into(
    param_names: &[&str], stmts: &[Statement], out: &mut HashMap<String, u32>,
) {
    for p in param_names {
        *out.entry(p.to_string()).or_insert(0) += 1;
    }
    collect_decl_names_into(stmts, out);
}

/// 函数作用域名收集结果：单趟遍历同时填八个输出，与各旧独立收集函数逐位一致。
///
/// 字段含义（var 提升名 / own 绑定名 / 块级声明名 / 直接子词法名 / 顶层函数名集 /
/// 顶层函数声明序 / 块级函数名 / 块函数抑制集）。调用点经本结构取各面。
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
    /// 块级普通函数声明名（源序去重；直接子级是提升 var 声明不纳，async/生成器
    /// 是块级词法绑定不纳，不递归嵌套函数体）。
    pub(crate) block_function_names: Vec<String>,
    /// 块级函数名的外层 var 绑定抑制集（形参名 ∪ 树内词法声明名；命中抑制的名字
    /// 不建外层 var 绑定、不求值写回）。
    pub(crate) block_fn_suppressed: HashSet<String>,
}

/// 单趟收集函数作用域八个名字输出。
///
/// 步骤：
/// 1. 形参名注入 `own_binding_names` 与 `block_fn_suppressed`（须在遍历前，
///    own 面是遮蔽集、抑制集以形参名为基底，形参须先入）。
/// 2. 以 `top=true` 递归遍历顶层语句列表，直接子级填直接子词法/顶层函数名/
///    块函数名输出，各递归层填 var/own/块级/抑制输出。
///
/// 边界：`param_names` 含解构叶名与 rest 名（规范 `paramNames` 口径）；顶层语句列表
/// 不递归进嵌套函数体（嵌套函数是独立编译单元）。
/// 副作用：无（纯收集）。
/// 注意事项：八个字段与旧八收集函数逐位一致，调用点切换后发射产物不变。
pub(crate) fn collect_fn_scope_names(param_names: &[&str], stmts: &[Statement]) -> FnScopeNames {
    let mut out = FnScopeNames {
        var_binding_names: HashSet::new(),
        own_binding_names: HashSet::new(),
        block_level_decl_names: HashSet::new(),
        direct_lexical_names: HashSet::new(),
        top_level_function_names: HashSet::new(),
        top_level_function_names_ordered: Vec::new(),
        block_function_names: Vec::new(),
        block_fn_suppressed: HashSet::new(),
    };
    for p in param_names {
        let name = p.to_string();
        out.own_binding_names.insert(name.clone());
        out.block_fn_suppressed.insert(name);
    }
    collect_fn_scope_names_impl(stmts, &mut out, true);
    out
}

/// 递归遍历语句树单趟填八个输出。`top` 标记当前层是否为顶层（直接子级）：顶层填
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
                // 抑制面：词法名与块函数名同名时退化为纯块作用域（各层均收集）。
                if !is_var {
                    for d in &vd.declarations {
                        collect_binding_pattern_names(&d.id, &mut out.block_fn_suppressed);
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
                    } else if !fd.generator && !fd.r#async {
                        // 块级函数面：仅普通函数（async/生成器是块级词法绑定），
                        // 源序去重，重名只记一次。
                        let name = id.name.to_string();
                        if !out.block_function_names.contains(&name) {
                            out.block_function_names.push(name);
                        }
                    }
                }
            }
            Statement::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
                    let name = id.name.to_string();
                    // own 面：类名是本作用域 const 绑定。
                    out.own_binding_names.insert(name.clone());
                    // 抑制面：类名是词法声明，与块函数名同名时退化纯块作用域。
                    out.block_fn_suppressed.insert(name.clone());
                    if top {
                        // 直接子词法面：顶层类是顶层词法绑定。
                        out.direct_lexical_names.insert(name.clone());
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
                            // 抑制面：export 包装的词法名与块函数名同名时退化纯块作用域。
                            if !is_var {
                                for d in &vd.declarations {
                                    collect_binding_pattern_names(&d.id, &mut out.block_fn_suppressed);
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
                    // 抑制面：for 头词法名与块函数名同名时退化纯块作用域。
                    if !is_var {
                        for d in &vd.declarations {
                            collect_binding_pattern_names(&d.id, &mut out.block_fn_suppressed);
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
                        // 抑制面：for 头词法名与块函数名同名时退化纯块作用域。
                        collect_for_left_decl_names(&fi.left, &mut out.block_fn_suppressed);
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
                        // 抑制面：for 头词法名与块函数名同名时退化纯块作用域。
                        collect_for_left_decl_names(&fo.left, &mut out.block_fn_suppressed);
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
                // 先块后 handler 再 finalizer：块级函数名列表按源序去重，块在前
                // 与旧收集器逐位一致（各集合面不受访问序影响）。
                collect_fn_scope_names_impl(&ts.block.body, out, false);
                if let Some(h) = &ts.handler {
                    // catch 参数是本作用域的词法绑定：own、块级与抑制三面均计入。
                    if let Some(param) = &h.param {
                        collect_binding_pattern_names(&param.pattern, &mut out.own_binding_names);
                        collect_binding_pattern_names(&param.pattern, &mut out.block_level_decl_names);
                        collect_binding_pattern_names(&param.pattern, &mut out.block_fn_suppressed);
                    }
                    collect_fn_scope_names_impl(&h.body.body, out, false);
                }
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
/// 含嵌套块，不进嵌套函数体），写入影子计次映射（计次加一）。
pub(crate) fn collect_decl_names_into(stmts: &[Statement], out: &mut HashMap<String, u32>) {
    for stmt in stmts {
        match stmt {
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    collect_binding_pattern_names_into(&d.id, out);
                }
            }
            Statement::FunctionDeclaration(fd) => {
                if let Some(id) = &fd.id {
                    *out.entry(id.name.to_string()).or_insert(0) += 1;
                }
            }
            // class 名是本作用域 const 绑定：须计入 own_bindings，否则嵌套函数
            // 引用类名时捕获分析缺失 → 落符号表兜底（TDZ 误报或读父寄存器残留）。
            Statement::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
                    *out.entry(id.name.to_string()).or_insert(0) += 1;
                }
            }
            // import 绑定是模块作用域 const 绑定，须计入 own_bindings 供
            // 嵌套函数 cell 捕获。
            Statement::ImportDeclaration(imp) => {
                if let Some(specifiers) = &imp.specifiers {
                    for sp in specifiers {
                        match sp {
                            oxide_parser::ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                *out.entry(s.local.name.to_string()).or_insert(0) += 1;
                            }
                            oxide_parser::ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                *out.entry(s.local.name.to_string()).or_insert(0) += 1;
                            }
                            oxide_parser::ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                *out.entry(s.local.name.to_string()).or_insert(0) += 1;
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
                                collect_binding_pattern_names_into(&d.id, out);
                            }
                        }
                        oxide_parser::Declaration::FunctionDeclaration(fd) => {
                            if let Some(id) = &fd.id {
                                *out.entry(id.name.to_string()).or_insert(0) += 1;
                            }
                        }
                        oxide_parser::Declaration::ClassDeclaration(cd) => {
                            if let Some(id) = &cd.id {
                                *out.entry(id.name.to_string()).or_insert(0) += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
                oxide_parser::ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                    if let Some(id) = &fd.id {
                        *out.entry(id.name.to_string()).or_insert(0) += 1;
                    }
                }
                oxide_parser::ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                    if let Some(id) = &cd.id {
                        *out.entry(id.name.to_string()).or_insert(0) += 1;
                    }
                }
                _ => {}
            },
            Statement::BlockStatement(b) => collect_decl_names_into(&b.body, out),
            Statement::IfStatement(is) => {
                collect_decl_names_into(std::slice::from_ref(&is.consequent), out);
                if let Some(alt) = &is.alternate {
                    collect_decl_names_into(std::slice::from_ref(alt), out);
                }
            }
            Statement::WhileStatement(w) => collect_decl_names_into(std::slice::from_ref(&w.body), out),
            Statement::DoWhileStatement(d) => collect_decl_names_into(std::slice::from_ref(&d.body), out),
            Statement::ForStatement(f) => {
                if let Some(oxide_parser::ForStatementInit::VariableDeclaration(vd)) = &f.init {
                    for d in &vd.declarations {
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                            *out.entry(bi.name.to_string()).or_insert(0) += 1;
                        }
                    }
                }
                collect_decl_names_into(std::slice::from_ref(&f.body), out);
            }
            // for-in / for-of 头部的 var 声明是本函数局部绑定（遮蔽外层同名），
            // 须计入 own_bindings，否则闭包捕获分析会误判为捕获外层变量。
            Statement::ForInStatement(fi) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(_) = &fi.left {
                    collect_for_left_decl_names_into(&fi.left, out);
                }
                collect_decl_names_into(std::slice::from_ref(&fi.body), out);
            }
            Statement::ForOfStatement(fo) => {
                if let oxide_parser::ForStatementLeft::VariableDeclaration(_) = &fo.left {
                    collect_for_left_decl_names_into(&fo.left, out);
                }
                collect_decl_names_into(std::slice::from_ref(&fo.body), out);
            }
            Statement::SwitchStatement(sw) => {
                for case in &sw.cases {
                    collect_decl_names_into(&case.consequent, out);
                }
            }
            Statement::TryStatement(ts) => {
                collect_decl_names_into(&ts.block.body, out);
                if let Some(h) = &ts.handler {
                    // catch 参数是本函数作用域的词法绑定：计入 own_bindings，
                    // 嵌套函数引用它时捕获分析才会建 cell 供 upvalue 读取。
                    if let Some(param) = &h.param {
                        collect_binding_pattern_names_into(&param.pattern, out);
                    }
                    collect_decl_names_into(&h.body.body, out);
                }
                if let Some(f) = &ts.finalizer {
                    collect_decl_names_into(&f.body, out);
                }
            }
            Statement::LabeledStatement(ls) => collect_decl_names_into(std::slice::from_ref(&ls.body), out),
            Statement::WithStatement(ws) => collect_decl_names_into(std::slice::from_ref(&ws.body), out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 夹具：parse 源串后断言单趟八输出与钉值逐位一致（脚本口径）。
    #[allow(clippy::too_many_arguments)]
    fn assert_scope_names(
        src: &str, params: &[&str], var: &[&str], own: &[&str], blk: &[&str], lex: &[&str], top: &[&str],
        topo: &[&str], bfn: &[&str], sup: &[&str],
    ) {
        assert_names(src, params, false, var, own, blk, lex, top, topo, bfn, sup);
    }

    /// 夹具：parse 源串后断言单趟八输出与钉值逐位一致（模块口径）。
    #[allow(clippy::too_many_arguments)]
    fn assert_scope_names_module(
        src: &str, params: &[&str], var: &[&str], own: &[&str], blk: &[&str], lex: &[&str], top: &[&str],
        topo: &[&str], bfn: &[&str], sup: &[&str],
    ) {
        assert_names(src, params, true, var, own, blk, lex, top, topo, bfn, sup);
    }

    #[allow(clippy::too_many_arguments)]
    fn assert_names(
        src: &str, params: &[&str], is_module: bool, var: &[&str], own: &[&str], blk: &[&str], lex: &[&str],
        top: &[&str], topo: &[&str], bfn: &[&str], sup: &[&str],
    ) {
        let alloc = oxide_parser::Allocator::default();
        let program = if is_module {
            oxide_parser::parse_module(&alloc, src).unwrap_or_else(|e| panic!("parse_module 失败 {src:?}: {e:?}"))
        } else {
            oxide_parser::parse(&alloc, src).unwrap_or_else(|e| panic!("parse 失败 {src:?}: {e:?}"))
        };
        let names = collect_fn_scope_names(params, &program.body);
        let eq = |got: &HashSet<String>, want: &[&str], face: &str| {
            let want_set: HashSet<String> = want.iter().map(|s| s.to_string()).collect();
            assert_eq!(got, &want_set, "{face} 面漂移：{src:?}");
        };
        eq(&names.var_binding_names, var, "var");
        eq(&names.own_binding_names, own, "own");
        eq(&names.block_level_decl_names, blk, "块级");
        eq(&names.direct_lexical_names, lex, "直接子词法");
        eq(&names.top_level_function_names, top, "顶层函数");
        assert_eq!(
            &names.top_level_function_names_ordered,
            &topo.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "顶层函数声明序漂移：{src:?}"
        );
        assert_eq!(
            &names.block_function_names,
            &bfn.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "块函数面漂移：{src:?}"
        );
        eq(&names.block_fn_suppressed, sup, "抑制");
    }

    /// 顶层声明面：var/let/const/函数/类声明与解构 pattern 叶名。
    #[test]
    fn fn_scope_names_declarations() {
        assert_scope_names(
            "var a; let b = 1; const c = 2; function f() {} class K {}",
            &[],
            &["a"],
            &["K", "a", "b", "c", "f"],
            &[],
            &["K", "b", "c"],
            &["f"],
            &["f"],
            &[],
            &["K", "b", "c"],
        );
        assert_scope_names(
            "var a = {x: 1, y: [z]}; let [p, ...q] = arr; let {m, n: o} = obj;",
            &[],
            &["a"],
            &["a", "m", "o", "p", "q"],
            &[],
            &["m", "o", "p", "q"],
            &[],
            &[],
            &[],
            &["m", "o", "p", "q"],
        );
        assert_scope_names(
            "function f() { var inner; } function g() {} var f2;",
            &[],
            &["f2"],
            &["f", "f2", "g"],
            &[],
            &[],
            &["f", "g"],
            &["f", "g"],
            &[],
            &[],
        );
        // 重名顶层函数：名集去重、声明序保留两份。
        assert_scope_names(
            "var a; var a; function f() {} function f() {}",
            &[],
            &["a"],
            &["a", "f"],
            &[],
            &[],
            &["f"],
            &["f", "f"],
            &[],
            &[],
        );
    }

    /// 控制流面：if/while/do-while/for/for-in/for-of/switch/try/labeled/with 各分支。
    #[test]
    fn fn_scope_names_control_flow() {
        assert_scope_names(
            "if (x) { var a; let b; } else { var c; let d; }",
            &[],
            &["a", "c"],
            &["a", "b", "c", "d"],
            &["b", "d"],
            &[],
            &[],
            &[],
            &[],
            &["b", "d"],
        );
        assert_scope_names(
            "while (x) { var a; let b; } do { var c; } while (x);",
            &[],
            &["a", "c"],
            &["a", "b", "c"],
            &["b"],
            &[],
            &[],
            &[],
            &[],
            &["b"],
        );
        assert_scope_names(
            "for (var i = 0; i < n; i++) { var a; let b; }",
            &[],
            &["a", "i"],
            &["a", "b", "i"],
            &["b"],
            &[],
            &[],
            &[],
            &[],
            &["b"],
        );
        assert_scope_names(
            "for (let j of it) { var a; } for (var k in obj) { let b; }",
            &[],
            &["a", "k"],
            &["a", "b", "j", "k"],
            &["b", "j"],
            &[],
            &[],
            &[],
            &[],
            &["b", "j"],
        );
        assert_scope_names(
            "switch (x) { case 1: { var a; let b; } default: { var c; } }",
            &[],
            &["a", "c"],
            &["a", "b", "c"],
            &["b"],
            &[],
            &[],
            &[],
            &[],
            &["b"],
        );
        assert_scope_names(
            "try { var a; } catch (e) { let b; } finally { var c; }",
            &[],
            &["a", "c"],
            &["a", "b", "c", "e"],
            &["b", "e"],
            &[],
            &[],
            &[],
            &[],
            &["b", "e"],
        );
        assert_scope_names(
            "try { var a; } catch { let b; }",
            &[],
            &["a"],
            &["a", "b"],
            &["b"],
            &[],
            &[],
            &[],
            &[],
            &["b"],
        );
        assert_scope_names("L: { var a; let b; }", &[], &["a"], &["a", "b"], &["b"], &[], &[], &[], &[], &["b"]);
        assert_scope_names(
            "with (obj) { var a; let b; }",
            &[],
            &["a"],
            &["a", "b"],
            &["b"],
            &[],
            &[],
            &[],
            &[],
            &["b"],
        );
    }

    /// 嵌套块面：块内 let/const/class 是块级绑定，顶层直接子不纳。
    #[test]
    fn fn_scope_names_nested_blocks() {
        assert_scope_names(
            "{ { var a; let b; class K {} } }",
            &[],
            &["a"],
            &["K", "a", "b"],
            &["K", "b"],
            &[],
            &[],
            &[],
            &[],
            &["K", "b"],
        );
        assert_scope_names(
            "if (x) { { let a; const b = 1; } }",
            &[],
            &[],
            &["a", "b"],
            &["a", "b"],
            &[],
            &[],
            &[],
            &[],
            &["a", "b"],
        );
        assert_scope_names(
            "var a; { let b; } function f() { var c; }",
            &[],
            &["a"],
            &["a", "b", "f"],
            &["b"],
            &[],
            &["f"],
            &["f"],
            &[],
            &["b"],
        );
        assert_scope_names(
            "if (x) while (y) { let a; } for (var i;;) { const b = 1; }",
            &[],
            &["i"],
            &["a", "b", "i"],
            &["a", "b"],
            &[],
            &[],
            &[],
            &[],
            &["a", "b"],
        );
    }

    /// 形参注入面：param_names 进 own 面与抑制面。
    #[test]
    fn fn_scope_names_params_injection() {
        assert_scope_names(
            "var p; let q; function f() {}",
            &["p", "q", "r"],
            &["p"],
            &["f", "p", "q", "r"],
            &[],
            &["q"],
            &["f"],
            &["f"],
            &[],
            &["p", "q", "r"],
        );
        assert_scope_names("var x = 1;", &["x", "y"], &["x"], &["x", "y"], &[], &[], &[], &[], &[], &["x", "y"]);
        // 内层 var 在嵌套函数体内，不提升收集；形参名注入 own 面与抑制面。
        assert_scope_names("function f() { var p; }", &["p"], &[], &["f", "p"], &[], &[], &["f"], &["f"], &[], &["p"]);
    }

    /// 块函数面：源序去重、各控制流下钻、async/生成器排除、直接子排除、嵌套函数不递归。
    #[test]
    fn fn_scope_names_block_fn_names() {
        assert_scope_names(
            "{ function bf() {} } if (x) { function bf2() {} } while (y) { function bf3() {} }",
            &[],
            &[],
            &["bf", "bf2", "bf3"],
            &[],
            &[],
            &[],
            &[],
            &["bf", "bf2", "bf3"],
            &[],
        );
        assert_scope_names(
            "for (var i;;) { function bf4() {} } switch (z) { case 1: { function bf5() {} } }",
            &[],
            &["i"],
            &["bf4", "bf5", "i"],
            &[],
            &[],
            &[],
            &[],
            &["bf4", "bf5"],
            &[],
        );
        assert_scope_names(
            "try { function bf6() {} } catch (e) { function bf7() {} } finally { function bf8() {} }",
            &[],
            &[],
            &["bf6", "bf7", "bf8", "e"],
            &["e"],
            &[],
            &[],
            &[],
            &["bf6", "bf7", "bf8"],
            &["e"],
        );
        assert_scope_names(
            "L2: { function bf9() {} } with (o) { function bf10() {} }",
            &[],
            &[],
            &["bf10", "bf9"],
            &[],
            &[],
            &[],
            &[],
            &["bf9", "bf10"],
            &[],
        );
        // async/生成器是块级词法绑定，不入块函数名集。
        assert_scope_names(
            "async function af() {} { async function af2() {} } { function* g() {} }",
            &[],
            &[],
            &["af", "af2", "g"],
            &[],
            &[],
            &["af"],
            &["af"],
            &[],
            &[],
        );
        // 直接子级函数是提升 var 声明，非块级。
        assert_scope_names(
            "function direct() {} { function inner() {} }",
            &[],
            &[],
            &["direct", "inner"],
            &[],
            &[],
            &["direct"],
            &["direct"],
            &["inner"],
            &[],
        );
        // 重名块函数源序去重，只记一次。
        assert_scope_names(
            "{ function a() {} } { function a() {} }",
            &[],
            &[],
            &["a"],
            &[],
            &[],
            &[],
            &[],
            &["a"],
            &[],
        );
        // 嵌套函数体是独立编译单元，不递归收集。
        assert_scope_names(
            "function outer() { { function inner() {} } }",
            &[],
            &[],
            &["outer"],
            &[],
            &[],
            &["outer"],
            &["outer"],
            &[],
            &[],
        );
    }

    /// 抑制面：词法声明（let/class/for 头/catch 参数/export 词法）与块函数名同名时退化纯块作用域。
    #[test]
    fn fn_scope_names_block_fn_suppressed() {
        assert_scope_names(
            "let shadow; { function shadow() {} }",
            &[],
            &[],
            &["shadow"],
            &[],
            &["shadow"],
            &[],
            &[],
            &["shadow"],
            &["shadow"],
        );
        assert_scope_names("class C {} { function C() {} }", &[], &[], &["C"], &[], &["C"], &[], &[], &["C"], &["C"]);
        assert_scope_names(
            "for (let s;;) { function s() {} }",
            &[],
            &[],
            &["s"],
            &["s"],
            &[],
            &[],
            &[],
            &["s"],
            &["s"],
        );
        // catch 参数与块函数名同名：catch 体外的 if 臂块函数被抑制。
        assert_scope_names(
            "try {} catch (e) {} if (x) { function e() {} }",
            &[],
            &[],
            &["e"],
            &["e"],
            &[],
            &[],
            &[],
            &["e"],
            &["e"],
        );
        // 形参名进抑制面：块函数名撞形参名退化纯块作用域（形参名同时注入 own 面）。
        assert_scope_names(
            "function f(a) { { function a() {} } }",
            &["a"],
            &[],
            &["a", "f"],
            &[],
            &[],
            &["f"],
            &["f"],
            &[],
            &["a"],
        );
    }

    /// 闭包捕获面：var/词法入口 cell 名集与块函数泄漏名混合。
    #[test]
    fn fn_scope_names_capture_faces() {
        assert_scope_names(
            "var v = 1; let l = 2; function f() { return function() { return v + l; }; }",
            &[],
            &["v"],
            &["f", "l", "v"],
            &[],
            &["l"],
            &["f"],
            &["f"],
            &[],
            &["l"],
        );
        assert_scope_names(
            "{ function g() { return function() { return v; }; } var v = 3; }",
            &[],
            &["v"],
            &["g", "v"],
            &[],
            &[],
            &[],
            &[],
            &["g"],
            &[],
        );
    }

    /// 模块声明面：import/export 包装的声明计入各面（模块口径 parse）。
    #[test]
    fn fn_scope_names_module_decls() {
        assert_scope_names_module(
            "import a from 'm'; import * as ns from 'm'; import { b, c as d } from 'm';",
            &[],
            &[],
            &["a", "b", "d", "ns"],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_scope_names_module(
            "export var e = 1; export let f = 2; export const g = 3;",
            &[],
            &["e"],
            &["e", "f", "g"],
            &[],
            &["f", "g"],
            &[],
            &[],
            &[],
            &["f", "g"],
        );
        // export class 不入抑制面（旧收集器只收 export 变量声明）。
        assert_scope_names_module(
            "export function h() {} export class J {}",
            &[],
            &[],
            &["J", "h"],
            &[],
            &["J"],
            &[],
            &[],
            &[],
            &[],
        );
        assert_scope_names_module(
            "export default function named() {}",
            &[],
            &[],
            &["named"],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_scope_names_module("export default class K {}", &[], &[], &["K"], &[], &["K"], &[], &[], &[], &[]);
        assert_scope_names_module(
            "import a from 'm'; export var b = 1; function c() {}",
            &[],
            &["b"],
            &["a", "b", "c"],
            &[],
            &[],
            &["c"],
            &["c"],
            &[],
            &[],
        );
        assert_scope_names_module("export { x } from 'n';", &[], &[], &[], &[], &[], &[], &[], &[], &[]);
    }

    /// for 头面：var/let/const 头名分面归属（var 提升、let/const 块级与抑制）。
    #[test]
    fn fn_scope_names_for_heads() {
        assert_scope_names(
            "for (var [a, b] of it) {} for (let {x, ...y} in obj) {}",
            &[],
            &["a", "b"],
            &["a", "b", "x", "y"],
            &["x", "y"],
            &[],
            &[],
            &[],
            &[],
            &["x", "y"],
        );
        assert_scope_names(
            "for (var i;;) {} for (let j;;) {} for (const k = 0;;) {}",
            &[],
            &["i"],
            &["i", "j", "k"],
            &["j", "k"],
            &[],
            &[],
            &[],
            &[],
            &["j", "k"],
        );
        assert_scope_names(
            "for (var i = 0;;) { for (let j;;) { var a; } }",
            &[],
            &["a", "i"],
            &["a", "i", "j"],
            &["j"],
            &[],
            &[],
            &[],
            &[],
            &["j"],
        );
    }

    /// 模块块函数面：export 词法声明抑制块函数外层 var 绑定。
    #[test]
    fn fn_scope_names_module_block_fn() {
        assert_scope_names_module(
            "var mv = 1; let ml = 2; function mf() { return function() { return mv + ml; }; }",
            &[],
            &["mv"],
            &["mf", "ml", "mv"],
            &[],
            &["ml"],
            &["mf"],
            &["mf"],
            &[],
            &["ml"],
        );
        assert_scope_names_module(
            "export let ex; { function ex() {} }",
            &[],
            &[],
            &["ex"],
            &[],
            &["ex"],
            &[],
            &[],
            &["ex"],
            &["ex"],
        );
        assert_scope_names_module(
            "export let fl = 1; { function fl() {} }",
            &[],
            &[],
            &["fl"],
            &[],
            &["fl"],
            &[],
            &[],
            &["fl"],
            &["fl"],
        );
    }
}
