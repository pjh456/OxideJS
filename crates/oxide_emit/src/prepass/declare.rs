//! emit 前置 pass，声明预登记：函数/var 声明提升预声明、块级函数声明按 ES2015
//! 块绑定预声明、let/const/class 建 TDZ 占位；顶层 lexical 声明撞受限全局名
//! 在声明实例化期拒绝（global_lexical 门控，门控源居 emit_program 调用点）。

use super::check_restricted_global_lexical;
use crate::{CompileCtx, Emitter};
use oxide_parser::{BindingPattern, Declaration, ExportDefaultDeclarationKind, Statement, VariableDeclarationKind};

impl Emitter {
    pub(crate) fn predeclare_function_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            let function = match statement {
                Statement::FunctionDeclaration(f) => f,
                Statement::ExportNamedDeclaration(exp) => match &exp.declaration {
                    Some(Declaration::FunctionDeclaration(f)) => f,
                    _ => continue,
                },
                // export default function foo(){}：foo 绑定由 lexical predeclare 以
                // Const 预声明（emit 侧 emit_bind_target(Const) 消费预登记槽）。
                _ => continue,
            };
            let Some(identifier) = &function.id else {
                continue;
            };
            let reg = ctx.alloc_reg();
            let _ = ctx.declare_initialized(identifier.name.as_str(), reg, VariableDeclarationKind::Var, false);
        }
    }

    /// 预声明函数体直接子级中标签链直接包裹的函数声明名（Annex B.3.2 Labelled
    /// Function Declarations）：sloppy 下标签不改变执行流，该声明的名与直接子
    /// 函数声明一样是函数作用域 `var` 绑定，须在入口物化闭包前完成预登记。
    ///
    /// # 边界与前提
    /// - 仅扫描直接子语句（标签链展开后须是函数声明）；标签包块等其它形归块面
    ///   块级函数预声明路径。
    /// - 与块级函数名收集路径可能对同名重复建绑定：`declare_initialized` 命中
    ///   已存在绑定时只补初始化标志，不改变既有寄存器分配，语义幂等。
    pub(crate) fn predeclare_labeled_function_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            let Some(function) = Self::labeled_function_decl(statement) else {
                continue;
            };
            let Some(identifier) = &function.id else {
                continue;
            };
            let reg = ctx.alloc_reg();
            let _ = ctx.declare_initialized(identifier.name.as_str(), reg, VariableDeclarationKind::Var, false);
        }
    }

    /// 预声明单个 `var` 名：仅顶层（全局作用域）的内置名落内置镜像槽（预载全局
    /// 属性值的固定寄存器槽，本轮求值开始时预载），使 GlobalDeclarationInstantiation
    /// （脚本顶层声明实例化）序言与声明点同步都用入口原值，而非程序求值前
    /// define-if-absent 序列新建的 undefined 槽，现存值（NaN 等）不被抹；函数体内
    /// 内置名仍是局部 var 遮蔽（新建槽，不命中只读内置拦截）；其余名新建 var 槽。
    pub(crate) fn predeclare_var_name(&self, name: &str, ctx: &mut CompileCtx) {
        if ctx.is_global_scope && CompileCtx::is_known_builtin(name) {
            let _ = ctx.lookup_or_builtin(name);
        } else {
            let reg = ctx.alloc_reg();
            let _ = ctx.declare_initialized(name, reg, VariableDeclarationKind::Var, false);
        }
    }

    /// 预声明语句列表中的全部 `var` 绑定，使先发的提升函数声明能解析它们。
    /// 顶层 `var` 名在编译闭包捕获它的函数体时必须可见。
    pub(crate) fn predeclare_var_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            match statement {
                Statement::VariableDeclaration(decl) => {
                    if !matches!(decl.kind, VariableDeclarationKind::Var) {
                        continue;
                    }
                    for d in &decl.declarations {
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                            self.predeclare_var_name(bi.name.as_str(), ctx);
                        }
                    }
                }
                Statement::BlockStatement(bs) => self.predeclare_var_declarations(&bs.body, ctx),
                Statement::IfStatement(is) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&is.consequent), ctx);
                    if let Some(alt) = &is.alternate {
                        self.predeclare_var_declarations(std::slice::from_ref(alt), ctx);
                    }
                }
                Statement::WhileStatement(wh) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&wh.body), ctx);
                }
                Statement::DoWhileStatement(dw) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&dw.body), ctx);
                }
                Statement::ForStatement(fs) => {
                    if let Some(oxide_parser::ForStatementInit::VariableDeclaration(decl)) = &fs.init {
                        if matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                    self.predeclare_var_name(bi.name.as_str(), ctx);
                                }
                            }
                        }
                    }
                    self.predeclare_var_declarations(std::slice::from_ref(&fs.body), ctx);
                }
                Statement::ForInStatement(fi) => {
                    // var 头名提升到函数/全局作用域（与 For 臂同规则），先于提升
                    // 函数声明发射预声明，使函数体按既有 var 绑定解析头名；
                    // let/const 头是迭代级绑定，不入提升面。
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(decl) = &fi.left {
                        if matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                    self.predeclare_var_name(bi.name.as_str(), ctx);
                                }
                            }
                        }
                    }
                    self.predeclare_var_declarations(std::slice::from_ref(&fi.body), ctx);
                }
                Statement::ForOfStatement(fo) => {
                    if let oxide_parser::ForStatementLeft::VariableDeclaration(decl) = &fo.left {
                        if matches!(decl.kind, VariableDeclarationKind::Var) {
                            for d in &decl.declarations {
                                if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                    self.predeclare_var_name(bi.name.as_str(), ctx);
                                }
                            }
                        }
                    }
                    self.predeclare_var_declarations(std::slice::from_ref(&fo.body), ctx);
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        self.predeclare_var_declarations(&case.consequent, ctx);
                    }
                }
                Statement::TryStatement(ts) => {
                    self.predeclare_var_declarations(&ts.block.body, ctx);
                    if let Some(handler) = &ts.handler {
                        self.predeclare_var_declarations(&handler.body.body, ctx);
                    }
                    if let Some(finalizer) = &ts.finalizer {
                        self.predeclare_var_declarations(&finalizer.body, ctx);
                    }
                }
                Statement::LabeledStatement(ls) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&ls.body), ctx);
                }
                Statement::WithStatement(ws) => {
                    self.predeclare_var_declarations(std::slice::from_ref(&ws.body), ctx);
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(Declaration::VariableDeclaration(decl)) = &exp.declaration {
                        if !matches!(decl.kind, VariableDeclarationKind::Var) {
                            continue;
                        }
                        for d in &decl.declarations {
                            if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                self.predeclare_var_name(bi.name.as_str(), ctx);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 预声明当前块直接子树中的函数声明（ES2015 块级绑定，kind=Let 落当前块），
    /// 使块内声明点之前的引用在编译期可见。递归进入嵌套块时压/弹作用域，
    /// 与 emit 阶段嵌套块 push_scope 的时机对齐。
    ///
    /// # 边界与前提
    /// - 只处理块内函数声明；switch 不推 scope（其 case 内函数声明随 switch
    ///   作用域修复一并处理）；export 声明不可能出现在块内。
    /// - `if` 支臂的裸函数声明属 Annex B 隐式支臂块：名已有外层 var 绑定承载
    ///   （非 eval 脚本顶层、未被形参/词法同名抑制、且非脚本顶层只读三常量）时
    ///   不建外层块绑定，声明点物化进该 var 槽（建外层块 Let 会遮蔽同名 var）；
    ///   名无外层 var 绑定（形参/词法抑制、eval 脚本顶层或只读三常量）时保留
    ///   块级绑定承载支臂闭包，避免声明点覆写外层绑定。支臂为块时由该块自身的
    ///   块入口流程处理。
    /// - 与 lexical 预声明的顺序：本函数在前，`{ let g; function g(){} }` 时
    ///   lexical 的 `declare_predeclared` 命中已存在的函数绑定自然报重复声明错，
    ///   避免函数预声明被 lexical 占位静默覆盖而破坏 let 的 TDZ。
    /// - var 与函数同名（`{ var g; function g(){} }`）：var 落函数作用域、
    ///   函数落当前块，不同 scope 互不冲突。
    pub(crate) fn predeclare_block_function_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            match statement {
                Statement::FunctionDeclaration(f) => {
                    let Some(identifier) = &f.id else {
                        continue;
                    };
                    let reg = ctx.alloc_reg();
                    let _ = ctx.declare_initialized(identifier.name.as_str(), reg, VariableDeclarationKind::Let, false);
                }
                Statement::BlockStatement(bs) => {
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(&bs.body, ctx);
                    ctx.pop_scope();
                }
                Statement::IfStatement(is) => {
                    self.predeclare_if_arm_block_function(&is.consequent, ctx);
                    if let Some(alt) = &is.alternate {
                        self.predeclare_if_arm_block_function(alt, ctx);
                    }
                }
                Statement::WhileStatement(wh) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&wh.body), ctx);
                }
                Statement::DoWhileStatement(dw) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&dw.body), ctx);
                }
                Statement::ForStatement(fs) => {
                    // 循环推独立作用域（头声明所在），函数声明落该作用域与 emit 对齐。
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(std::slice::from_ref(&fs.body), ctx);
                    ctx.pop_scope();
                }
                Statement::ForInStatement(fi) => {
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(std::slice::from_ref(&fi.body), ctx);
                    ctx.pop_scope();
                }
                Statement::ForOfStatement(fo) => {
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(std::slice::from_ref(&fo.body), ctx);
                    ctx.pop_scope();
                }
                Statement::TryStatement(ts) => {
                    ctx.push_scope();
                    self.predeclare_block_function_declarations(&ts.block.body, ctx);
                    ctx.pop_scope();
                    if let Some(handler) = &ts.handler {
                        ctx.push_scope();
                        self.predeclare_block_function_declarations(&handler.body.body, ctx);
                        ctx.pop_scope();
                    }
                    if let Some(finalizer) = &ts.finalizer {
                        ctx.push_scope();
                        self.predeclare_block_function_declarations(&finalizer.body, ctx);
                        ctx.pop_scope();
                    }
                }
                Statement::LabeledStatement(ls) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&ls.body), ctx);
                }
                Statement::WithStatement(ws) => {
                    self.predeclare_block_function_declarations(std::slice::from_ref(&ws.body), ctx);
                }
                _ => {}
            }
        }
    }

    /// 预声明 `if` 支臂裸函数声明的块级绑定（展开标签链并下钻嵌套 `if` 后取
    /// 函数声明）：仅当该名没有外层 var 绑定承载时才在当前块建块 Let，承载支臂
    /// 闭包，避免声明点覆写外层形参/词法绑定；名已有外层 var 绑定（非 eval 脚本
    /// 顶层、未被形参/词法同名抑制、且非全局只读内置三常量）时不建块级绑定，
    /// 声明点物化进该 var 槽。
    ///
    /// # 边界与前提
    /// - 支臂为块时返回：该块自身的块入口流程预声明其函数声明。
    /// - 支臂为嵌套 `if`（含 `else if` 链）时沿 consequent/alternate 下钻至叶子，
    ///   叶子按同一门控判定——嵌套支臂与单层支臂同属 Annex B 隐式支臂块。
    /// - 支臂为其它语句时返回：其中的函数声明不属于 `if` 支臂隐式块。
    /// - 须在 `block_fn_suppressed` 已构建之后调用（函数体/程序声明实例化阶段
    ///   先于任何块 emit）；`is_eval_script` 仅标记 eval 脚本顶层程序，其顶层
    ///   不建块级函数外层 var 绑定；脚本顶层的只读三常量全局属性不可配置，
    ///   声明实例化不建外层 var 绑定，须保留块级绑定承载支臂闭包。
    fn predeclare_if_arm_block_function(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        let mut cur = stmt;
        while let Statement::LabeledStatement(ls) = cur {
            cur = &ls.body;
        }
        match cur {
            Statement::FunctionDeclaration(f) => {
                let Some(identifier) = &f.id else {
                    return;
                };
                if Self::if_arm_has_outer_var_carrier(ctx, identifier.name.as_str()) {
                    return;
                }
                let reg = ctx.alloc_reg();
                let _ = ctx.declare_initialized(identifier.name.as_str(), reg, VariableDeclarationKind::Let, false);
            }
            Statement::IfStatement(is) => {
                self.predeclare_if_arm_block_function(&is.consequent, ctx);
                if let Some(alt) = &is.alternate {
                    self.predeclare_if_arm_block_function(alt, ctx);
                }
            }
            _ => {}
        }
    }

    /// `if` 支臂裸函数声明的名是否已有外层 var/函数绑定承载：非 eval 脚本顶层的
    /// 非抑制名由实例化阶段建外层 var 绑定，唯一例外是脚本顶层的只读三常量
    /// （全局属性不可配置，声明实例化刻意不建外层绑定），其名须退回块级绑定。
    fn if_arm_has_outer_var_carrier(ctx: &CompileCtx, name: &str) -> bool {
        !ctx.is_eval_script
            && !ctx.block_fn_suppressed.contains(name)
            && !(ctx.is_global_scope && CompileCtx::is_non_writable_global_builtin(name))
    }

    /// 预声明当前作用域直接子语句中的 `let`/`const`/`class` 绑定（未初始化，
    /// 建立 TDZ 占位）。使声明点之前的读取在编译期可分辨为 TDZ 而非隐式全局，
    /// 声明点复用预登记槽位。
    ///
    /// # 边界与前提
    /// - 只扫描直接子语句 + 递归 switch case（switch 不推 scope，case 内 lexical
    ///   声明属于外层作用域）；不递归块/if/for/while body（嵌套块自预声明；
    ///   单语句 body 不接受 lexical 声明——lexical 属 Declaration、非 Statement
    ///   子产生式，parser 按语法错误直接拒绝，这些形状不会进入 emit）。
    /// - 跳过 for 头声明（循环作用域由 for 分支内联 declare）。
    /// - `global_lexical` 为 true 时（仅脚本顶层调用点传 `!is_eval_script`）：
    ///   lexical 声明撞受限全局名报 SyntaxError（脚本声明实例化对全局对象受限
    ///   自有属性名做检查，eval 代码声明实例化无此检查）；函数体/块/try/模块
    ///   调用点传 false，lexical 声明是局部绑定不查。
    pub(crate) fn predeclare_lexical_declarations(
        &self, statements: &[Statement], ctx: &mut CompileCtx, global_lexical: bool,
    ) -> Result<(), String> {
        for statement in statements {
            match statement {
                Statement::VariableDeclaration(decl) => {
                    if matches!(decl.kind, VariableDeclarationKind::Var) {
                        continue;
                    }
                    let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                    for d in &decl.declarations {
                        self.predeclare_lexical_pattern(&d.id, is_const, ctx, global_lexical)?;
                    }
                }
                Statement::ClassDeclaration(cd) => {
                    if let Some(id) = &cd.id {
                        check_restricted_global_lexical(id.name.as_str(), global_lexical)?;
                        let reg = ctx.alloc_reg();
                        let _ = ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                    }
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        for s in &case.consequent {
                            self.predeclare_lexical_stmt(s, ctx, global_lexical)?;
                        }
                    }
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(decl) = &exp.declaration {
                        match decl {
                            Declaration::VariableDeclaration(vd) => {
                                if matches!(vd.kind, VariableDeclarationKind::Var) {
                                    continue;
                                }
                                let is_const = matches!(vd.kind, VariableDeclarationKind::Const);
                                for d in &vd.declarations {
                                    self.predeclare_lexical_pattern(&d.id, is_const, ctx, global_lexical)?;
                                }
                            }
                            Declaration::ClassDeclaration(cd) => {
                                if let Some(id) = &cd.id {
                                    check_restricted_global_lexical(id.name.as_str(), global_lexical)?;
                                    let reg = ctx.alloc_reg();
                                    let _ = ctx.declare_predeclared(
                                        id.name.as_str(),
                                        reg,
                                        VariableDeclarationKind::Const,
                                        true,
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
                    ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                        if let Some(id) = &cd.id {
                            check_restricted_global_lexical(id.name.as_str(), global_lexical)?;
                            let reg = ctx.alloc_reg();
                            let _ =
                                ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                        }
                    }
                    ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                        if let Some(id) = &fd.id {
                            let reg = ctx.alloc_reg();
                            let _ =
                                ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }

    /// 预声明单个语句中的 lexical 声明（供 switch case 递归；`let`/`const`/`class` 分支）。
    fn predeclare_lexical_stmt(
        &self, stmt: &Statement, ctx: &mut CompileCtx, global_lexical: bool,
    ) -> Result<(), String> {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                if matches!(decl.kind, VariableDeclarationKind::Var) {
                    return Ok(());
                }
                let is_const = matches!(decl.kind, VariableDeclarationKind::Const);
                for d in &decl.declarations {
                    self.predeclare_lexical_pattern(&d.id, is_const, ctx, global_lexical)?;
                }
            }
            Statement::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
                    check_restricted_global_lexical(id.name.as_str(), global_lexical)?;
                    let reg = ctx.alloc_reg();
                    let _ = ctx.declare_predeclared(id.name.as_str(), reg, VariableDeclarationKind::Const, true);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 递归预声明绑定 pattern 内的全部标识符（含数组/对象/默认值解构）。
    fn predeclare_lexical_pattern(
        &self, pattern: &BindingPattern, is_const: bool, ctx: &mut CompileCtx, global_lexical: bool,
    ) -> Result<(), String> {
        match pattern {
            BindingPattern::BindingIdentifier(bi) => {
                check_restricted_global_lexical(bi.name.as_str(), global_lexical)?;
                let reg = ctx.alloc_reg();
                let _ = ctx.declare_predeclared(
                    bi.name.as_str(),
                    reg,
                    if is_const {
                        VariableDeclarationKind::Const
                    } else {
                        VariableDeclarationKind::Let
                    },
                    is_const,
                );
            }
            BindingPattern::ArrayPattern(ap) => {
                for e in ap.elements.iter().flatten() {
                    self.predeclare_lexical_pattern(e, is_const, ctx, global_lexical)?;
                }
                if let Some(rest) = &ap.rest {
                    self.predeclare_lexical_pattern(&rest.argument, is_const, ctx, global_lexical)?;
                }
            }
            BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    self.predeclare_lexical_pattern(&prop.value, is_const, ctx, global_lexical)?;
                }
                if let Some(rest) = &op.rest {
                    self.predeclare_lexical_pattern(&rest.argument, is_const, ctx, global_lexical)?;
                }
            }
            BindingPattern::AssignmentPattern(ap) => {
                self.predeclare_lexical_pattern(&ap.left, is_const, ctx, global_lexical)?;
            }
        }
        Ok(())
    }
}
