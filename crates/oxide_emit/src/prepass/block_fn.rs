//! emit 前置 pass，块级函数名收集：web-compat 外层 var 绑定实例化支撑——
//! 语句树内块级函数声明名源序去重收集、词法声明名抑制集（块函数名与词法名
//! 同名时退化为纯块作用域）、形参名 ∪ 词法名抑制集构建。

use std::collections::HashSet;

use crate::Emitter;
use oxide_parser::{Declaration, Statement, VariableDeclarationKind};

impl Emitter {
    /// 递归收集语句树内块级函数声明名（源序去重），供 web-compat 外层 var 绑定
    /// 实例化。
    ///
    /// # 边界与前提
    /// - 调用点直接子级（函数体/程序顶层直接子句）是提升 var 声明，非块级
    ///   函数，不收集；进入嵌套语句（块/if/循环/switch case/try 各区）后的
    ///   函数声明才是块级。
    /// - 不递归进嵌套函数体：那是独立编译单元，其内部块函数名由该单元自身收集。
    /// - switch 不推作用域（case 内函数声明随外层块），收集须进入 case 才能到达
    ///   case 内的 for/块等块级形。
    pub(crate) fn collect_block_function_names(&self, statements: &[Statement]) -> Vec<String> {
        let mut names = Vec::new();
        self.collect_block_function_names_into(statements, &mut names, false);
        names
    }

    fn collect_block_function_names_into(&self, statements: &[Statement], names: &mut Vec<String>, block_level: bool) {
        for statement in statements {
            match statement {
                Statement::FunctionDeclaration(f) => {
                    if block_level {
                        if let Some(id) = &f.id {
                            let name = id.name.to_string();
                            if !names.contains(&name) {
                                names.push(name);
                            }
                        }
                    }
                }
                Statement::BlockStatement(bs) => self.collect_block_function_names_into(&bs.body, names, true),
                Statement::IfStatement(is) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&is.consequent), names, true);
                    if let Some(alt) = &is.alternate {
                        self.collect_block_function_names_into(std::slice::from_ref(alt), names, true);
                    }
                }
                Statement::WhileStatement(wh) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&wh.body), names, true)
                }
                Statement::DoWhileStatement(dw) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&dw.body), names, true)
                }
                Statement::ForStatement(fs) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&fs.body), names, true)
                }
                Statement::ForInStatement(fi) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&fi.body), names, true)
                }
                Statement::ForOfStatement(fo) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&fo.body), names, true)
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        self.collect_block_function_names_into(&case.consequent, names, true);
                    }
                }
                Statement::TryStatement(ts) => {
                    self.collect_block_function_names_into(&ts.block.body, names, true);
                    if let Some(handler) = &ts.handler {
                        self.collect_block_function_names_into(&handler.body.body, names, true);
                    }
                    if let Some(finalizer) = &ts.finalizer {
                        self.collect_block_function_names_into(&finalizer.body, names, true);
                    }
                }
                Statement::LabeledStatement(ls) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&ls.body), names, true)
                }
                Statement::WithStatement(ws) => {
                    self.collect_block_function_names_into(std::slice::from_ref(&ws.body), names, true)
                }
                _ => {}
            }
        }
    }

    /// 收集语句树内的词法声明名（let/const/class 含解构叶、for 头词法名、catch
    /// 参数）：块级函数名与树内任意词法声明同名时退化为纯块作用域（无外层
    /// 绑定、无求值写回）。
    ///
    /// # 边界与前提
    /// - 不递归进嵌套函数体/class 元素体：独立编译单元，其词法名不撞本单元的
    ///   块函数名。
    /// - var 声明是 var 绑定，不撞词法环境，不入集。
    fn collect_lexical_name_suppressions(&self, statements: &[Statement], names: &mut HashSet<String>) {
        for statement in statements {
            match statement {
                Statement::VariableDeclaration(vd) => {
                    if !matches!(vd.kind, VariableDeclarationKind::Var) {
                        for d in &vd.declarations {
                            self.collect_binding_pattern_names(&d.id, names);
                        }
                    }
                }
                Statement::ClassDeclaration(cd) => {
                    if let Some(id) = &cd.id {
                        names.insert(id.name.to_string());
                    }
                }
                Statement::ForStatement(fs) => {
                    if let Some(oxide_parser::ForStatementInit::VariableDeclaration(decl)) = &fs.init {
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                self.collect_binding_pattern_names(&d.id, names);
                            }
                        }
                    }
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&fs.body), names);
                }
                Statement::ForInStatement(fi) => {
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(decl) = &fi.left {
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                self.collect_binding_pattern_names(&d.id, names);
                            }
                        }
                    }
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&fi.body), names);
                }
                Statement::ForOfStatement(fo) => {
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(decl) = &fo.left {
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                self.collect_binding_pattern_names(&d.id, names);
                            }
                        }
                    }
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&fo.body), names);
                }
                Statement::TryStatement(ts) => {
                    // catch 参数是 handler 环境的词法绑定。
                    if let Some(handler) = &ts.handler {
                        if let Some(param) = &handler.param {
                            self.collect_binding_pattern_names(&param.pattern, names);
                        }
                        self.collect_lexical_name_suppressions(&handler.body.body, names);
                    }
                    self.collect_lexical_name_suppressions(&ts.block.body, names);
                    if let Some(finalizer) = &ts.finalizer {
                        self.collect_lexical_name_suppressions(&finalizer.body, names);
                    }
                }
                Statement::BlockStatement(bs) => self.collect_lexical_name_suppressions(&bs.body, names),
                Statement::IfStatement(is) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&is.consequent), names);
                    if let Some(alt) = &is.alternate {
                        self.collect_lexical_name_suppressions(std::slice::from_ref(alt), names);
                    }
                }
                Statement::WhileStatement(wh) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&wh.body), names)
                }
                Statement::DoWhileStatement(dw) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&dw.body), names)
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        self.collect_lexical_name_suppressions(&case.consequent, names);
                    }
                }
                Statement::LabeledStatement(ls) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&ls.body), names)
                }
                Statement::WithStatement(ws) => {
                    self.collect_lexical_name_suppressions(std::slice::from_ref(&ws.body), names)
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(Declaration::VariableDeclaration(vd)) = &exp.declaration {
                        if !matches!(vd.kind, VariableDeclarationKind::Var) {
                            for d in &vd.declarations {
                                self.collect_binding_pattern_names(&d.id, names);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 块级函数名 web-compat 外层绑定抑制集：形参名（含解构叶/rest）∪ 语句树内
    /// 词法声明名。命中抑制的名字不建外层 var 绑定、不求值写回。
    pub(crate) fn collect_block_fn_suppressed_names(
        &self, statements: &[Statement], param_names: &HashSet<String>,
    ) -> HashSet<String> {
        let mut names = param_names.clone();
        self.collect_lexical_name_suppressions(statements, &mut names);
        names
    }
}
