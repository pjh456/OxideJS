//! emit：AST → IR 代码生成（parse → IR → bytecode 中段）。
//!
//! `Emitter` 提供各语法域的 emit_* 方法；核心状态集中在 `CompileCtx`
//! （见 `compile_ctx.rs`）。产出分域组合的 `IRFunction`，由
//! `oxide_ir::lower` 降为 bytecode。

use std::collections::HashSet;

use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

/// 是否为匿名函数定义（剥括号）：函数/箭头/class 表达式。
pub fn is_anonymous_function_definition(expr: &oxide_parser::Expression) -> bool {
    match expr {
        oxide_parser::Expression::ArrowFunctionExpression(_)
        | oxide_parser::Expression::FunctionExpression(_)
        | oxide_parser::Expression::ClassExpression(_) => true,
        oxide_parser::Expression::ParenthesizedExpression(p) => is_anonymous_function_definition(&p.expression),
        _ => false,
    }
}

use crate::compile_ctx::CompileCtx;

/// 常量池项（bytecode module 类型）re-export，供调用方构造常量。
pub use oxide_bytecode::module::Constant;
/// 变量声明种类（var/let/const），re-export 自 parser。
pub use oxide_parser::VariableDeclarationKind;
/// AST 语法树节点与运算符类型，re-export 自 parser。
pub use oxide_parser::{AssignmentOperator, BinaryOperator, Expression, Statement, UnaryOperator};

/// 编译入口。方法按语法域组织在 `impl Emitter` 中。
pub struct Emitter {
    /// 源码是否为 `oxide_kernel::string_forge::source_escape` 产物
    /// （eval/Function 动态编译）：源文本内的孤立 surrogate 单元 / FFFD 以
    /// `\uXXXX` 转义文本承载（oxc 不可见裸单元——Rust str 无孤立 surrogate，
    /// 转义文本是唯一注入形态），反斜杠原样透传。正则字面量的源文本切片据此
    /// 经 `source_escape_to_key` 还原池键 marker 形态入池（物化时
    /// `decode_key` 还原为原始单元，`.source` 按原始源返回）；静态源切片不含
    /// 注入 marker，走 `pool_key_plain`。
    source_encoded: bool,
}

/// 判断 f64 是否为整数值且在 i32 范围内（整数常量编码用）。
pub fn is_int_literal(value: f64) -> bool {
    value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64
}

/// 判断表达式是否无副作用（字面量/标识符/纯二元运算等）。
/// 用于可丢弃值的优化路径。
/// 逻辑运算符快速路径的"无副作用"判定：仅字面量/标识符/this 读取安全。
/// 算术表达式（`1 / a` 等）不得判为无副作用——对象操作数强转（ToNumber 触发
/// valueOf/toString/getter）可能在运行期抛错，急切求值会破坏 `||`/`&&` 短路。
pub fn is_side_effect_free(expr: &Expression) -> bool {
    let mut stack = vec![expr];
    while let Some(expr) = stack.pop() {
        match expr {
            Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::Identifier(_)
            | Expression::RegExpLiteral(_)
            | Expression::ThisExpression(_) => {}
            Expression::ParenthesizedExpression(p) => stack.push(&p.expression),
            _ => return false,
        }
    }
    true
}

pub(crate) const BUILTIN_GLOBALS: &[&str] = &[
    "NaN",
    "undefined",
    "Infinity",
    "globalThis",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "Function",
    "Error",
    "TypeError",
    "ReferenceError",
    "RangeError",
    "SyntaxError",
    "URIError",
    "EvalError",
    "eval",
    "SuppressedError",
    "DisposableStack",
    "AsyncDisposableStack",
    "Math",
    "JSON",
    "Promise",
    "AggregateError",
    "Date",
    "Set",
    "Map",
    "RegExp",
    "Symbol",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "Proxy",
    "WeakMap",
    "WeakSet",
    "WeakRef",
    "FinalizationRegistry",
    "Atomics",
    "SharedArrayBuffer",
    "ArrayBuffer",
    "DataView",
    "Iterator",
    "BigInt",
    "TypedArray",
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float32Array",
    "Float64Array",
    "BigInt64Array",
    "BigUint64Array",
    "Reflect",
    "escape",
    "unescape",
    "encodeURI",
    "decodeURI",
    "encodeURIComponent",
    "decodeURIComponent",
    // test262 宿主对象：编译期按已知全局解析，运行期由 VM 绑定（详见 bind_test262_host）。
    "$262",
];

/// 只读全局内置名：全局对象上的数据属性为 {writable:false, configurable:false}，
/// 对它们的 put 永不成功。写路径命中这些名字的全局绑定（非局部遮蔽）时编译期
/// 拦截：sloppy 静默丢弃、strict 抛 TypeError；BUILTIN_GLOBALS 内其余名属性可写
/// （writable:true），标识符写须双写全局属性（见 targets_writable_builtin）。
pub(crate) const NON_WRITABLE_GLOBAL_BUILTINS: &[&str] = &["undefined", "NaN", "Infinity"];

impl Emitter {
    /// 构造空 `Emitter`（静态编译口径：源文本为良形 UTF-8，无注入 marker）。
    pub fn new() -> Self {
        Self { source_encoded: false }
    }

    /// 置位编码源口径（见 [`Emitter::source_encoded`]）：仅动态编译入口
    /// （eval / Function 构造器）经编译器设置。
    pub fn with_source_encoded(mut self, enable: bool) -> Self {
        self.source_encoded = enable;
        self
    }

    pub(crate) fn emit_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        match stmt {
            Statement::ExpressionStatement(_) | Statement::ReturnStatement(_) | Statement::EmptyStatement(_) => {
                self.emit_basic_domain(stmt, ctx)
            }
            Statement::BlockStatement(_) => self.emit_block_domain(stmt, ctx),
            Statement::VariableDeclaration(_) | Statement::FunctionDeclaration(_) | Statement::ClassDeclaration(_) => {
                self.emit_declaration_domain(stmt, ctx)
            }
            Statement::IfStatement(_) => self.emit_control_domain(stmt, ctx),
            Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_) => self.emit_iteration_domain(stmt, ctx),
            Statement::SwitchStatement(_) => self.emit_switch_domain(stmt, ctx),
            Statement::ThrowStatement(_) | Statement::TryStatement(_) => self.emit_exception_domain(stmt, ctx),
            Statement::BreakStatement(b) => self.emit_break_statement(b, ctx),
            Statement::ContinueStatement(c) => self.emit_continue_statement(c, ctx),
            Statement::LabeledStatement(ls) => self.emit_labeled_statement(ls, ctx),
            Statement::WithStatement(_) => self.emit_with_domain(stmt, ctx),
            Statement::ExportNamedDeclaration(_)
            | Statement::ExportDefaultDeclaration(_)
            | Statement::ExportAllDeclaration(_) => self.emit_module_export_domain(stmt, ctx),
            _ => Ok(None),
        }
    }

    pub(crate) fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::NumericLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::RegExpLiteral(_) => self.emit_literal(expr, ctx),
            Expression::BinaryExpression(_)
            | Expression::PrivateInExpression(_)
            | Expression::UnaryExpression(_)
            | Expression::ConditionalExpression(_)
            | Expression::LogicalExpression(_)
            | Expression::UpdateExpression(_) => self.emit_operator(expr, ctx),
            Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_)
            | Expression::ChainExpression(_) => self.emit_member_domain(expr, ctx),
            Expression::ObjectExpression(_) | Expression::ArrayExpression(_) => self.emit_object_domain(expr, ctx),
            Expression::AssignmentExpression(assign) => self.emit_assignment_expression(assign, ctx),
            Expression::TemplateLiteral(_) | Expression::TaggedTemplateExpression(_) => {
                self.emit_template_domain(expr, ctx)
            }
            Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ClassExpression(_)
            | Expression::NewExpression(_) => self.emit_function_domain(expr, ctx),
            Expression::Identifier(ident) => self.emit_identifier_expression(ident, ctx),
            Expression::YieldExpression(ye) => self.emit_yield_expression(ye, ctx),
            Expression::AwaitExpression(ae) => self.emit_await_expression(ae, ctx),
            Expression::CallExpression(_) => self.emit_call_domain(expr, ctx),
            Expression::ThisExpression(_) => self.emit_this_expression(ctx),
            Expression::SequenceExpression(seq) => self.emit_sequence_expression(seq, ctx),
            Expression::ParenthesizedExpression(p) => self.emit_parenthesized_expression(p, ctx),
            Expression::MetaProperty(mp) => self.emit_meta_property_expression(mp, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }

    /// 隐式全局写（sloppy 未声明标识符写）：把值寄存器写到全局对象可写/可枚举/
    /// 可配置数据属性（未解析引用上的 PutValue 语义）。全局对象由 VM 运行期从
    /// session 解析，不依赖 this。
    ///
    /// # 边界与前提
    /// - 仅读写登记集并集命中的寄存器调用（未声明标识符写）：未声明名谁先引用谁
    ///   登记全局槽，读侧与写侧登记同属一个全局槽，写一律须穿透到全局对象。
    ///
    /// # 副作用
    /// - 定义全局对象数据属性（可写/可枚举/可配置），属性缺失时新建。
    pub(crate) fn emit_implicit_global_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        ctx.inst(Inst::define_global_prop_c(Operand::Reg(val_reg), idx));
    }

    /// 严格模式未声明写：发射 ReferenceError 抛错指令序列（未解析引用不可 put，
    /// 值无关，编译期拦截）。错误消息格式与读侧 LOAD_GLOBAL 运行期消息一致。
    ///
    /// # 副作用
    /// - 发射 THROW 指令序列，其后控制流不可达，dummy 值保持寄存器良定义。
    pub(crate) fn emit_strict_undeclared_write(&self, name: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        self.emit_throw_error("ReferenceError", &format!("{name} is not defined"), ctx)
    }

    /// 把脚本顶层 var/function 绑定的当前值同步写入全局对象属性，使顶层声明
    /// 可经 `globalThis` 反射（脚本环境记录的 var 绑定全局对象属性）。
    ///
    /// # 边界与前提
    /// - 仅顶层模块上下文调用。
    /// - let/const/class 不落全局对象，不得调用本函数。
    ///
    /// # 副作用
    /// - 普通脚本：定义可写/可枚举/不可配置数据属性（经顶层 `this` = 全局对象）。
    /// - eval 脚本（`is_eval_script`）：属性可配置（configurable:true），全局对象
    ///   经 session 解析——eval var 声明允许后续 redefine/delete。
    pub(crate) fn emit_global_prop_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        if ctx.is_eval_script {
            ctx.inst(Inst::define_global_prop_c(Operand::Reg(val_reg), idx));
        } else {
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::define_global_prop(Operand::This, Operand::Reg(val_reg), Operand::Reg(key_reg)));
        }
    }

    /// 顶层函数声明的 A 侧同步写：eval 脚本沿用 [`emit_global_prop_write`] 的
    /// 0x98 分支（属性可配置，创建检查面不覆盖 eval 动态臂）；普通脚本经顶层
    /// This 走 0x9E（CreateGlobalFunctionBinding 三臂，声明检查由 GDI 序言的
    /// 0x9D 段预先完成）。
    pub(crate) fn emit_global_func_bind_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        if ctx.is_eval_script {
            self.emit_global_prop_write(name, val_reg, ctx);
            return;
        }
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
        ctx.inst(Inst::define_global_func_bind(Operand::This, Operand::Reg(val_reg), Operand::Reg(key_reg)));
    }

    /// 顶层 var 声明带初始化的 A 侧同步：PutValue 语义——既有不可写数据属性
    /// strict 抛 TypeError / sloppy 静默 no-op；可写照原描述符仅更值。脚本与
    /// eval 均经 session 解析全局对象，不依赖顶层 this。
    ///
    /// # 边界与前提
    /// - 仅顶层模块上下文调用；builtin 名不走本写点（保持 GDI 零动作写点）。
    ///
    /// # 副作用
    /// - 更新全局对象数据属性，失败面抛 TypeError（strict 不可写）。
    pub(crate) fn emit_global_put_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        ctx.inst(Inst::define_global_prop_c(Operand::Reg(val_reg), idx));
    }

    /// 名字是否为顶层已声明 var（A 侧单一真值）：在顶层 var 名集内，且当前解析
    /// 绑定落在全局作用域（scope 0）——嵌套函数内的局部同名遮蔽命中更高作用域，
    /// 判定为局部而非顶层，走既有 cell/寄存器路径。
    pub(crate) fn is_global_tier_name(&self, ctx: &CompileCtx, name: &str) -> bool {
        ctx.global_tier_names.contains(name) && matches!(ctx.scopes.symbols.lookup_any_binding(name), Some((_, 0)))
    }

    /// 顶层已声明 var 的裸写落到全局对象属性（A 侧单一真值）：顶层普通脚本经
    /// This=全局对象走 0x6F；顶层 eval 与嵌套函数 This≠全局对象，经 session 解析
    /// 走 0x98（c:true，不升级既有 c:false 描述符，只更值）。
    pub(crate) fn emit_tier_global_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        if ctx.is_global_scope && !ctx.is_eval_script {
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::define_global_prop(Operand::This, Operand::Reg(val_reg), Operand::Reg(key_reg)));
        } else {
            ctx.inst(Inst::define_global_prop_c(Operand::Reg(val_reg), idx));
        }
    }

    /// GDI 序言：顶层 var 全局属性 define-if-absent。既有属性（数据或 accessor）
    /// 零动作——CreateGlobalVarBinding 对既有数据描述符零修改（不更值）；缺失新建。
    /// 顶层普通脚本经 This 走 0x99；eval 经 session 走 0x9A。
    pub(crate) fn emit_global_prop_write_if_absent(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        if ctx.is_eval_script {
            ctx.inst(Inst::define_global_prop_c_if_absent(Operand::Reg(val_reg), idx));
        } else {
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::define_global_prop_if_absent(
                Operand::This,
                Operand::Reg(val_reg),
                Operand::Reg(key_reg),
            ));
        }
    }

    /// GDI step 9 检查阶段：顶层函数声明名按声明逆序逐名去重，每名发一条 0x9D
    /// （CanDeclareGlobalFunction 运行期判定，撞既有不可配置非可写数据属性或
    /// 不可扩展上的缺失名抛 TypeError）。发射位必须位于 GDI var 序言之先——
    /// 检查失败时任何绑定（含 var 序言新建属性）不得实例化。
    ///
    /// # 边界与前提
    /// - 仅普通脚本调用（`is_eval_script` 面由调用点门控）；eval 动态臂与
    ///   eval 三常量编译期门禁零触碰。
    /// - 仅遍历语句列表直接子级具名函数声明（生成器/异步声明同节点类型，天然
    ///   覆盖）；块内函数声明与 `export default function` 不入全局检查面。
    pub(crate) fn emit_gdi_func_decl_checks(&self, stmts: &[Statement], ctx: &mut CompileCtx) {
        let names = self.collect_top_level_function_names_ordered(stmts);
        let mut seen = HashSet::new();
        for name in names.iter().rev() {
            // 逆序首见即源序最后声明者（规范去重口径），重名只查一次。
            if !seen.insert(name.as_str()) {
                continue;
            }
            let idx = ctx.add_constant(Constant::String(name.clone()));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::can_declare_global_func(Operand::This, Operand::Reg(key_reg)));
        }
    }

    /// 把完整程序编译为顶层模块体的 IRFunction。
    ///
    /// 调用方为 `oxide_compiler::Compiler::compile`：本函数完成 emit 半程，
    /// 随后由 `oxide_ir::lower::lower` 降为字节码。
    /// `repl_persist` 为 true 时脚本顶层 let/const 也写全局对象（REPL 跨轮次持久）；
    /// `is_eval_script` 为 true 时顶层 var/function 声明落全局属性 configurable:true。
    /// 顶层 var 的全局对象属性在求值开始前统一创建（值 undefined），声明语句
    /// 保持赋值语义——声明语句出现之前的读取（typeof/反射/自引用）即见绑定。
    pub fn emit_program(
        &self, program: &oxide_parser::Program, repl_persist: bool, is_eval_script: bool,
    ) -> Result<IRFunction, String> {
        crate::emit_debug!("emit_program: {} stmts", program.body.len());
        let mut ctx = CompileCtx::new();
        // 脚本顶层：var/function 声明需落到全局对象，let/const/class 不进全局。
        ctx.is_global_scope = true;
        ctx.repl_persist = repl_persist;
        ctx.is_eval_script = is_eval_script;
        ctx.source_encoded = self.source_encoded;
        // 脚本顶层严格模式由源码 "use strict" directive 决定（嵌套函数经父 ctx 继承）。
        ctx.is_strict = program.has_use_strict_directive();

        // eval 代码顶层函数声明撞不可写全局内置（三常量的全局绑定在任何符合规范的
        // 实现中均不可配置，声明实例化无法建立全局绑定）：规范在建立全局 var 绑定
        // 之前抛 TypeError（step 8 的 abrupt 先于绑定实例化），故 throw 发为程序首
        // 指令，其后预声明/序言/声明发射均不可达（寄存器保持良定义，运行期零开销）。
        // 严格 eval 代码函数声明绑定 eval 自身 lexical 环境、不触全局、不抛，门禁
        // 随 !is_strict 关闭，其写点抑制在声明发射处。
        if ctx.is_eval_script && !ctx.is_strict {
            for name in self.collect_top_level_function_names(&program.body) {
                if NON_WRITABLE_GLOBAL_BUILTINS.contains(&name.as_str()) {
                    let _ = self.emit_throw_error(
                        "TypeError",
                        &format!("Cannot declare function '{name}': global property is not configurable"),
                        &mut ctx,
                    )?;
                    break;
                }
            }
        }

        // GDI step 9 函数臂检查阶段（运行期，先于任何绑定实例化）：普通脚本
        // 顶层函数声明撞既有不可配置全局属性抛 TypeError，sloppy/strict 同形。
        // eval 面零触碰：静态三常量面由上方 eval 三常量编译期门禁拦，动态臂另归口。
        if !ctx.is_eval_script {
            self.emit_gdi_func_decl_checks(&program.body, &mut ctx);
        }

        self.predeclare_function_declarations(&program.body, &mut ctx);

        // 预注册 builtin 引用（先于任何临时寄存器），builtin 槽不进入临时寄存器池。
        self.pre_register_builtin_references(&program.body, &mut ctx);

        // 预声明顶层 `var` 名，使首个 sub-pass 中提升的函数声明能解析外层 var。
        self.predeclare_var_declarations(&program.body, &mut ctx);

        // 预声明顶层 `let`/`const`/`class`（未初始化 TDZ 占位）。受限全局名检查
        // 仅对脚本代码启用：脚本声明实例化查全局对象受限自有属性名，eval 代码
        // 声明实例化不查——门控随 is_eval_script 而非作用域标志（嵌套块内
        // is_global_scope 仍为 true，不能作门控）。
        let global_lexical = !ctx.is_eval_script;
        self.predeclare_lexical_declarations(&program.body, &mut ctx, global_lexical)?;

        // 顶层块级函数名 web-compat 外层绑定（sloppy、非 eval）：实例化 var 绑定
        // （新建 var 槽；顶层只读三常量名不可声明不建）、并入顶层 var 名集（裸
        // 读/写路由全局对象属性）、GDI 序言建属性（define-if-absent，既有属性
        // 零动作）。
        let mut block_fn_names: Vec<String> = Vec::new();
        if !ctx.is_strict && !ctx.is_eval_script {
            ctx.block_fn_suppressed = self.collect_block_fn_suppressed_names(&program.body, &ctx.param_names);
            block_fn_names = self.collect_block_function_names(&program.body);
            for name in &block_fn_names {
                if !ctx.block_fn_suppressed.contains(name) && !CompileCtx::is_non_writable_global_builtin(name) {
                    self.predeclare_var_name(name, &mut ctx);
                }
            }
        }

        // 闭包捕获分析（AST 级，emit 前确定）
        ctx.own_bindings = self.collect_own_binding_names(&[], &program.body);
        // 顶层已声明名（A 侧单一真值）：裸读走全局对象属性、裸写走描述符感知
        // A 侧写，不落镜像 cell——从捕获集剔除，使嵌套函数经继承 scope-0 直连全局。
        // 集 = 顶层 var 名 ∪ 顶层函数声明名：函数值是编译闭包，编译期不可得，
        // 其 A 侧值由首 sub-pass 以真闭包建立，先于任何用户代码。
        // 仅在此顶层调用点过滤：嵌套函数的局部同名遮蔽是独立绑定，其调用点不过滤。
        let var_names = self.collect_var_binding_names(&program.body);
        let mut tier_names = var_names.clone();
        tier_names.extend(self.collect_top_level_function_names(&program.body));
        // 块级函数泄漏名并入：求值期写回与头写同走全局对象属性（A 侧单一真值）。
        for name in &block_fn_names {
            if !ctx.block_fn_suppressed.contains(name) && !CompileCtx::is_non_writable_global_builtin(name) {
                tier_names.insert(name.clone());
            }
        }
        ctx.global_tier_names = tier_names;
        ctx.captured_bindings = self.collect_captured_bindings(&program.body, &[], &ctx.own_bindings);
        ctx.captured_bindings.retain(|n, _| !ctx.global_tier_names.contains(n));

        // 顶层 var 入口实例化：被捕获的 var 名统一 MAKE_CELL(undefined)，使 var
        // 声明语句执行前创建的闭包读取到 undefined（脚本 GlobalDeclarationInstantiation
        // 语义），而非占位 cell 的 TDZ 误报。声明语句的 MAKE_CELL 覆盖此初值。
        // 名集保持 var-only：函数声明名从不入捕获集（tier 剔除），无需入口 cell。
        let var_names: Vec<String> = var_names
            .into_iter()
            .filter(|n| ctx.captured_bindings.contains_key(n))
            .collect();
        if !var_names.is_empty() {
            let undef_reg = self.emit_undefined(&mut ctx);
            for name in var_names {
                if let Some(&cell_idx) = ctx.captured_bindings.get(&name) {
                    ctx.inst(Inst::new(
                        OpCode::MAKE_CELL,
                        Operand::Reg(undef_reg),
                        Operand::Imm(cell_idx as u16),
                        Operand::None,
                    ));
                }
            }
        }

        // 全局声明实例化序言：脚本求值前为顶层 var 名创建全局对象属性（值 undefined），
        // 使声明语句执行前的读取（typeof、反射、自引用）可经全局对象见绑定。
        // 顶层函数声明名不进序言：函数值是编译闭包，编译期不可得，其 A 侧值由
        // 首 sub-pass 的声明写以真闭包建立，先于任何用户代码（首 sub-pass 仅发
        // 函数声明），无 undefined 读窗口。
        // CreateGlobalVarBinding 对既有属性零动作：define-if-absent 只在属性缺失时
        // 新建，可写/不可写/可配置既有属性（含值）一律保留。builtin 名的全局属性
        // 运行期预存（session 绑定）：缺失分支写入值取 builtin 镜像槽（run 起点
        // 预载全局属性值），既有属性零动作，值幂等保留。
        // 序言名集 = 顶层 var 名 ∪ 顶层块级函数泄漏名（web-compat 外层绑定同样
        // 在求值前实例化，同走 define-if-absent）。
        let mut gdi_var_names = self.collect_var_binding_names(&program.body);
        gdi_var_names.extend(
            block_fn_names
                .iter()
                .filter(|n| !ctx.block_fn_suppressed.contains(*n) && !CompileCtx::is_non_writable_global_builtin(n))
                .cloned(),
        );
        if !gdi_var_names.is_empty() {
            let undef_reg = self.emit_undefined(&mut ctx);
            for name in &gdi_var_names {
                let value_reg = if CompileCtx::is_known_builtin(name) {
                    // 镜像槽未预登记时（解构 pattern 名等）就地登记，run 起点预载。
                    ctx.lookup_or_builtin(name).unwrap_or(undef_reg)
                } else {
                    undef_reg
                };
                self.emit_global_prop_write_if_absent(name, value_reg, &mut ctx);
            }
        }

        // 首个 sub-pass：发函数声明（hoisting），保证任何代码运行前函数对象已就绪。
        for stmt in &program.body {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                self.emit_statement(stmt, &mut ctx)?;
            }
        }

        // 第二个 sub-pass：发其余所有语句。
        let mut last_result: Option<u32> = None;
        // directive 序言是 AST 独立字段（顶格字符串字面量，不在 body）：作为语句
        // 序列的头前缀按源序先于 body 发射。完成值即字符串值本身，参与"最后非空
        // 完成值"收敛；严格模式标志由 has_use_strict_directive 独立处理，此处不涉。
        for dir in &program.directives {
            last_result = Some(self.emit_string_literal_expression(&dir.expression, &mut ctx)?);
        }
        for stmt in &program.body {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                continue; // Already emitted above
            }
            // 空完成值语句（变量/函数等声明）不覆写此前结果：保留最后一个非空
            // 完成值，与函数体 emit_body_stmts 的收敛口径一致。
            if let Some(r) = self.emit_statement(stmt, &mut ctx)? {
                last_result = Some(r);
            }
        }
        if let Some(r) = last_result {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::None, Operand::Reg(r), Operand::None));
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.inst(Inst::load_const(Operand::None, undef_idx));
        }
        ctx.inst(Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None));

        let ir = ctx.assemble_ir(
            oxide_ir::ParamLayout {
                // 顶层模块无父函数：base 恒 0。曾用 builtin_reg_map.len()，harness 前缀
                // 大时把模块自身低号 vreg 误判为父槽 → 恒等色收缩可分配色集、spill 增多。
                base: 0,
                count: 0,
            },
            None,
        );
        Ok(ir)
    }
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{BUILTIN_GLOBALS, NON_WRITABLE_GLOBAL_BUILTINS};
    use crate::prepass::RESTRICTED_GLOBAL_LEXICAL_NAMES;

    /// 漂移守卫：受限全局名集与 put 写拦截名单交叠名恒同步（两名单语义独立、
    /// 不互相派生，靠本断言防止改名/删名时单边漂移）。
    #[test]
    fn restricted_lexical_names_within_builtin_globals() {
        for name in RESTRICTED_GLOBAL_LEXICAL_NAMES {
            assert!(BUILTIN_GLOBALS.contains(name), "受限全局名缺失于 builtin 名单：{name}");
        }
    }

    /// 漂移守卫：只读名单是 builtin 母集子集，且只读/可写两谓词对母集构成
    /// 划分（不重不漏）——防三常量集或母集改名/删名时拦截面与双写面单边漂移。
    #[test]
    fn readonly_and_writable_partition_builtin_globals() {
        for name in NON_WRITABLE_GLOBAL_BUILTINS {
            assert!(BUILTIN_GLOBALS.contains(name), "只读全局名缺失于 builtin 名单：{name}");
        }
        for name in BUILTIN_GLOBALS {
            let readonly = NON_WRITABLE_GLOBAL_BUILTINS.contains(name);
            let writable = crate::CompileCtx::is_writable_builtin_global(name);
            assert!(readonly ^ writable, "builtin 名只读/可写归属漂移：{name}");
        }
    }

    /// 漂移守卫：可删名集自可写划分派生（可删 ⊆ 可写、只读三常量不入可删），
    /// 且宿主名 $262 恒不可删——防派生口径改动时 delete 面单边漂移。
    #[test]
    fn deletable_global_builtins_derive_from_writable_partition() {
        for name in BUILTIN_GLOBALS {
            let deletable = crate::CompileCtx::is_deletable_global_builtin(name);
            let writable = crate::CompileCtx::is_writable_builtin_global(name);
            assert!(!deletable || writable, "可删名缺失于可写划分：{name}");
            if NON_WRITABLE_GLOBAL_BUILTINS.contains(name) {
                assert!(!deletable, "只读三常量名误入可删集：{name}");
            }
        }
        assert!(!crate::CompileCtx::is_deletable_global_builtin("$262"), "不可删名误入可删集：$262");
    }
}
