//! 模块编译：import/export 解析、依赖加载（递归）、快照式链接与命名空间。
//!
//! 快照式链接：依赖模块先整体求值（`__moduleEval`）并返回命名空间对象，
//! 导入绑定在 prelude 一次性初始化；`export` 语句就地注册导出值。
//! 未支持：live binding（导入绑定活态跟随源模块值变化）、defer（延迟求值）、
//! source-phase——导入绑定为链接期快照，不跟随源模块值变化；循环导入在编译期跳过。

use std::collections::HashSet;
use std::path::Path;

use crate::capture::{collect_captured_bindings, collect_own_binding_names};
use crate::expr::call::pack_arg_regs;
use crate::{CompileCtx, Emitter};
use oxc_allocator::Box;
use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::{IRFunction, ParamLayout};
use oxide_parser::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, ImportAttributeKey,
    ImportDeclarationSpecifier, ModuleExportName, Statement, VariableDeclarationKind, WithClause,
};

/// 数据模块种类（非 JS 源码模块）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Js,
    Json,
    Text,
    Bytes,
}

/// 已解析的依赖模块。
pub struct ResolvedModule {
    /// 模块源码（JS / 数据内容）。
    pub source: String,
    /// 模块规范路径（用于自身导入识别与循环检测）。
    pub path: String,
    /// 模块种类。
    pub kind: ModuleKind,
}

/// 模块加载器：由调用方（test262 runner / CLI）提供文件解析。
pub trait ModuleSourceLoader {
    /// 解析依赖模块：按 `base_dir` 与 `specifier` 定位模块源码，`attributes` 为
    /// with 子句的导入属性键值对。
    fn resolve(
        &mut self, base_dir: &str, specifier: &str, attributes: &[(&str, &str)],
    ) -> Result<ResolvedModule, String>;
}

/// `export default` 的默认导出名，也是匿名默认导出函数/类的隐式 `name`。
const DEFAULT_EXPORT_NAME: &str = "default";

/// 剥离可能的多层括号表达式，供隐式名分派判定内层函数/类形态。
fn strip_parens<'a, 'b>(expr: &'b Expression<'a>) -> &'b Expression<'a> {
    let mut cur = expr;
    while let Expression::ParenthesizedExpression(p) = cur {
        cur = &p.expression;
    }
    cur
}

/// 把隐式名写到刚发射的匿名函数子模块。
///
/// # 副作用
/// - 修改 `ctx.nested` 末项的 `function_name`；已有名（具名函数）不覆盖。
///
/// # 注意事项
/// - 仅用于函数/箭头/生成器：它们在发射期只 push 一个子模块，末项即目标。
///   类在构造器之后还会 push 方法子模块，末项不是构造器，类名必须经
///   `emit_class_with_binding` 的 `implicit_name` 参数落名。
fn set_implicit_name_of_last_nested(ctx: &mut CompileCtx, name: &str) {
    if let Some(m) = ctx.nested.last_mut() {
        if m.function_name.is_none() {
            m.function_name = Some(name.to_string());
        }
    }
}

/// 把 `ModuleExportName`（标识符名 / 标识符引用 / 字符串字面量）统一转为字符串导出名。
fn module_export_name_str(m: &ModuleExportName) -> String {
    match m {
        ModuleExportName::IdentifierName(i) => i.name.to_string(),
        ModuleExportName::IdentifierReference(i) => i.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.to_string(),
    }
}

/// 从 with 子句提取导入属性键值对（键取标识符或字符串字面量，值取字符串字面量）。
fn module_attributes(with_clause: &Option<Box<'_, WithClause<'_>>>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(wc) = with_clause {
        for attr in &wc.with_entries {
            let key = match &attr.key {
                ImportAttributeKey::Identifier(id) => id.name.to_string(),
                ImportAttributeKey::StringLiteral(s) => s.value.to_string(),
            };
            out.push((key, attr.value.value.to_string()));
        }
    }
    out
}

/// 收集模块自身直接导出的名称集合（含字符串导出名与 default）：
/// 供 self-import 链接期校验（自引用导入不存在的导出 → 编译期错误）。
fn module_own_export_names(body: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in body {
        match stmt {
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(decl) = &exp.declaration {
                    match decl {
                        Declaration::VariableDeclaration(vd) => {
                            for d in &vd.declarations {
                                if let BindingPattern::BindingIdentifier(bi) = &d.id {
                                    names.insert(bi.name.to_string());
                                }
                            }
                        }
                        Declaration::FunctionDeclaration(fd) => {
                            if let Some(id) = &fd.id {
                                names.insert(id.name.to_string());
                            }
                        }
                        Declaration::ClassDeclaration(cl) => {
                            if let Some(id) = &cl.id {
                                names.insert(id.name.to_string());
                            }
                        }
                        _ => {}
                    }
                } else {
                    for spec in &exp.specifiers {
                        names.insert(module_export_name_str(&spec.exported));
                    }
                }
            }
            Statement::ExportDefaultDeclaration(_) => {
                names.insert(DEFAULT_EXPORT_NAME.to_string());
            }
            Statement::ExportAllDeclaration(exp) => {
                if let Some(exported) = &exp.exported {
                    names.insert(module_export_name_str(exported));
                }
            }
            _ => {}
        }
    }
    names
}

/// 判断语句是否为提升的函数声明（含 export 包装的函数声明）。
fn is_hoisted_function_decl(stmt: &Statement) -> bool {
    match stmt {
        Statement::FunctionDeclaration(_) => true,
        Statement::ExportNamedDeclaration(exp) => {
            matches!(&exp.declaration, Some(Declaration::FunctionDeclaration(_)))
        }
        _ => false,
    }
}

impl Emitter {
    /// 编译 ES module：顶层 body + 递归依赖。
    /// `module_path` 为模块文件路径；入口即归一为规范绝对路径（依赖解析基准 =
    /// 其父目录），与加载器 resolve 的绝对规范口径对齐，使自导入身份比较稳定。
    pub fn emit_program_module(
        &self, program: &oxide_parser::Program, module_path: &str, loader: &mut dyn ModuleSourceLoader,
    ) -> Result<IRFunction, String> {
        crate::emit_debug!("emit_program_module: {} stmts", program.body.len());
        // 顶层模块路径归一为规范绝对路径：自导入身份比较（resolved.path ==
        // module_path）与 base_dir 需与加载器 resolve 的 canonicalize 绝对口径
        // 一致——若保留相对发现路径，自导入比较恒 false、自导入被当外部依赖
        // 重编译。文件不存在（虚拟/内存模块）时 canonicalize 失败，保留原路径。
        let canonical = std::fs::canonicalize(module_path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| module_path.to_string());
        let module_path = canonical.as_str();
        let mut ctx = CompileCtx::new();
        // 模块顶层 var/function 不写全局对象（模块作用域绑定）。
        ctx.is_global_scope = false;
        // ES module 顶层恒严格模式（模块代码是严格模式代码，嵌套函数经父 ctx 继承）。
        ctx.is_strict = true;
        let mut path_stack = vec![module_path.to_string()];
        self.emit_module_into_ctx(program, module_path, loader, &mut path_stack, &mut ctx, true)?;
        Ok(ctx.assemble_ir(ParamLayout { base: 0, count: 0 }, None))
    }

    /// 模块体 emit（顶层与依赖模块共用入口）。
    ///
    /// `top_level` 区分收尾返回口径：顶层模块求值完成值按规范为空记录
    /// （对外 undefined），依赖模块经 `__moduleEval` 以返回值作命名空间对象。
    #[allow(clippy::too_many_arguments)]
    fn emit_module_into_ctx(
        &self, program: &oxide_parser::Program, module_path: &str, loader: &mut dyn ModuleSourceLoader,
        path_stack: &mut Vec<String>, ctx: &mut CompileCtx, top_level: bool,
    ) -> Result<(), String> {
        let body = &program.body;

        // —— 预声明（镜像 emit_program；import/export 包装的声明也计入 hoisting）——
        self.predeclare_function_declarations(body, ctx);
        self.pre_register_builtin_references(body, ctx);
        self.predeclare_var_declarations(body, ctx);
        // 模块 lexical 声明入模块环境（非全局对象），不做受限全局名检查。
        let _ = self.predeclare_lexical_declarations(body, ctx, false);

        // 闭包捕获分析（import 绑定名纳入 own_bindings，供嵌套函数 cell 捕获）。
        ctx.own_bindings = collect_own_binding_names(&[], body);
        ctx.captured_bindings = collect_captured_bindings(body, &[], &ctx.own_bindings);

        // —— 模块命名空间对象 ——
        let ns_reg = self.emit_module_call(ctx, "__moduleObject", &[])?;
        ctx.module_ns_reg = Some(ns_reg);

        // —— 依赖收集（import + re-export/star 的 source，按首次出现去重）——
        let base_dir = Path::new(module_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut deps: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for stmt in body {
            let (spec, attrs) = match stmt {
                Statement::ImportDeclaration(imp) => {
                    (imp.source.value.to_string(), module_attributes(&imp.with_clause))
                }
                Statement::ExportNamedDeclaration(exp) => {
                    if let Some(src) = &exp.source {
                        (src.value.to_string(), module_attributes(&exp.with_clause))
                    } else {
                        continue;
                    }
                }
                Statement::ExportAllDeclaration(exp) => {
                    (exp.source.value.to_string(), module_attributes(&exp.with_clause))
                }
                _ => continue,
            };
            if !deps.iter().any(|(s, _)| *s == spec) {
                deps.push((spec, attrs));
            }
        }

        // —— 编译并求值依赖 ——
        for (spec, attrs) in deps {
            let attr_refs: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let resolved = loader.resolve(&base_dir, &spec, &attr_refs)?;
            let dep_ns_reg = if resolved.path == module_path {
                // 自导入：读自身命名空间（依赖即本模块，勿递归）。
                ns_reg
            } else {
                if path_stack.contains(&resolved.path) {
                    return Err(format!("circular module import not supported: {}", resolved.path));
                }
                path_stack.push(resolved.path.clone());
                let dep_ir = match resolved.kind {
                    ModuleKind::Js => self.compile_js_dep(&resolved, loader, path_stack)?,
                    ModuleKind::Json => self.compile_data_dep("json", &resolved.source)?,
                    ModuleKind::Text => self.compile_data_dep("text", &resolved.source)?,
                    ModuleKind::Bytes => return Err("bytes module not supported".into()),
                };
                path_stack.pop();
                ctx.nested.push(dep_ir);
                let fn_reg = ctx.alloc_reg();
                ctx.inst(Inst::create_closure(Operand::Reg(fn_reg), ctx.nested.len() as u16));
                self.emit_module_call(ctx, "__moduleEval", &[fn_reg])?
            };
            ctx.module_dep_ns_regs.insert(spec.clone(), dep_ns_reg);
            if resolved.path == module_path {
                // 自导入：绑定走别名语义（源导出在 body 执行中才就绪），
                // 不能像普通依赖那样链接期快照。
                ctx.module_self_import_specs.insert(spec.clone());
            }
        }

        // —— 导入绑定初始化（const 语义；命名空间绑定直接引用 ns 对象）——
        let own_export_names = module_own_export_names(body);
        // 无命名 `export * from 'x'` 会把依赖的全部导出转发给自身；自导入名可能来自
        // star 再导出，编译期无法静态判定该名是否存在于自身导出，故链接校验放宽。
        let has_unnamed_star = body
            .iter()
            .any(|s| matches!(s, Statement::ExportAllDeclaration(e) if e.exported.is_none()));
        // 自导入（import from 自身）的绑定在 body 执行中由 export 语句回写：
        // prelude 先初始化为 undefined（规范：实例化后未求值前 var 源绑定为
        // undefined），并登记 导出名 → 本地绑定槽 供回写。
        for stmt in body {
            if let Statement::ImportDeclaration(imp) = stmt {
                let dep_spec = imp.source.value.to_string();
                let dep_ns_reg = *ctx
                    .module_dep_ns_regs
                    .get(&dep_spec)
                    .ok_or_else(|| format!("module dependency missing: {dep_spec}"))?;
                let is_self = ctx.module_self_import_specs.contains(&dep_spec);
                if let Some(specifiers) = &imp.specifiers {
                    for sp in specifiers {
                        match sp {
                            ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                let imported = module_export_name_str(&s.imported);
                                if is_self {
                                    if !own_export_names.contains(&imported) && !has_unnamed_star {
                                        return Err(format!("requested module export is not exported: {imported}"));
                                    }
                                    let undef_idx = ctx.add_constant(Constant::Undefined);
                                    let undef_reg = ctx.alloc_reg();
                                    ctx.inst(Inst::load_const(Operand::Reg(undef_reg), undef_idx));
                                    self.emit_bind_target(
                                        s.local.name.as_str(),
                                        undef_reg,
                                        VariableDeclarationKind::Const,
                                        true,
                                        false,
                                        ctx,
                                    )?;
                                    if let Ok(reg) = ctx.lookup(s.local.name.as_str()) {
                                        ctx.module_self_aliases.insert(imported, reg);
                                    }
                                } else {
                                    let name_reg = self.load_string_const(&imported, ctx);
                                    let val_reg =
                                        self.emit_module_call(ctx, "__moduleLinkGet", &[dep_ns_reg, name_reg])?;
                                    self.emit_bind_target(
                                        s.local.name.as_str(),
                                        val_reg,
                                        VariableDeclarationKind::Const,
                                        true,
                                        false,
                                        ctx,
                                    )?;
                                }
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                if is_self {
                                    if !own_export_names.contains(DEFAULT_EXPORT_NAME) {
                                        return Err("requested module export is not exported: default".into());
                                    }
                                    let undef_idx = ctx.add_constant(Constant::Undefined);
                                    let undef_reg = ctx.alloc_reg();
                                    ctx.inst(Inst::load_const(Operand::Reg(undef_reg), undef_idx));
                                    self.emit_bind_target(
                                        s.local.name.as_str(),
                                        undef_reg,
                                        VariableDeclarationKind::Const,
                                        true,
                                        false,
                                        ctx,
                                    )?;
                                    if let Ok(reg) = ctx.lookup(s.local.name.as_str()) {
                                        ctx.module_self_aliases.insert(DEFAULT_EXPORT_NAME.to_string(), reg);
                                    }
                                } else {
                                    let name_reg = self.load_string_const(DEFAULT_EXPORT_NAME, ctx);
                                    let val_reg =
                                        self.emit_module_call(ctx, "__moduleLinkGet", &[dep_ns_reg, name_reg])?;
                                    self.emit_bind_target(
                                        s.local.name.as_str(),
                                        val_reg,
                                        VariableDeclarationKind::Const,
                                        true,
                                        false,
                                        ctx,
                                    )?;
                                }
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                // 命名空间绑定引用 ns 对象本身：自导入时同一对象，
                                // body 执行后导出自然可见。
                                self.emit_bind_target(
                                    s.local.name.as_str(),
                                    dep_ns_reg,
                                    VariableDeclarationKind::Const,
                                    true,
                                    false,
                                    ctx,
                                )?;
                            }
                        }
                    }
                }
            }
        }

        // —— re-export（export { x } from 'mod'）链接检查：实例化期解析，须先于
        // body 求值。$DONOTEVALUATE 是 test262 测试框架的断言工具，断言链接错误先于
        // body 求值抛出，故链接检查必须先于 body 发射。——
        for stmt in body {
            if let Statement::ExportNamedDeclaration(exp) = stmt {
                if exp.declaration.is_some() {
                    continue;
                }
                if let Some(src) = &exp.source {
                    let dep_spec = src.value.to_string();
                    if ctx.module_self_import_specs.contains(&dep_spec) {
                        continue; // 自身导出未就绪，body 执行中再解析
                    }
                    if let Some(&dep_ns_reg) = ctx.module_dep_ns_regs.get(&dep_spec) {
                        for spec in &exp.specifiers {
                            let local_name = module_export_name_str(&spec.local);
                            let name_reg = self.load_string_const(&local_name, ctx);
                            self.emit_module_call(ctx, "__moduleLinkGet", &[dep_ns_reg, name_reg])?;
                        }
                    }
                }
            }
        }

        // —— body（函数声明 hoisting 先发，随后其余；import 已由 prelude 处理）——
        // directive 序言是 AST 独立字段（顶格字符串字面量，不在 body）：纯字符串
        // 无副作用，按源序先于 body 发射，与脚本 emit_program 的发射口径一致。
        for dir in &program.directives {
            self.emit_string_literal_expression(&dir.expression, ctx)?;
        }
        for stmt in body {
            if is_hoisted_function_decl(stmt) {
                self.emit_statement(stmt, ctx)?;
            }
        }
        for stmt in body {
            if is_hoisted_function_decl(stmt) {
                continue;
            }
            if matches!(stmt, Statement::ImportDeclaration(_)) {
                continue;
            }
            self.emit_statement(stmt, ctx)?;
        }

        // —— 收尾：封冻命名空间并返回（顶层 RETURN 亦终止 run）——
        self.emit_module_call(ctx, "__moduleSeal", &[ns_reg])?;
        if top_level {
            // 顶层模块求值完成值为空记录（对外表现 undefined）；依赖模块求值必须返回
            // 命名空间对象（`__moduleEval` 以依赖返回值作命名空间）——两条路径的返回
            // 值契约不同，顶层路径不得复用依赖路径的返回值。
            let undef_idx = ctx.add_constant(Constant::Undefined);
            let r = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(r), undef_idx));
            ctx.inst(Inst::ret(Operand::Reg(r), 0, 0));
        } else {
            ctx.inst(Inst::ret(Operand::Reg(ns_reg), 0, 0));
        }
        Ok(())
    }

    /// 编译 JS 依赖模块（递归）。
    fn compile_js_dep(
        &self, resolved: &ResolvedModule, loader: &mut dyn ModuleSourceLoader, path_stack: &mut Vec<String>,
    ) -> Result<IRFunction, String> {
        let alloc = oxide_parser::Allocator::default();
        let program = oxide_parser::parse_module(&alloc, &resolved.source).map_err(|errs| {
            format!(
                "module parse error: {}",
                errs.first().map(|e| e.message.as_str()).unwrap_or("parse failed")
            )
        })?;
        let mut ctx = CompileCtx::new();
        ctx.is_global_scope = false;
        // 依赖模块经 parse_module 解析，顶层恒严格模式。
        ctx.is_strict = true;
        self.emit_module_into_ctx(&program, &resolved.path, loader, path_stack, &mut ctx, false)?;
        Ok(ctx.assemble_ir(ParamLayout { base: 0, count: 0 }, None))
    }

    /// 编译数据模块（json/text）：body 仅为 __moduleData 调用 + RETURN。
    fn compile_data_dep(&self, kind: &str, content: &str) -> Result<IRFunction, String> {
        let mut ctx = CompileCtx::new();
        let kind_reg = self.load_string_const(kind, &mut ctx);
        let content_reg = self.load_string_const(content, &mut ctx);
        let val_reg = self.emit_module_call(&mut ctx, "__moduleData", &[kind_reg, content_reg])?;
        ctx.inst(Inst::ret(Operand::Reg(val_reg), 0, 0));
        Ok(ctx.assemble_ir(ParamLayout { base: 0, count: 0 }, None))
    }

    /// 内部辅助调用：CALL_NATIVE 到全局 native（__module*），返回结果寄存器。
    pub(crate) fn emit_module_call(&self, ctx: &mut CompileCtx, name: &str, arg_regs: &[u32]) -> Result<u32, String> {
        let callee_reg = ctx.lookup_or_builtin(name)?;
        let this_reg = ctx.alloc_reg();
        let undef_idx = ctx.add_constant(Constant::Undefined);
        ctx.inst(Inst::load_const(Operand::Reg(this_reg), undef_idx));
        // CALL_NATIVE 按 regs[first_arg + i] 连续读参数：把非连续虚拟寄存器（vreg，经寄存器
        // 分配后映射为物理号）打包为连续寄存器块。
        let mut packed = arg_regs.to_vec();
        let first_arg_reg = if packed.is_empty() { this_reg } else { pack_arg_regs(&mut packed, ctx) };
        ctx.inst(Inst::call_native(
            Operand::Reg(callee_reg),
            Operand::Reg(this_reg),
            Operand::Reg(first_arg_reg),
            packed.len() as u8,
        ));
        let result_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(result_reg), Operand::None, Operand::None));
        Ok(result_reg)
    }

    fn load_string_const(&self, s: &str, ctx: &mut CompileCtx) -> u32 {
        let idx = ctx.add_constant(Constant::String(s.to_string()));
        let reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(reg), idx));
        reg
    }

    /// 自导入别名回写：导出名若被本模块 self-import 绑定别名引用，
    /// export 语句执行时同步把值写回本地绑定槽。
    fn emit_self_alias_write(&self, ctx: &mut CompileCtx, export_name: &str, value_reg: u32) {
        if let Some(&alias_reg) = ctx.module_self_aliases.get(export_name) {
            ctx.inst(Inst::new(
                OpCode::STORE_VAR,
                Operand::Reg(alias_reg),
                Operand::Reg(value_reg),
                Operand::None,
            ));
        }
    }

    fn emit_module_set(&self, ctx: &mut CompileCtx, ns_reg: u32, name: &str, value_reg: u32) -> Result<(), String> {
        let name_reg = self.load_string_const(name, ctx);
        self.emit_module_call(ctx, "__moduleSet", &[ns_reg, name_reg, value_reg])?;
        Ok(())
    }

    fn load_var_reg(&self, name: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        let var_reg = ctx.lookup(name)?;
        let val_reg = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(val_reg), Operand::Reg(var_reg), Operand::None));
        Ok(val_reg)
    }

    /// export 语句 emit：声明照常 + 导出值就地注册到命名空间。
    pub(crate) fn emit_module_export_domain(
        &self, stmt: &Statement, ctx: &mut CompileCtx,
    ) -> Result<Option<u32>, String> {
        let ns_reg = ctx.module_ns_reg.ok_or_else(|| "export outside module context".to_string())?;
        match stmt {
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(decl) = &exp.declaration {
                    match decl {
                        Declaration::VariableDeclaration(vd) => {
                            self.emit_variable_declaration(vd, ctx)?;
                            for d in &vd.declarations {
                                if let BindingPattern::BindingIdentifier(bi) = &d.id {
                                    let name = bi.name.as_str();
                                    let val_reg = self.load_var_reg(name, ctx)?;
                                    self.emit_module_set(ctx, ns_reg, name, val_reg)?;
                                    self.emit_self_alias_write(ctx, name, val_reg);
                                }
                            }
                        }
                        Declaration::FunctionDeclaration(fd) => {
                            self.emit_function_declaration(fd, ctx)?;
                            if let Some(id) = &fd.id {
                                let name = id.name.as_str();
                                let val_reg = self.load_var_reg(name, ctx)?;
                                self.emit_module_set(ctx, ns_reg, name, val_reg)?;
                                self.emit_self_alias_write(ctx, name, val_reg);
                            }
                        }
                        Declaration::ClassDeclaration(cl) => {
                            self.emit_class_declaration(cl, ctx)?;
                            if let Some(id) = &cl.id {
                                let name = id.name.as_str();
                                let val_reg = self.load_var_reg(name, ctx)?;
                                self.emit_module_set(ctx, ns_reg, name, val_reg)?;
                                self.emit_self_alias_write(ctx, name, val_reg);
                            }
                        }
                        _ => return Err("unsupported export declaration".into()),
                    }
                } else {
                    let dep_ns_opt = exp
                        .source
                        .as_ref()
                        .map(|src| ctx.module_dep_ns_regs.get(src.value.as_str()).copied());
                    for spec in &exp.specifiers {
                        let local_name = module_export_name_str(&spec.local);
                        let exported_name = module_export_name_str(&spec.exported);
                        let val_reg = match dep_ns_opt {
                            Some(Some(dep_ns_reg)) => {
                                let name_reg = self.load_string_const(&local_name, ctx);
                                self.emit_module_call(ctx, "__moduleLinkGet", &[dep_ns_reg, name_reg])?
                            }
                            Some(None) => {
                                return Err(format!(
                                    "export from unknown module: {}",
                                    exp.source.as_ref().unwrap().value
                                ))
                            }
                            None => self.load_var_reg(&local_name, ctx)?,
                        };
                        self.emit_module_set(ctx, ns_reg, &exported_name, val_reg)?;
                    }
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                let val_reg = match &exp.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(fd) => {
                        let reg = self.emit_function_expression(fd, ctx)?;
                        if let Some(id) = &fd.id {
                            self.emit_bind_target(
                                id.name.as_str(),
                                reg,
                                VariableDeclarationKind::Const,
                                true,
                                false,
                                ctx,
                            )?;
                        } else {
                            set_implicit_name_of_last_nested(ctx, DEFAULT_EXPORT_NAME);
                        }
                        reg
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(cl) => {
                        let reg = self.emit_class_with_binding(
                            cl,
                            ctx,
                            None,
                            cl.id.is_none().then_some(DEFAULT_EXPORT_NAME),
                        )?;
                        if let Some(id) = &cl.id {
                            self.emit_bind_target(
                                id.name.as_str(),
                                reg,
                                VariableDeclarationKind::Const,
                                true,
                                false,
                                ctx,
                            )?;
                        }
                        reg
                    }
                    other => {
                        let expr = other
                            .as_expression()
                            .ok_or_else(|| "unsupported export default declaration".to_string())?;
                        if crate::is_anonymous_function_definition(expr) {
                            match strip_parens(expr) {
                                // 匿名类：构造器落名后还会 push 方法子模块，必须经
                                // implicit_name 写构造器；具名类由 ctor_name 优先。
                                Expression::ClassExpression(cl) => {
                                    self.emit_class_with_binding(cl, ctx, None, Some(DEFAULT_EXPORT_NAME))?
                                }
                                // 函数/生成器/箭头：发射期只 push 一个子模块，末项即目标；
                                // 具名函数表达式已有名，守卫不覆盖。
                                _ => {
                                    let reg = self.emit_expression(expr, ctx)?;
                                    set_implicit_name_of_last_nested(ctx, DEFAULT_EXPORT_NAME);
                                    reg
                                }
                            }
                        } else {
                            self.emit_expression(expr, ctx)?
                        }
                    }
                };
                self.emit_module_set(ctx, ns_reg, DEFAULT_EXPORT_NAME, val_reg)?;
                self.emit_self_alias_write(ctx, DEFAULT_EXPORT_NAME, val_reg);
            }
            Statement::ExportAllDeclaration(exp) => {
                let source = exp.source.value.as_str();
                let dep_ns_reg = *ctx
                    .module_dep_ns_regs
                    .get(source)
                    .ok_or_else(|| format!("export * from unknown module: {source}"))?;
                if let Some(exported) = &exp.exported {
                    // export * as ns from '...'：命名空间再导出。
                    let exported_name = module_export_name_str(exported);
                    self.emit_module_set(ctx, ns_reg, &exported_name, dep_ns_reg)?;
                } else {
                    self.emit_module_call(ctx, "__moduleStar", &[ns_reg, dep_ns_reg])?;
                }
            }
            _ => return Err("unsupported module export statement".into()),
        }
        Ok(None)
    }
}
