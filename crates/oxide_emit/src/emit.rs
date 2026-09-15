//! emit：AST → IR 代码生成（parse → IR → bytecode 中段）。
//!
//! `Emitter` 提供各语法域的 emit_* 方法；核心状态集中在 `CompileCtx`
//! （见 `compile_ctx.rs`）。产出分域组合的 `IRFunction`，由
//! `oxide_ir::lower` 降为 bytecode。

use std::collections::HashSet;

use oxide_bytecode::module::UpvalueCapture;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::{LabelId, Operand};
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

use crate::compile_ctx::{CompileCtx, FieldBuffer};
use crate::symbol_table::{Binding, ScopeKind};

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

/// 函数体编译上下文：决定 `this`/`super` 绑定与参数前导（prologue）形态。
#[derive(Clone, Copy)]
pub enum FunctionBodyContext {
    /// 普通函数：自身 `this`、独立作用域。
    Ordinary,
    /// 箭头函数：词法捕获外层 `this`，不生成参数前导。
    Arrow,
    /// 类元素方法：按类语义处理 `super` 与 home object。
    ClassElement,
}

/// 参数规格：普通形参为标识符（可带默认值 initializer），解构形参用合成名 + 原始 pattern，
/// rest 形参为数组（无默认值，只能是最末形参）。
pub enum ParamSpec<'a> {
    Identifier {
        name: String,
        initializer: Option<&'a Expression<'a>>,
    },
    Pattern {
        synthetic_name: String,
        pattern: &'a oxide_parser::BindingPattern<'a>,
        initializer: Option<&'a Expression<'a>>,
    },
    Rest {
        name: String,
    },
}

impl ParamSpec<'_> {
    pub(crate) fn register_name(&self) -> &str {
        match self {
            Self::Identifier { name, .. } => name,
            Self::Pattern { synthetic_name, .. } => synthetic_name,
            Self::Rest { name } => name,
        }
    }
}

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

    /// 生成运行时抛 `kind` 类型错误的指令序列，返回一个未定义 dummy 寄存器
    /// 保证 THROW 后不可达控制流的寄存器良定义。
    ///
    /// # 步骤
    /// 1. 取全局错误构造器并 LOAD。
    /// 2. 加载错误消息常量，`new {kind}(msg)` 构造错误对象。
    /// 3. THROW 抛出；尾接 dummy 值保持后续读引用有确定寄存器。
    ///
    /// # 边界与前提
    /// - `kind` 必须是已注册的全局构造器名（如 "ReferenceError"/"TypeError"）。
    pub(crate) fn emit_throw_error(&self, kind: &str, msg: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        let ctor_reg = ctx.lookup_or_builtin(kind)?;
        let ctor = ctx.alloc_reg();
        ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::Reg(ctor), Operand::Reg(ctor_reg), Operand::None));
        let msg_reg = ctx.alloc_reg();
        let msg_idx = ctx.add_constant(Constant::String(msg.to_string()));
        ctx.inst(Inst::load_const(Operand::Reg(msg_reg), msg_idx));
        let exc_reg = ctx.alloc_reg();
        ctx.inst(Inst::new_expression(Operand::Reg(exc_reg), Operand::Reg(ctor), Operand::Reg(msg_reg), 1));
        ctx.inst(Inst::new(OpCode::THROW, Operand::Reg(exc_reg), Operand::None, Operand::None));
        let dummy = ctx.alloc_reg();
        let undef_idx = ctx.add_constant(Constant::Undefined);
        ctx.inst(Inst::load_const(Operand::Reg(dummy), undef_idx));
        Ok(dummy)
    }

    /// 生成运行时抛 ReferenceError 的指令序列（TDZ 访问专用，语义见 [`emit_throw_error`]）。
    pub(crate) fn emit_tdz_throw(&self, msg: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        self.emit_throw_error("ReferenceError", msg, ctx)
    }

    /// 赋值目标 TDZ 检查：未初始化绑定在赋值引用解析时抛 ReferenceError。
    /// 须在 RHS 求值之前调用（规范：赋值 LHS 的 ResolveBinding 先于 RHS 副作用）。
    ///
    /// # 副作用
    /// - TDZ 命中时发射 THROW 指令序列，其后指令不可达但保持寄存器良定义。
    pub(crate) fn emit_identifier_tdz_guard(&self, name: &str, ctx: &mut CompileCtx) -> Result<(), String> {
        if let Some((binding, _)) = ctx.scopes.symbols.lookup_any_binding(name) {
            if !binding.initialized {
                let _ = self.emit_tdz_throw(&format!("Cannot access '{name}' before initialization"), ctx)?;
            }
        }
        Ok(())
    }

    /// const 写检查：已初始化的 const 绑定再赋值编译期抛 TypeError（与槽值无关）。
    /// 简单赋值在 RHS 求值之后、复合/更新在读旧值之前调用；解构赋值在写目标时调用。
    ///
    /// # 副作用
    /// - const 命中时发射 THROW 指令序列，其后写指令不可达但保持寄存器良定义。
    pub(crate) fn emit_const_write_guard(&self, name: &str, ctx: &mut CompileCtx) -> Result<(), String> {
        if ctx.lookup_const_flag(name) {
            let _ = self.emit_throw_error("TypeError", "Assignment to constant variable", ctx)?;
        }
        Ok(())
    }

    /// 遍历解构 pattern 收集内嵌默认值表达式（AssignmentPattern.right）与
    /// 计算键表达式（`{[k]: a}` 的键，运行时求值）——二者都会在参数绑定期被
    /// 内部闭包引用，须一并纳入捕获分析。
    fn collect_pattern_default_exprs<'a>(
        &self, pattern: &'a oxide_parser::BindingPattern<'a>, out: &mut Vec<&'a oxide_parser::Expression<'a>>,
    ) {
        use oxide_parser::BindingPattern;
        match pattern {
            BindingPattern::AssignmentPattern(ap) => {
                out.push(&ap.right);
                self.collect_pattern_default_exprs(&ap.left, out);
            }
            BindingPattern::ArrayPattern(ap) => {
                for p in ap.elements.iter().flatten() {
                    self.collect_pattern_default_exprs(p, out);
                }
                if let Some(rest) = &ap.rest {
                    self.collect_pattern_default_exprs(&rest.argument, out);
                }
            }
            BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
                    if prop.computed {
                        out.push(prop.key.to_expression());
                    }
                    self.collect_pattern_default_exprs(&prop.value, out);
                }
                if let Some(rest) = &op.rest {
                    self.collect_pattern_default_exprs(&rest.argument, out);
                }
            }
            _ => {}
        }
    }

    // ── 闭包捕获分析（AST 级，时序无关）──

    /// 把 rest 形参的绑定模式追加为 `ParamSpec::Rest`：仅支持标识符形态（解构 rest 未支持）。
    pub(crate) fn push_rest_param<'a>(
        &self, argument: &'a oxide_parser::BindingPattern<'a>, out: &mut Vec<ParamSpec<'a>>,
    ) -> Result<(), String> {
        match argument {
            oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                out.push(ParamSpec::Rest { name: bi.name.to_string() });
            }
            _ => return Err("rest parameters with destructuring patterns not yet supported".into()),
        }
        Ok(())
    }

    /// 收集函数参数的绑定名（BindingIdentifier 形态）。
    pub(crate) fn extract_function_parts<'a>(
        &self, function: &'a oxide_parser::Function<'a>,
    ) -> Result<(Vec<ParamSpec<'a>>, &'a [Statement<'a>]), String> {
        let mut param_specs = Vec::new();
        for (idx, param) in function.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                    param_specs.push(ParamSpec::Identifier {
                        name: bi.name.to_string(),
                        initializer: param.initializer.as_deref(),
                    });
                }
                pattern => {
                    param_specs.push(ParamSpec::Pattern {
                        synthetic_name: format!("@@param_{idx}"),
                        pattern,
                        initializer: param.initializer.as_deref(),
                    });
                }
            }
        }
        if let Some(rest) = &function.params.rest {
            self.push_rest_param(&rest.rest.argument, &mut param_specs)?;
        }
        let body_stmts: &[Statement] = if let Some(body) = &function.body { &body.statements } else { &[] };
        Ok((param_specs, body_stmts))
    }

    /// 编译函数体（函数声明/函数表达式/箭头函数共用），单 pass 完成发码。
    ///
    /// `is_expression_body` 为 true（箭头表达式体）时返回最后一个表达式的值，
    /// 否则返回 undefined。`is_arrow` 控制 super 相关标志的继承：箭头函数词法
    /// 继承外层 super，普通函数重置 super 作用域。
    pub(crate) fn compile_function_body<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, is_arrow: bool, own_strict: bool,
    ) -> Result<IRFunction, String> {
        self.compile_function_body_with_flags(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            is_arrow,
            false,
            false,
            own_strict,
        )
    }

    /// 编译函数体并显式指定生成器标志（`function*` 走此入口）。
    pub(crate) fn compile_generator_body<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx, own_strict: bool,
    ) -> Result<IRFunction, String> {
        self.compile_function_body_with_flags(
            param_specs,
            body_stmts,
            parent_ctx,
            false,
            false,
            true,
            false,
            own_strict,
        )
    }

    /// 编译异步生成器函数体（`async function*` / async 生成器表达式走此入口）：
    /// 同时标记 `is_generator` 与 `is_async`，VM 调用时按异步生成器协议执行
    /// （next 返回 Promise，yield 挂起与 await 挂起共存）。
    pub(crate) fn compile_async_generator_body<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx, own_strict: bool,
    ) -> Result<IRFunction, String> {
        self.compile_function_body_with_flags(param_specs, body_stmts, parent_ctx, false, false, true, true, own_strict)
    }

    /// 编译异步函数体（`async function` / async 箭头走此入口）：`is_async` 使
    /// `assemble_ir` 标记模块，VM 调用时按异步函数协议执行。
    pub(crate) fn compile_async_body<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, is_arrow: bool, own_strict: bool,
    ) -> Result<IRFunction, String> {
        self.compile_function_body_with_flags(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            is_arrow,
            false,
            true,
            own_strict,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_function_body_with_flags<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, is_arrow: bool, is_generator: bool, is_async: bool, own_strict: bool,
    ) -> Result<IRFunction, String> {
        let body_context = if is_arrow {
            FunctionBodyContext::Arrow
        } else {
            FunctionBodyContext::Ordinary
        };
        self.compile_function_body_with_bindings_gen(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            &[],
            body_context,
            is_generator,
            is_async,
            own_strict,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compile_function_body_with_bindings_gen<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, extra_bindings: &[(&str, u32)], body_context: FunctionBodyContext,
        is_generator: bool, is_async: bool, own_strict: bool,
    ) -> Result<IRFunction, String> {
        self.compile_function_body_with_field_hooks_gen(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            extra_bindings,
            body_context,
            None::<fn(&Emitter, &mut CompileCtx) -> Result<(), String>>,
            false,
            &[],
            &[],
            is_generator,
            is_async,
            own_strict,
        )
    }

    /// 预注册 builtin 引用并编译函数体（普通/箭头/类元素方法共用入口）。
    ///
    /// 本函数体内任意位置（表达式、成员对象、调用实参、类字段等）引用的内置全局
    /// 标识符，其寄存器槽都先于任何临时寄存器登记。
    ///
    /// vreg 化后临时值不复用（独立 vreg），但 builtin 槽预注册仍保证分配序稳定：
    /// builtin 槽先于临时值池，嵌套函数继承边界不受扰。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compile_function_body_with_field_hooks<'a, E>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, extra_bindings: &[(&str, u32)], body_context: FunctionBodyContext,
        emit_fields: Option<E>, fields_after_super: bool, extra_capture_exprs: &[&'a Expression<'a>],
        extra_upvalue_names: &[(&str, u8)], own_strict: bool,
    ) -> Result<IRFunction, String>
    where
        E: FnMut(&Emitter, &mut CompileCtx) -> Result<(), String>,
    {
        self.compile_function_body_with_field_hooks_gen(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            extra_bindings,
            body_context,
            emit_fields,
            fields_after_super,
            extra_capture_exprs,
            extra_upvalue_names,
            false,
            false,
            own_strict,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compile_function_body_with_field_hooks_gen<'a, E>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, extra_bindings: &[(&str, u32)], body_context: FunctionBodyContext,
        mut emit_fields: Option<E>, fields_after_super: bool, extra_capture_exprs: &[&'a Expression<'a>],
        extra_upvalue_names: &[(&str, u8)], is_generator: bool, is_async: bool, own_strict: bool,
    ) -> Result<IRFunction, String>
    where
        E: FnMut(&Emitter, &mut CompileCtx) -> Result<(), String>,
    {
        let mut ctx = CompileCtx::new();
        ctx.is_generator = is_generator;
        ctx.is_async = is_async;
        // 函数 strict = 自身 directive ‖ 外层严格（规范不可解除）；
        // 类元素方法/构造器恒 strict（ClassBody 是严格模式代码）。
        ctx.is_strict = own_strict || matches!(body_context, FunctionBodyContext::ClassElement) || parent_ctx.is_strict;

        // 形参名集（含解构叶子与 rest）：块级函数名同名的 web-compat 守卫查此集。
        let mut param_names = HashSet::new();
        for spec in param_specs {
            param_names.insert(spec.register_name().to_string());
            if let ParamSpec::Pattern { pattern, .. } = spec {
                self.collect_binding_pattern_names(pattern, &mut param_names);
            }
        }
        ctx.param_names = param_names;

        // length = 第一个带默认值形参之前的形参数（解构默认与标识符默认同规则）；
        // rest 参数不计入 length（以 0 结尾即止）。
        ctx.function_length = param_specs
            .iter()
            .take_while(|spec| match spec {
                ParamSpec::Identifier { initializer, .. } => initializer.is_none(),
                ParamSpec::Pattern { initializer, .. } => initializer.is_none(),
                ParamSpec::Rest { .. } => false,
            })
            .count() as u32;

        // 继承父内置寄存器映射：子模块寄存器文件中，内置标识符（Math、Object 等）
        // 解析到父预先分配的槽位。
        ctx.scopes.builtin_reg_map = parent_ctx.scopes.builtin_reg_map.clone();
        ctx.scopes.private_name_map = parent_ctx.scopes.private_name_map.clone();
        ctx.scopes.private_element_kinds = parent_ctx.scopes.private_element_kinds.clone();
        ctx.scopes.private_brand_id = parent_ctx.scopes.private_brand_id;
        ctx.scopes.next_private_name_id = parent_ctx.scopes.next_private_name_id;
        // 标签模板 site 序号继承：整棵编译树全局唯一（跨嵌套函数递增），
        // 运行时以 (flat_id, site_no) 缓存模板对象，跨函数不冲突。
        ctx.next_template_site = parent_ctx.next_template_site;

        // 传递 enclosing_this_reg：嵌套箭头函数捕获正确的 `this`。
        ctx.enclosing_this_reg = parent_ctx.enclosing_this_reg;
        // 编码源口径是整棵编译树属性（嵌套函数仍在同一编码源内）。
        ctx.source_encoded = parent_ctx.source_encoded;

        // 箭头函数词法继承 super；类方法体顶层编译也需要类提供的 super 上下文。
        if matches!(body_context, FunctionBodyContext::Arrow | FunctionBodyContext::ClassElement) {
            ctx.in_derived_constructor = parent_ctx.in_derived_constructor;
            ctx.in_instance_method = parent_ctx.in_instance_method;
            ctx.in_static_method = parent_ctx.in_static_method;
        } else {
            ctx.in_derived_constructor = false;
            ctx.in_instance_method = false;
            ctx.in_static_method = false;
        }

        // 继承父全局作用域条目：先前声明的函数名在函数体内可见。
        let mut inherited_reg_start = 1u32.max(ctx.builtin_reg_floor());
        for (name, binding) in &parent_ctx.scopes.symbols.scopes[0].bindings {
            ctx.scopes.symbols.scopes[0].bindings.insert(
                name.clone(),
                Binding {
                    reg: binding.reg,
                    initialized: binding.initialized,
                    is_const: binding.is_const,
                    predeclared: false,
                },
            );
            inherited_reg_start = inherited_reg_start.max(binding.reg.saturating_add(1));
        }
        // 隐式全局登记集合随继承绑定传入：父层未声明名已登记全局作用域，子层解析
        // 命中继承绑定时须同样补全局对象属性写（或严格模式抛错）。读写两侧登记
        // 一并继承——读侧登记的全局槽与写侧同属一个槽，写判定跨嵌套函数一致。
        ctx.implicit_global_writes = parent_ctx.implicit_global_writes.clone();
        ctx.implicit_global_reads = parent_ctx.implicit_global_reads.clone();
        // 顶层已声明 var 名集随作用域继承：嵌套函数裸读/写顶层 var 直连全局对象
        // （经继承的 scope-0 绑定 + 作用域索引判定，不被局部同名遮蔽误判）。
        ctx.global_tier_names = parent_ctx.global_tier_names.clone();
        for (name, reg) in extra_bindings {
            ctx.scopes.symbols.scopes[0].bindings.insert(
                (*name).to_string(),
                Binding {
                    reg: *reg,
                    initialized: true,
                    is_const: true,
                    predeclared: false,
                },
            );
            inherited_reg_start = inherited_reg_start.max(reg.saturating_add(1));
        }
        ctx.reserved_reg_start = inherited_reg_start.max(1);

        // 让 next_reg 与 builtin 槽位对齐，参数在 builtin 槽之后分配。
        ctx.reset_regs();

        let param_base = self.emit_params_prologue(
            param_specs,
            body_stmts,
            parent_ctx,
            &mut ctx,
            body_context,
            extra_capture_exprs,
            extra_upvalue_names,
        )?;
        // 实例字段 computed key 数组所在 upvalue 下标，供字段初始化 emit 定位。
        ctx.field_keys_uv = ctx
            .current_upvalue_captures
            .iter()
            .position(|u| u.name == "@@field_keys")
            .map(|i| i as u8);

        self.predeclare_function_declarations(body_stmts, &mut ctx);

        // 预注册 builtin 引用（先于任何临时寄存器），builtin 槽不与被复用的临时值冲突。
        self.pre_register_builtin_references(body_stmts, &mut ctx);

        // 预声明 `var` 名，使首个 sub-pass 中提升的函数声明能解析其闭包引用的外层 var。
        self.predeclare_var_declarations(body_stmts, &mut ctx);

        // 预声明 body 级 `let`/`const`/`class`（未初始化 TDZ 占位），
        // 使声明点前读取可编译为运行时 ReferenceError。函数体 lexical 声明是
        // 局部绑定，不做受限全局名检查；重复声明错在 emit 期报。
        let _ = self.predeclare_lexical_declarations(body_stmts, &mut ctx, false);

        // 块级函数名 web-compat 外层 var 绑定（sloppy）：块内函数声明名在实例化
        // 期于函数作用域建外层 var 绑定——无同名绑定则新建 var 槽；名在抑制集
        // （形参/词法声明同名，该形退化为纯块作用域）则不建。求值期声明点把
        // 函数对象写回此绑定（见函数声明 emit）。
        if !ctx.is_strict {
            ctx.block_fn_suppressed = self.collect_block_fn_suppressed_names(body_stmts, &ctx.param_names);
            for name in self.collect_block_function_names(body_stmts) {
                if !ctx.block_fn_suppressed.contains(&name) {
                    self.predeclare_var_name(&name, &mut ctx);
                }
            }
        }

        // 生成器：body 起点标记——调用时参数初始化（emit_params_prologue）结束后挂起于此，
        // 参数副作用/异常在 `g()` 调用时刻生效，首次 next() 从这继续执行 body。
        if is_generator {
            ctx.inst(Inst::suspend_body());
        }

        if let Some(emit) = emit_fields.as_mut() {
            if fields_after_super {
                let mut parent_insts = Vec::new();
                let mut parent_label_pos = Vec::new();
                std::mem::swap(&mut ctx.insts, &mut parent_insts);
                std::mem::swap(&mut ctx.labels.label_pos, &mut parent_label_pos);
                emit(self, &mut ctx)?;
                let field_buffer = FieldBuffer {
                    insts: std::mem::take(&mut ctx.insts),
                    labels: std::mem::take(&mut ctx.labels.label_pos)
                        .into_iter()
                        .enumerate()
                        .filter_map(|(id, pos)| pos.map(|p| (id as LabelId, p)))
                        .collect(),
                };
                ctx.insts = parent_insts;
                ctx.labels.label_pos = parent_label_pos;
                ctx.field_buffer = Some(field_buffer);
            } else {
                emit(self, &mut ctx)?;
            }
        }

        // 发 body 语句（先函数声明 hoisting，再其余）。
        let last_result_reg = self.emit_body_stmts(body_stmts, &mut ctx)?;

        // 隐式 RETURN：表达式体返回最后表达式，语句体返回 undefined。
        // 函数尾词法上不在任何循环内，迭代器逃出计数恒 0。
        if is_expression_body {
            if let Some(reg) = last_result_reg {
                ctx.inst(Inst::ret(Operand::Reg(reg), 0, 0));
            } else {
                let undef_idx = ctx.add_constant(Constant::Undefined);
                let undef_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(undef_reg), undef_idx));
                ctx.inst(Inst::ret(Operand::Reg(undef_reg), 0, 0));
            }
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            let undef_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(undef_reg), undef_idx));
            ctx.inst(Inst::ret(Operand::Reg(undef_reg), 0, 0));
        }

        // 调用契约参数段只含固定形参：rest 是函数体内普通变量，不在 VM 实参传递区。
        let fixed_count = param_specs
            .iter()
            .filter(|spec| !matches!(spec, ParamSpec::Rest { .. }))
            .count() as u32;
        let ir = ctx.assemble_ir(
            oxide_ir::ParamLayout {
                base: param_base,
                count: fixed_count,
            },
            Some(parent_ctx),
        );
        Ok(ir)
    }

    /// 参数 prologue：函数作用域 + 参数声明/解构 + 闭包捕获与 upvalue 分析。返回 param_base。
    #[allow(clippy::too_many_arguments)]
    fn emit_params_prologue<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        ctx: &mut CompileCtx, body_context: FunctionBodyContext, extra_capture_exprs: &[&'a Expression<'a>],
        extra_upvalue_names: &[(&str, u8)],
    ) -> Result<u32, String> {
        ctx.push_scope_with_kind(ScopeKind::FunctionScope);
        let param_base = ctx.next_reg;

        // 发参数声明（分配寄存器；分析用 register_name 引用）。
        for spec in param_specs {
            let name = spec.register_name();
            let reg = ctx.alloc_reg();
            ctx.declare_initialized(name, reg, VariableDeclarationKind::Var, false)?;
        }

        // 闭包捕获分析（AST 级，emit 前确定）：须在参数默认值 emit 之前，
        // 否则默认值内嵌套函数（IIFE）编译时父分析为空 → upvalue 捕获丢失。
        let param_names: Vec<&str> = param_specs.iter().map(|s| s.register_name()).collect();
        let mut param_defaults: Vec<&oxide_parser::Expression> = Vec::new();
        for spec in param_specs {
            match spec {
                ParamSpec::Identifier { initializer, .. } => {
                    if let Some(init) = initializer {
                        param_defaults.push(init);
                    }
                }
                ParamSpec::Pattern { pattern, initializer, .. } => {
                    if let Some(init) = initializer {
                        param_defaults.push(init);
                    }
                    // 模式内嵌默认值（`[x = expr]` 的 AssignmentPattern.right）也会被
                    // 内部闭包引用，需纳入捕获分析。
                    self.collect_pattern_default_exprs(pattern, &mut param_defaults);
                }
                ParamSpec::Rest { .. } => {}
            }
        }
        ctx.own_bindings = self.collect_own_binding_names(&param_names, body_stmts);
        for spec in param_specs {
            if let ParamSpec::Pattern { pattern, .. } = spec {
                self.collect_binding_pattern_names(pattern, &mut ctx.own_bindings);
            }
        }

        // 自动声明 arguments 绑定（非箭头函数，且用户未显式声明同名标识符）。
        // 先登记符号并纳入 own_bindings，使嵌套箭头引用 arguments 被识别为本函数
        // 绑定（否则被当自由变量 → 子模块 upvalue 解析错位）。
        let mut arguments_reg = None;
        if !matches!(body_context, FunctionBodyContext::Arrow) && !ctx.own_bindings.contains("arguments") {
            let reg = ctx.alloc_reg();
            ctx.declare_initialized("arguments", reg, VariableDeclarationKind::Var, false)?;
            ctx.own_bindings.insert("arguments".to_string());
            arguments_reg = Some(reg);
        }

        // 字段初始化表达式（值表达式）与参数默认值一并纳入捕获分析。
        let mut capture_exprs: Vec<&oxide_parser::Expression> = param_defaults.clone();
        capture_exprs.extend_from_slice(extra_capture_exprs);
        ctx.captured_bindings = self.collect_captured_bindings(body_stmts, &capture_exprs, &ctx.own_bindings);
        // 自由变量分析：收集 upvalue 捕获（类方法也是普通函数，可捕获外层变量）。
        if matches!(
            body_context,
            FunctionBodyContext::Ordinary | FunctionBodyContext::Arrow | FunctionBodyContext::ClassElement
        ) {
            ctx.current_upvalue_captures = self.collect_upvalue_names(
                body_stmts,
                &capture_exprs,
                &parent_ctx.captured_bindings,
                &parent_ctx.current_upvalue_captures,
                &ctx.own_bindings,
            );
            // 捕获 const 信息快照：父作用域符号表此时完整（预声明已完成），
            // 直接查绑定 is_const（不依赖初始化状态，TDZ 中 const 也须拦截）。
            ctx.upvalue_const_flags = ctx
                .current_upvalue_captures
                .iter()
                .filter(|u| u.parent_uv_idx.is_none())
                .filter(|u| {
                    parent_ctx
                        .scopes
                        .symbols
                        .lookup_any_binding(u.name.as_str())
                        .map(|(b, _)| b.is_const)
                        .unwrap_or(false)
                })
                .map(|u| u.name.clone())
                .collect();
            // 类字段 computed key 数组等合成捕获：直接追加 upvalue（cell_idx 由父分配）。
            for (name, cell_idx) in extra_upvalue_names {
                if !ctx.own_bindings.contains(*name) && !ctx.current_upvalue_captures.iter().any(|u| u.name == *name) {
                    ctx.current_upvalue_captures.push(UpvalueCapture {
                        name: (*name).to_string(),
                        enclosing_reg: 0,
                        cell_idx: *cell_idx,
                        parent_uv_idx: None,
                    });
                }
            }
        }

        // 创建 arguments 对象：指令须在默认参数求值前发出（默认参数可引用 arguments）。
        if let Some(reg) = arguments_reg {
            ctx.inst(Inst::create_arguments(Operand::Reg(reg)));
        }

        for spec in param_specs {
            match spec {
                ParamSpec::Pattern {
                    synthetic_name,
                    pattern,
                    initializer,
                } => {
                    let src_reg = ctx.lookup(synthetic_name)?;
                    let src_reg = if let Some(init) = initializer {
                        self.emit_default_if_undefined(src_reg, init, Some(synthetic_name), ctx)?
                    } else {
                        src_reg
                    };
                    self.emit_binding_pattern(pattern, src_reg, VariableDeclarationKind::Var, false, false, ctx)?;
                }
                ParamSpec::Identifier { name, initializer } => {
                    if let Some(init) = initializer {
                        // 默认参数：实参为 undefined 时用默认值。
                        let reg = ctx.lookup(name)?;
                        self.emit_default_if_undefined(reg, init, Some(name), ctx)?;
                    }
                }
                ParamSpec::Rest { name } => {
                    // rest 数组：从实参区收集固定形参之后的实参，绑定为普通变量。
                    let reg = ctx.lookup(name)?;
                    let fixed_count =
                        param_specs.iter().filter(|s| !matches!(s, ParamSpec::Rest { .. })).count() as u32;
                    ctx.inst(Inst::create_rest_array(Operand::Reg(reg), fixed_count));
                }
            }
        }

        // 被捕获的参数也必须建 cell（MAKE_CELL）：否则子函数经 lazy upvalue 路径读
        // 自身寄存器（依赖调用者寄存器残留），vreg 化/RegAlloc 移动寄存器后读到垃圾。
        // 与 var/let/const 的 MAKE_CELL 语义一致（binding.rs:50）。须在默认值之后
        // （默认值 emit 会读参数寄存器）。
        for spec in param_specs {
            let name = spec.register_name();
            if let Some(&cell_idx) = ctx.captured_bindings.get(name) {
                let reg = ctx.lookup(name)?;
                ctx.inst(Inst::new(
                    OpCode::MAKE_CELL,
                    Operand::Reg(reg),
                    Operand::Imm(cell_idx as u16),
                    Operand::None,
                ));
            }
        }

        // 被捕获的 arguments 绑定同样建 cell（与参数一致，须在默认值之后）。
        if let (Some(reg), Some(&cell_idx)) = (arguments_reg, ctx.captured_bindings.get("arguments")) {
            ctx.inst(Inst::new(
                OpCode::MAKE_CELL,
                Operand::Reg(reg),
                Operand::Imm(cell_idx as u16),
                Operand::None,
            ));
        }

        // var 绑定函数入口实例化：被捕获的 var 名统一 MAKE_CELL(undefined)。
        // 规范上 var 在函数入口即初始化为 undefined（HoistDeclaration），声明语句
        // 只是赋值；否则声明语句前创建的闭包读取占位 cell → TDZ 误报。参数与
        // arguments 已在上方初始化（跳过以免覆盖参数值）；let/const/class 保持
        // TDZ 语义不动。声明语句的 MAKE_CELL 按占位更新语义覆盖此初值。
        let var_names: Vec<String> = self
            .collect_var_binding_names(body_stmts)
            .into_iter()
            .filter(|n| !param_names.contains(&n.as_str()) && n != "arguments")
            .filter(|n| ctx.captured_bindings.contains_key(n))
            .collect();
        if !var_names.is_empty() {
            let undef_reg = self.emit_undefined(ctx);
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

        Ok(param_base)
    }

    /// 双 sub-pass emit body：先函数声明（hoisting），再其余语句。返回最后结果寄存器。
    fn emit_body_stmts(&self, body_stmts: &[Statement], ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let mut last_result_reg = None;
        for stmt in body_stmts {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                if let Some(reg) = self.emit_statement(stmt, ctx)? {
                    last_result_reg = Some(reg);
                }
            }
        }
        for stmt in body_stmts {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                continue;
            }
            if let Some(reg) = self.emit_statement(stmt, ctx)? {
                last_result_reg = Some(reg);
            }
        }
        Ok(last_result_reg)
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
