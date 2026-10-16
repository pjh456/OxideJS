//! 闭包捕获分析，遮蔽感知引用收集：`CaptureEngine` 捆绑双引用集（own ×
//! parent_names）× shadow（遮蔽名计次映射）× 双输出累加器（captured_out ×
//! upvalue_out）× depth（嵌套深度），遍历语句/表达式/赋值目标/类体 AST，嵌套
//! 函数/类/for 头/catch 参数逐层遮蔽，按深度分区写两输出。

use std::collections::{HashMap, HashSet};

use oxide_parser::{ClassBody, ClassElement, Expression, Statement};

use super::names::collect_own_binding_names_into;
use super::{
    collect_binding_pattern_names_into, collect_fn_param_names_into, collect_for_left_decl_names_into,
    collect_for_left_decl_names_remove, shadow_dec,
};

/// 双引用集引擎：捆绑 own（深度 ≥ 1 引用集）、parent_names（全深度引用集）、
/// sub_own（upvalue 显式排除集）、shadow（遮蔽名计次映射）、depth（嵌套深度）与双输出
/// 累加器，本族十五个收集函数共享单一 `&mut CaptureEngine` 参数。作用域边界（嵌套
/// 函数/类/for 头/catch 参数）对 shadow 计次映射做计次增减（函数边界清空重建，
/// catch 参数只增不减），逐层遮蔽外层绑定；函数/类
/// 边界对 depth 做增减，区分本函数体层与嵌套层。
pub(crate) struct CaptureEngine<'a> {
    /// 深度 ≥ 1 的引用集：本函数 own 绑定集（父层捕获判定）。
    own: &'a HashSet<String>,
    /// 全深度引用集：父捕获名 ∪ 父 upvalue 名（子层 upvalue 判定）。
    parent_names: &'a HashSet<String>,
    /// upvalue 判定的显式排除集：本函数 own 绑定集。
    sub_own: &'a HashSet<String>,
    /// 遮蔽名计次映射：当前作用域内被遮蔽的名字及其计次；作用域边界计次增减，
    /// 函数边界清空重建，每边界零分配。
    shadow: HashMap<String, u32>,
    /// 嵌套深度：0 = 本函数体层，≥ 1 = 嵌套函数/类内。
    depth: u32,
    /// 输出累加器：被嵌套函数捕获的名字（父层捕获判定）。
    captured_out: &'a mut HashSet<String>,
    /// 输出累加器：子函数引用父被捕获绑定的名字（子层 upvalue 判定）。
    upvalue_out: &'a mut HashSet<String>,
}

impl<'a> CaptureEngine<'a> {
    /// 构造引擎：`own` 为深度 ≥ 1 引用集，`parent_names` 为全深度引用集，
    /// `sub_own` 为 upvalue 显式排除集，`captured_out` 与 `upvalue_out` 为双输出
    /// 累加器。shadow 初始空，depth 初始 0。
    pub(crate) fn new(
        own: &'a HashSet<String>, parent_names: &'a HashSet<String>, sub_own: &'a HashSet<String>,
        captured_out: &'a mut HashSet<String>, upvalue_out: &'a mut HashSet<String>,
    ) -> Self {
        CaptureEngine {
            own,
            parent_names,
            sub_own,
            shadow: HashMap::new(),
            depth: 0,
            captured_out,
            upvalue_out,
        }
    }

    /// 单一标识符检查点：按深度分区写两输出。
    /// 深度 ≥ 1 且名字在 own 且不在 shadow → captured_out；
    /// 名字在 parent_names 且不在 shadow 且不在 sub_own → upvalue_out。
    fn check_identifier(&mut self, name: &str) {
        if self.depth >= 1 && self.own.contains(name) && !self.shadow_contains(name) {
            self.captured_out.insert(name.to_string());
        }
        if self.parent_names.contains(name) && !self.shadow_contains(name) && !self.sub_own.contains(name) {
            self.upvalue_out.insert(name.to_string());
        }
    }

    /// 遮蔽判定：名字在计次映射中的计次大于零即被遮蔽。
    fn shadow_contains(&self, name: &str) -> bool {
        self.shadow.get(name).is_some_and(|&c| c > 0)
    }
}

/// 以引擎的 ref_set × shadow 双集驱动，收集被任意深度嵌套函数引用、且未被遮蔽的
/// 名字写入 out。本族带显式 ref_set/shadow 语义，按过滤条件收集引用名；`captured.rs`
/// 的 `collect_captured_*` 族则以本函数 own 绑定集为过滤条件做捕获最终判定。
pub(crate) fn collect_capture_names_shadowed(stmts: &[Statement], eng: &mut CaptureEngine) {
    for stmt in stmts {
        collect_capture_names_stmt(stmt, eng);
    }
}

/// 按语句类型递归扫描引用；进入嵌套函数/类/for 头/catch 时逐层扩充遮蔽集。
pub(crate) fn collect_capture_names_stmt(stmt: &Statement, eng: &mut CaptureEngine) {
    match stmt {
        Statement::ExpressionStatement(es) => collect_capture_names_expr(&es.expression, eng),
        Statement::ReturnStatement(rs) => {
            if let Some(a) = &rs.argument {
                collect_capture_names_expr(a, eng);
            }
        }
        Statement::VariableDeclaration(vd) => {
            for d in &vd.declarations {
                // 绑定 pattern 的计算键是运行时求值表达式，其引用须纳入捕获。
                collect_capture_names_binding_keys(&d.id, eng);
                if let Some(init) = &d.init {
                    collect_capture_names_expr(init, eng);
                }
            }
        }
        Statement::FunctionDeclaration(fd) => {
            let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
            collect_fn_default_names(&fd.params, eng);
            // 函数形参与体内声明遮蔽外层绑定：影子计次映射保存/恢复，清空重建为
            // 本函数自身名，深度增一。
            let saved = std::mem::take(&mut eng.shadow);
            collect_fn_param_names_into(&fd.params, &mut eng.shadow);
            collect_own_binding_names_into(&[], body, &mut eng.shadow);
            eng.depth += 1;
            collect_capture_names_shadowed(body, eng);
            eng.depth -= 1;
            eng.shadow = saved;
        }
        Statement::ClassDeclaration(cd) => {
            // 类体（构造器/方法体、字段键与值、静态块）是嵌套作用域：
            // 类名遮蔽外层绑定，方法形参与方法体局部进一步遮蔽。深度增一。
            if let Some(id) = &cd.id {
                *eng.shadow.entry(id.name.as_str().to_string()).or_insert(0) += 1;
            }
            eng.depth += 1;
            collect_class_capture_names(&cd.body, eng);
            eng.depth -= 1;
            if let Some(id) = &cd.id {
                shadow_dec(&mut eng.shadow, id.name.as_str().to_string());
            }
        }
        Statement::IfStatement(is) => {
            collect_capture_names_expr(&is.test, eng);
            collect_capture_names_stmt(&is.consequent, eng);
            if let Some(alt) = &is.alternate {
                collect_capture_names_stmt(alt, eng);
            }
        }
        Statement::ForStatement(fs) => {
            if let Some(init) = &fs.init {
                if let Some(e) = init.as_expression() {
                    collect_capture_names_expr(e, eng);
                }
                if let oxide_parser::ForStatementInit::VariableDeclaration(vd) = init {
                    for d in &vd.declarations {
                        collect_capture_names_binding_keys(&d.id, eng);
                        if let Some(i) = &d.init {
                            collect_capture_names_expr(i, eng);
                        }
                    }
                }
            }
            if let Some(t) = &fs.test {
                collect_capture_names_expr(t, eng);
            }
            if let Some(u) = &fs.update {
                collect_capture_names_expr(u, eng);
            }
            collect_capture_names_stmt(&fs.body, eng);
        }
        Statement::WhileStatement(w) => {
            collect_capture_names_expr(&w.test, eng);
            collect_capture_names_stmt(&w.body, eng);
        }
        Statement::DoWhileStatement(d) => {
            collect_capture_names_stmt(&d.body, eng);
            collect_capture_names_expr(&d.test, eng);
        }
        Statement::ForInStatement(fi) => {
            collect_capture_names_expr(&fi.right, eng);
            // 头的绑定键在头声明名进入影子集之前求值，以当前影子集扫描。
            if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fi.left {
                for d in &vd.declarations {
                    collect_capture_names_binding_keys(&d.id, eng);
                }
            } else {
                // 赋值头是既有绑定引用：头名与默认值表达式须纳入捕获判定。
                collect_capture_names_for_left(&fi.left, eng);
            }
            // left 的 var 声明遮蔽外层同名绑定，body 内引用不视为捕获外层。
            collect_for_left_decl_names_into(&fi.left, &mut eng.shadow);
            collect_capture_names_shadowed(std::slice::from_ref(&fi.body), eng);
            collect_for_left_decl_names_remove(&fi.left, &mut eng.shadow);
        }
        Statement::ForOfStatement(fo) => {
            collect_capture_names_expr(&fo.right, eng);
            if let oxide_parser::ForStatementLeft::VariableDeclaration(vd) = &fo.left {
                for d in &vd.declarations {
                    collect_capture_names_binding_keys(&d.id, eng);
                }
            } else {
                collect_capture_names_for_left(&fo.left, eng);
            }
            collect_for_left_decl_names_into(&fo.left, &mut eng.shadow);
            collect_capture_names_shadowed(std::slice::from_ref(&fo.body), eng);
            collect_for_left_decl_names_remove(&fo.left, &mut eng.shadow);
        }
        Statement::BlockStatement(b) => collect_capture_names_shadowed(&b.body, eng),
        Statement::TryStatement(ts) => {
            for s in &ts.block.body {
                collect_capture_names_stmt(s, eng);
            }
            if let Some(h) = &ts.handler {
                // catch 参数解构模式内的表达式引用（默认值等）也会被内部闭包捕获；
                // 先以外层影子集扫模式内表达式，再将参数声明名合并进引擎影子计次
                // 映射遮蔽 catch 作用域（只增不减，与旧代码一致）。
                if let Some(param) = &h.param {
                    collect_binding_pattern_exprs(&param.pattern, eng);
                    collect_binding_pattern_names_into(&param.pattern, &mut eng.shadow);
                }
                for s in &h.body.body {
                    collect_capture_names_stmt(s, eng);
                }
            }
            if let Some(f) = &ts.finalizer {
                for s in &f.body {
                    collect_capture_names_stmt(s, eng);
                }
            }
        }
        Statement::ExportNamedDeclaration(exp) => {
            if let Some(decl) = &exp.declaration {
                match decl {
                    oxide_parser::Declaration::VariableDeclaration(vd) => {
                        for d in &vd.declarations {
                            collect_capture_names_binding_keys(&d.id, eng);
                            if let Some(init) = &d.init {
                                collect_capture_names_expr(init, eng);
                            }
                        }
                    }
                    oxide_parser::Declaration::FunctionDeclaration(fd) => {
                        let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                        collect_fn_default_names(&fd.params, eng);
                        let saved = std::mem::take(&mut eng.shadow);
                        collect_fn_param_names_into(&fd.params, &mut eng.shadow);
                        collect_own_binding_names_into(&[], body, &mut eng.shadow);
                        eng.depth += 1;
                        collect_capture_names_shadowed(body, eng);
                        eng.depth -= 1;
                        eng.shadow = saved;
                    }
                    oxide_parser::Declaration::ClassDeclaration(cd) => {
                        if let Some(id) = &cd.id {
                            *eng.shadow.entry(id.name.as_str().to_string()).or_insert(0) += 1;
                        }
                        eng.depth += 1;
                        collect_class_capture_names(&cd.body, eng);
                        eng.depth -= 1;
                        if let Some(id) = &cd.id {
                            shadow_dec(&mut eng.shadow, id.name.as_str().to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        Statement::ExportDefaultDeclaration(exp) => match &exp.declaration {
            oxide_parser::ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                collect_fn_default_names(&fd.params, eng);
                let saved = std::mem::take(&mut eng.shadow);
                collect_fn_param_names_into(&fd.params, &mut eng.shadow);
                collect_own_binding_names_into(&[], body, &mut eng.shadow);
                eng.depth += 1;
                collect_capture_names_shadowed(body, eng);
                eng.depth -= 1;
                eng.shadow = saved;
            }
            oxide_parser::ExportDefaultDeclarationKind::ClassDeclaration(cd) => {
                if let Some(id) = &cd.id {
                    *eng.shadow.entry(id.name.as_str().to_string()).or_insert(0) += 1;
                }
                eng.depth += 1;
                collect_class_capture_names(&cd.body, eng);
                eng.depth -= 1;
                if let Some(id) = &cd.id {
                    shadow_dec(&mut eng.shadow, id.name.as_str().to_string());
                }
            }
            other => {
                if let Some(e) = other.as_expression() {
                    collect_capture_names_expr(e, eng);
                }
            }
        },
        Statement::ThrowStatement(ts) => collect_capture_names_expr(&ts.argument, eng),
        Statement::SwitchStatement(sw) => {
            collect_capture_names_expr(&sw.discriminant, eng);
            for case in &sw.cases {
                for s in &case.consequent {
                    collect_capture_names_stmt(s, eng);
                }
            }
        }
        Statement::LabeledStatement(ls) => collect_capture_names_stmt(&ls.body, eng),
        Statement::WithStatement(ws) => {
            collect_capture_names_expr(&ws.object, eng);
            collect_capture_names_stmt(&ws.body, eng);
        }
        // 无扫描需求变体：跳转/调试/空语句、TS 声明、import 与再导出。
        Statement::BreakStatement(_) => {}
        Statement::ContinueStatement(_) => {}
        Statement::DebuggerStatement(_) => {}
        Statement::EmptyStatement(_) => {}
        Statement::TSTypeAliasDeclaration(_) => {}
        Statement::TSInterfaceDeclaration(_) => {}
        Statement::TSEnumDeclaration(_) => {}
        Statement::TSModuleDeclaration(_) => {}
        Statement::TSGlobalDeclaration(_) => {}
        Statement::TSImportEqualsDeclaration(_) => {}
        Statement::ImportDeclaration(_) => {}
        Statement::ExportAllDeclaration(_) => {}
        Statement::TSExportAssignment(_) => {}
        Statement::TSNamespaceExportDeclaration(_) => {}
    }
}

/// 收集类体对 ref_set 的引用：方法/构造器体、字段键与值、静态块。
/// 方法形参与方法体局部声明遮蔽外层绑定（捕获判定用）。
pub(crate) fn collect_class_capture_names(class_body: &ClassBody, eng: &mut CaptureEngine) {
    for element in &class_body.body {
        match element {
            ClassElement::MethodDefinition(method) => {
                collect_fn_default_names(&method.value.params, eng);
                // 方法形参与方法体局部遮蔽外层绑定：影子计次映射保存/恢复，
                // 清空重建为本方法自身名。
                let saved = std::mem::take(&mut eng.shadow);
                collect_fn_param_names_into(&method.value.params, &mut eng.shadow);
                let body: &[Statement] = method.value.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                collect_own_binding_names_into(&[], body, &mut eng.shadow);
                collect_capture_names_shadowed(body, eng);
                eng.shadow = saved;
            }
            ClassElement::PropertyDefinition(prop) => {
                if let Some(expr) = prop.key.as_expression() {
                    collect_capture_names_expr(expr, eng);
                }
                if let Some(value) = &prop.value {
                    collect_capture_names_expr(value, eng);
                }
            }
            ClassElement::StaticBlock(block) => {
                collect_capture_names_shadowed(&block.body, eng);
            }
            // 无扫描需求变体：访问器属性与 TS 索引签名。
            ClassElement::AccessorProperty(_) => {}
            ClassElement::TSIndexSignature(_) => {}
        }
    }
}

/// 遍历绑定 pattern 的运行时求值表达式引用：计算键（`{[k]: a}` 的键）与内嵌
/// 默认值（`{a = x}` 的 x）都是模式内求值的表达式，嵌套函数引用外层绑定时须
/// 纳入捕获（与赋值侧/对象字面量/类字段键遍历同口径）。模式声明名不进影子计次
/// 映射：catch 与形参默认值边界都先以外层影子集扫完表达式、再写声明名，声明名
/// 不参与模式内表达式扫描。
pub(crate) fn collect_binding_pattern_exprs(pattern: &oxide_parser::BindingPattern, eng: &mut CaptureEngine) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(_) => {}
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_binding_pattern_exprs(p, eng);
            }
            if let Some(rest) = &ap.rest {
                collect_binding_pattern_exprs(&rest.argument, eng);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                // 计算键是运行时求值表达式，其引用须纳入捕获。
                if prop.computed {
                    collect_capture_names_expr(prop.key.to_expression(), eng);
                }
                collect_binding_pattern_exprs(&prop.value, eng);
            }
            if let Some(rest) = &op.rest {
                collect_binding_pattern_exprs(&rest.argument, eng);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            collect_capture_names_expr(&ap.right, eng);
            collect_binding_pattern_exprs(&ap.left, eng);
        }
    }
}

/// 遍历绑定 pattern 的运行时求值表达式引用：计算键（`{[k]: a}` 的键）与内嵌
/// 默认值（`{a = x}` 的 x）都是模式内求值的表达式，嵌套函数引用外层绑定时须
/// 纳入捕获（与赋值侧/对象字面量/类字段键遍历同口径）。
pub(crate) fn collect_capture_names_binding_keys(pattern: &oxide_parser::BindingPattern, eng: &mut CaptureEngine) {
    match pattern {
        oxide_parser::BindingPattern::BindingIdentifier(_) => {}
        oxide_parser::BindingPattern::ArrayPattern(ap) => {
            for p in ap.elements.iter().flatten() {
                collect_capture_names_binding_keys(p, eng);
            }
            if let Some(rest) = &ap.rest {
                collect_capture_names_binding_keys(&rest.argument, eng);
            }
        }
        oxide_parser::BindingPattern::ObjectPattern(op) => {
            for prop in &op.properties {
                if prop.computed {
                    collect_capture_names_expr(prop.key.to_expression(), eng);
                }
                collect_capture_names_binding_keys(&prop.value, eng);
            }
            if let Some(rest) = &op.rest {
                collect_capture_names_binding_keys(&rest.argument, eng);
            }
        }
        oxide_parser::BindingPattern::AssignmentPattern(ap) => {
            // 内嵌默认值表达式引用外层绑定须捕获（与 binding_pattern/赋值侧
            // maybe_default 的 init 扫描同口径）；left 继续递归模式键。
            collect_capture_names_expr(&ap.right, eng);
            collect_capture_names_binding_keys(&ap.left, eng);
        }
    }
}

/// 扫描赋值目标中的引用（标识符写名，成员目标扫对象/键，解构目标递归元素）。
pub(crate) fn collect_capture_names_assign_target(target: &oxide_parser::AssignmentTarget, eng: &mut CaptureEngine) {
    match target {
        oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(ati) => {
            eng.check_identifier(ati.name.as_str());
        }
        oxide_parser::AssignmentTarget::StaticMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
        }
        oxide_parser::AssignmentTarget::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
            collect_capture_names_expr(&m.expression, eng);
        }
        oxide_parser::AssignmentTarget::ArrayAssignmentTarget(a) => {
            collect_capture_names_array_target(a, eng);
        }
        oxide_parser::AssignmentTarget::ObjectAssignmentTarget(o) => {
            collect_capture_names_object_target(o, eng);
        }
        oxide_parser::AssignmentTarget::PrivateFieldExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
        }
        // 无扫描需求变体：TS 类型断言目标。
        oxide_parser::AssignmentTarget::TSAsExpression(_) => {}
        oxide_parser::AssignmentTarget::TSSatisfiesExpression(_) => {}
        oxide_parser::AssignmentTarget::TSNonNullExpression(_) => {}
        oxide_parser::AssignmentTarget::TSTypeAssertion(_) => {}
    }
}

/// 数组赋值目标：元素递归（含默认值），rest 目标递归。
fn collect_capture_names_array_target(a: &oxide_parser::ArrayAssignmentTarget, eng: &mut CaptureEngine) {
    for e in a.elements.iter().flatten() {
        collect_capture_names_maybe_default_target(e, eng);
    }
    if let Some(rest) = &a.rest {
        collect_capture_names_assign_target(&rest.target, eng);
    }
}

/// 对象赋值目标：属性标识符写名/默认值/计算键，rest 目标递归。
fn collect_capture_names_object_target(o: &oxide_parser::ObjectAssignmentTarget, eng: &mut CaptureEngine) {
    for prop in &o.properties {
        if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) = prop {
            eng.check_identifier(id.binding.name.as_str());
            if let Some(init) = &id.init {
                collect_capture_names_expr(init, eng);
            }
        } else if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) = prop {
            if let Some(name_expr) = p.name.as_expression() {
                collect_capture_names_expr(name_expr, eng);
            }
            collect_capture_names_maybe_default_target(&p.binding, eng);
        }
    }
    if let Some(rest) = &o.rest {
        collect_capture_names_assign_target(&rest.target, eng);
    }
}

/// 扫描 for-in/for-of 赋值头：标识符写名、数组/对象解构元素（含默认值）与 rest。
/// 声明头是新绑定无引用语义，不扫。
pub(crate) fn collect_capture_names_for_left(left: &oxide_parser::ForStatementLeft, eng: &mut CaptureEngine) {
    match left {
        oxide_parser::ForStatementLeft::AssignmentTargetIdentifier(id) => {
            eng.check_identifier(id.name.as_str());
        }
        oxide_parser::ForStatementLeft::ArrayAssignmentTarget(a) => {
            collect_capture_names_array_target(a, eng);
        }
        oxide_parser::ForStatementLeft::ObjectAssignmentTarget(o) => {
            collect_capture_names_object_target(o, eng);
        }
        // 无扫描需求变体：声明头是新绑定无引用语义，成员与 TS 类型断言目标不扫。
        oxide_parser::ForStatementLeft::VariableDeclaration(_) => {}
        oxide_parser::ForStatementLeft::StaticMemberExpression(_) => {}
        oxide_parser::ForStatementLeft::ComputedMemberExpression(_) => {}
        oxide_parser::ForStatementLeft::PrivateFieldExpression(_) => {}
        oxide_parser::ForStatementLeft::TSAsExpression(_) => {}
        oxide_parser::ForStatementLeft::TSSatisfiesExpression(_) => {}
        oxide_parser::ForStatementLeft::TSNonNullExpression(_) => {}
        oxide_parser::ForStatementLeft::TSTypeAssertion(_) => {}
    }
}

/// 解构赋值元素可能是 `AssignmentTarget` 或带默认值的包装（后者多一层 `init`）。
pub(crate) fn collect_capture_names_maybe_default_target(
    target: &oxide_parser::AssignmentTargetMaybeDefault, eng: &mut CaptureEngine,
) {
    use oxide_parser::AssignmentTargetMaybeDefault as MaybeDefault;
    match target {
        MaybeDefault::AssignmentTargetWithDefault(d) => {
            collect_capture_names_expr(&d.init, eng);
            collect_capture_names_assign_target(&d.binding, eng);
        }
        MaybeDefault::AssignmentTargetIdentifier(ati) => {
            eng.check_identifier(ati.name.as_str());
        }
        MaybeDefault::StaticMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
        }
        MaybeDefault::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
            collect_capture_names_expr(&m.expression, eng);
        }
        MaybeDefault::ArrayAssignmentTarget(a) => {
            for e in a.elements.iter().flatten() {
                collect_capture_names_maybe_default_target(e, eng);
            }
        }
        MaybeDefault::ObjectAssignmentTarget(o) => {
            for prop in &o.properties {
                if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) = prop {
                    eng.check_identifier(id.binding.name.as_str());
                    if let Some(init) = &id.init {
                        collect_capture_names_expr(init, eng);
                    }
                } else if let oxide_parser::AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) = prop {
                    if let Some(name_expr) = p.name.as_expression() {
                        collect_capture_names_expr(name_expr, eng);
                    }
                    collect_capture_names_maybe_default_target(&p.binding, eng);
                }
            }
        }
        // 无扫描需求变体：私有字段目标与 TS 类型断言目标。
        MaybeDefault::PrivateFieldExpression(_) => {}
        MaybeDefault::TSAsExpression(_) => {}
        MaybeDefault::TSSatisfiesExpression(_) => {}
        MaybeDefault::TSNonNullExpression(_) => {}
        MaybeDefault::TSTypeAssertion(_) => {}
    }
}

/// 扫描一元更新目标（`++x` / `obj.x++`）中的引用。
pub(crate) fn collect_capture_names_simple_target(
    target: &oxide_parser::SimpleAssignmentTarget, eng: &mut CaptureEngine,
) {
    match target {
        oxide_parser::SimpleAssignmentTarget::AssignmentTargetIdentifier(ati) => {
            eng.check_identifier(ati.name.as_str());
        }
        oxide_parser::SimpleAssignmentTarget::StaticMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
        }
        oxide_parser::SimpleAssignmentTarget::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
            collect_capture_names_expr(&m.expression, eng);
        }
        // 无扫描需求变体：私有字段目标（更新发射面未支持，程序不可编译）与 TS 类型断言目标。
        oxide_parser::SimpleAssignmentTarget::PrivateFieldExpression(_) => {}
        oxide_parser::SimpleAssignmentTarget::TSAsExpression(_) => {}
        oxide_parser::SimpleAssignmentTarget::TSSatisfiesExpression(_) => {}
        oxide_parser::SimpleAssignmentTarget::TSNonNullExpression(_) => {}
        oxide_parser::SimpleAssignmentTarget::TSTypeAssertion(_) => {}
    }
}

/// 收集函数参数默认值表达式里的引用（子层 upvalue 判定；参数名遮蔽）。
/// 默认值在定义处外层作用域求值，其中对外层 own 绑定的引用须纳入捕获判定：
/// 扫描前临时深度加一过门槛，扫完即减回；upvalue 判定无深度门槛，不受影响。
pub(crate) fn collect_fn_default_names(params: &oxide_parser::FormalParameters, eng: &mut CaptureEngine) {
    eng.depth += 1;
    for p in &params.items {
        if let Some(init) = &p.initializer {
            collect_capture_names_expr(init, eng);
        }
        // 形参模式内表达式（计算键、嵌套默认值）在外层作用域求值，须纳入捕获
        // 判定；模式声明名不进影子计次映射（旧代码扩展后立即恢复，净效果为空，
        // 保持逐位一致）。
        collect_binding_pattern_exprs(&p.pattern, eng);
    }
    eng.depth -= 1;
}

/// 按表达式类型递归扫描引用；进入嵌套函数体时以形参 + 体内声明扩充遮蔽集。与
/// `captured.rs` 的 `collect_captured_expr` 相比，本函数经引擎的 ref_set/shadow
/// 收集引用名，不做捕获最终判定。
pub(crate) fn collect_capture_names_expr(expr: &Expression, eng: &mut CaptureEngine) {
    match expr {
        Expression::Identifier(id) => {
            eng.check_identifier(id.name.as_str());
        }
        Expression::AssignmentExpression(ae) => {
            // 赋值目标里的引用也须捕获：标识符目标写名，成员/解构目标扫描其对象/元素。
            collect_capture_names_assign_target(&ae.left, eng);
            collect_capture_names_expr(&ae.right, eng);
        }
        Expression::UpdateExpression(ue) => {
            collect_capture_names_simple_target(&ue.argument, eng);
        }
        Expression::FunctionExpression(fe) => {
            let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
            collect_fn_default_names(&fe.params, eng);
            // 函数形参与体内声明遮蔽外层绑定：影子计次映射保存/恢复，清空重建为
            // 本函数自身名，深度增一。
            let saved = std::mem::take(&mut eng.shadow);
            collect_fn_param_names_into(&fe.params, &mut eng.shadow);
            collect_own_binding_names_into(&[], body, &mut eng.shadow);
            eng.depth += 1;
            collect_capture_names_shadowed(body, eng);
            eng.depth -= 1;
            eng.shadow = saved;
        }
        Expression::ArrowFunctionExpression(ae) => {
            collect_fn_default_names(&ae.params, eng);
            // 函数形参与体内声明遮蔽外层绑定：影子计次映射保存/恢复，清空重建为
            // 本函数自身名，深度增一。
            let saved = std::mem::take(&mut eng.shadow);
            collect_fn_param_names_into(&ae.params, &mut eng.shadow);
            collect_own_binding_names_into(&[], &ae.body.statements, &mut eng.shadow);
            eng.depth += 1;
            collect_capture_names_shadowed(&ae.body.statements, eng);
            eng.depth -= 1;
            eng.shadow = saved;
        }
        Expression::ClassExpression(class) => {
            // 类表达式：方法/构造器体与字段表达式是嵌套作用域，引用须捕获。深度增一。
            if let Some(id) = &class.id {
                *eng.shadow.entry(id.name.as_str().to_string()).or_insert(0) += 1;
            }
            eng.depth += 1;
            collect_class_capture_names(&class.body, eng);
            eng.depth -= 1;
            if let Some(id) = &class.id {
                shadow_dec(&mut eng.shadow, id.name.as_str().to_string());
            }
        }
        Expression::BinaryExpression(be) => {
            collect_capture_names_expr(&be.left, eng);
            collect_capture_names_expr(&be.right, eng);
        }
        Expression::UnaryExpression(ue) => collect_capture_names_expr(&ue.argument, eng),
        Expression::CallExpression(ce) => {
            collect_capture_names_expr(&ce.callee, eng);
            for a in &ce.arguments {
                collect_capture_names_arg(a, eng);
            }
        }
        Expression::NewExpression(ne) => {
            collect_capture_names_expr(&ne.callee, eng);
            for a in &ne.arguments {
                collect_capture_names_arg(a, eng);
            }
        }
        Expression::SequenceExpression(se) => {
            for e in &se.expressions {
                collect_capture_names_expr(e, eng);
            }
        }
        Expression::ConditionalExpression(ce) => {
            collect_capture_names_expr(&ce.test, eng);
            collect_capture_names_expr(&ce.consequent, eng);
            collect_capture_names_expr(&ce.alternate, eng);
        }
        Expression::ArrayExpression(ae) => {
            for e in &ae.elements {
                if let Some(e) = e.as_expression() {
                    collect_capture_names_expr(e, eng);
                }
            }
        }
        Expression::LogicalExpression(le) => {
            collect_capture_names_expr(&le.left, eng);
            collect_capture_names_expr(&le.right, eng);
        }
        Expression::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
            collect_capture_names_expr(&m.expression, eng);
        }
        Expression::StaticMemberExpression(m) => collect_capture_names_expr(&m.object, eng),
        Expression::PrivateFieldExpression(m) => collect_capture_names_expr(&m.object, eng),
        Expression::ParenthesizedExpression(p) => collect_capture_names_expr(&p.expression, eng),
        Expression::TemplateLiteral(tl) => {
            for e in &tl.expressions {
                collect_capture_names_expr(e, eng);
            }
        }
        Expression::TaggedTemplateExpression(tt) => {
            collect_capture_names_expr(&tt.tag, eng);
            for e in &tt.quasi.expressions {
                collect_capture_names_expr(e, eng);
            }
        }
        Expression::ObjectExpression(o) => {
            for prop in &o.properties {
                match prop {
                    oxide_parser::ObjectPropertyKind::ObjectProperty(p) => {
                        if p.computed {
                            collect_capture_names_expr(p.key.to_expression(), eng);
                        }
                        collect_capture_names_expr(&p.value, eng);
                    }
                    oxide_parser::ObjectPropertyKind::SpreadProperty(spread) => {
                        collect_capture_names_expr(&spread.argument, eng);
                    }
                }
            }
        }
        // 生成器让出表达式：被让出的值里引用的父变量须纳入捕获，否则生成器体经
        // LOAD_VAR 读调用方寄存器残留（挂起恢复后寄存器已被覆盖）。
        Expression::YieldExpression(ye) => {
            if let Some(a) = &ye.argument {
                collect_capture_names_expr(a, eng);
            }
        }
        // await 表达式：被等待的值里引用的父变量须纳入捕获（与 yield 同因——
        // 异步体挂起恢复后寄存器已被覆盖，只能经 cell 读取）。
        Expression::AwaitExpression(ae) => {
            collect_capture_names_expr(&ae.argument, eng);
        }
        Expression::ChainExpression(c) => collect_capture_names_chain(&c.expression, eng),
        // 无扫描需求变体：字面量、this/super/meta、import 表达式、
        // 私有 in 表达式、JSX 与 TS 类型断言族。
        Expression::BooleanLiteral(_) => {}
        Expression::NullLiteral(_) => {}
        Expression::NumericLiteral(_) => {}
        Expression::BigIntLiteral(_) => {}
        Expression::RegExpLiteral(_) => {}
        Expression::StringLiteral(_) => {}
        Expression::MetaProperty(_) => {}
        Expression::Super(_) => {}
        Expression::ImportExpression(_) => {}
        Expression::ThisExpression(_) => {}
        Expression::PrivateInExpression(_) => {}
        Expression::JSXElement(_) => {}
        Expression::JSXFragment(_) => {}
        Expression::TSAsExpression(_) => {}
        Expression::TSSatisfiesExpression(_) => {}
        Expression::TSTypeAssertion(_) => {}
        Expression::TSNonNullExpression(_) => {}
        Expression::TSInstantiationExpression(_) => {}
        Expression::V8IntrinsicExpression(_) => {}
    }
}

/// 遍历可选链元素，扫描链上成员访问、调用与实参中的引用。
pub(crate) fn collect_capture_names_chain(element: &oxide_parser::ChainElement, eng: &mut CaptureEngine) {
    match element {
        oxide_parser::ChainElement::StaticMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
        }
        oxide_parser::ChainElement::ComputedMemberExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
            collect_capture_names_expr(&m.expression, eng);
        }
        oxide_parser::ChainElement::PrivateFieldExpression(m) => {
            collect_capture_names_expr(&m.object, eng);
        }
        oxide_parser::ChainElement::CallExpression(call) => {
            collect_capture_names_expr(&call.callee, eng);
            for a in &call.arguments {
                collect_capture_names_arg(a, eng);
            }
        }
        // 无扫描需求变体：TS 非空断言链元素。
        oxide_parser::ChainElement::TSNonNullExpression(_) => {}
    }
}

/// 遍历调用实参：静态实参与 spread 内部表达式都纳入捕获扫描。
pub(crate) fn collect_capture_names_arg(arg: &oxide_parser::Argument, eng: &mut CaptureEngine) {
    if let Some(e) = arg.as_expression() {
        collect_capture_names_expr(e, eng);
    } else if let oxide_parser::Argument::SpreadElement(sp) = arg {
        collect_capture_names_expr(&sp.argument, eng);
    }
}
