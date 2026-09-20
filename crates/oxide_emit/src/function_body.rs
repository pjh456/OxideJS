//! 函数体编译域：`FunctionBodyContext` / `ParamSpec` 类型与函数体编译链
//! （参数 prologue、参数默认值与闭包捕获分析、双 sub-pass 语句发射）。
//!
//! 两类型经 lib.rs 重导出供各语法域构造实参；编译链为 `Emitter` 方法，
//! 捕获分析经自由函数调用 capture 族，与所在文件无关。

use std::collections::{BTreeMap, HashSet};

use oxide_bytecode::module::{Constant, UpvalueCapture};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::{LabelId, Operand};
use oxide_ir::IRFunction;
use oxide_parser::{Expression, Statement, VariableDeclarationKind};

use crate::capture::{
    collect_binding_pattern_names, collect_captured_bindings, collect_direct_lexical_names, collect_own_binding_names,
    collect_upvalue_names, collect_var_binding_names,
};
use crate::compile_ctx::{CompileCtx, FieldBuffer};
use crate::symbol_table::{Binding, ScopeKind};
use crate::Emitter;

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
                collect_binding_pattern_names(pattern, &mut param_names);
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
        // eval 起源随作用域继承：eval 顶层 var/函数名在嵌套函数内同样可删（物化
        // c:true 全局属性），delete 分类判定读此标志。
        ctx.is_eval_origin = parent_ctx.is_eval_origin;
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
        self.predeclare_labeled_function_declarations(body_stmts, &mut ctx);

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

        // `var` 与块级函数外层 var 绑定入口实例化（非捕获名）：预声明只登记槽位、
        // 不发射定义指令，声明点对已绑定名又跳过 undefined 写，缺失入口写会让首次
        // 写入前的读取取到调用方遗留的寄存器值。捕获名已由参数 prologue 的
        // MAKE_CELL(undefined) 实例化，此处只补未捕获名。
        self.instantiate_var_bindings(body_stmts, &mut ctx);

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

    /// 规范 `argumentsObjNeeded` 的引擎判据：函数作用域是否声明 `arguments`
    /// 从而抑制自动 arguments 对象。
    ///
    /// 规范 FunctionDeclarationInstantiation 仅在箭头函数、形参名含
    /// `arguments`、或（无参数表达式时）函数体**顶层**函数/词法声明含
    /// `arguments` 时不建对象；嵌套块、循环、`switch`/`try` 内的同名声明属块
    /// 作用域，不构成抑制。
    ///
    /// # 边界与前提
    /// - `param_names` 须含解构形参叶名（规范 `paramNames`）。
    /// - `has_param_exprs` 为形参默认值存在性（规范 `ContainsExpression`）：
    ///   为 true 时顶层函数声明不抑制，对象须在参数求值期已可读。
    /// - 函数体顶层词法声明同名无条件抑制：引擎同一作用域无法同时承载自动
    ///   绑定与顶层词法绑定。
    /// - `var arguments` 不在规范 `funcNames` 内，不抑制。
    fn arguments_obj_needed(
        &self, body_stmts: &[Statement], param_names: &HashSet<String>, has_param_exprs: bool,
    ) -> bool {
        if param_names.contains("arguments") {
            return false;
        }
        for stmt in body_stmts {
            match stmt {
                // 顶层 let/const/class 与自动绑定同作用域冲突，无条件抑制。
                Statement::VariableDeclaration(vd) if !matches!(vd.kind, VariableDeclarationKind::Var) => {
                    let mut names = HashSet::new();
                    for d in &vd.declarations {
                        collect_binding_pattern_names(&d.id, &mut names);
                    }
                    if names.contains("arguments") {
                        return false;
                    }
                }
                Statement::ClassDeclaration(cd) => {
                    if cd.id.as_ref().is_some_and(|id| id.name == "arguments") {
                        return false;
                    }
                }
                // 顶层函数声明仅无参数默认值时抑制。
                Statement::FunctionDeclaration(fd) => {
                    if !has_param_exprs && fd.id.as_ref().is_some_and(|id| id.name == "arguments") {
                        return false;
                    }
                }
                // Annex B.3.2 标签链直接包裹的函数声明与直接子函数声明同面。
                _ => {
                    if !has_param_exprs
                        && Self::labeled_function_decl(stmt)
                            .is_some_and(|fd| fd.id.as_ref().is_some_and(|id| id.name == "arguments"))
                    {
                        return false;
                    }
                }
            }
        }
        true
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
        ctx.own_bindings = collect_own_binding_names(&param_names, body_stmts);
        for spec in param_specs {
            if let ParamSpec::Pattern { pattern, .. } = spec {
                collect_binding_pattern_names(pattern, &mut ctx.own_bindings);
            }
        }

        // 自动声明 arguments 绑定（非箭头函数，且函数作用域未声明同名标识符）。
        // 先登记符号并纳入 own_bindings，使嵌套箭头引用 arguments 被识别为本函数
        // 绑定（否则被当自由变量 → 子模块 upvalue 解析错位）。
        let mut arguments_reg = None;
        if !matches!(body_context, FunctionBodyContext::Arrow)
            && self.arguments_obj_needed(body_stmts, &ctx.param_names, !param_defaults.is_empty())
        {
            let reg = ctx.alloc_reg();
            ctx.declare_initialized("arguments", reg, VariableDeclarationKind::Var, false)?;
            ctx.own_bindings.insert("arguments".to_string());
            arguments_reg = Some(reg);
        }

        // 字段初始化表达式（值表达式）与参数默认值一并纳入捕获分析。
        let mut capture_exprs: Vec<&oxide_parser::Expression> = param_defaults.clone();
        capture_exprs.extend_from_slice(extra_capture_exprs);
        ctx.captured_bindings = collect_captured_bindings(body_stmts, &capture_exprs, &ctx.own_bindings);
        // 自由变量分析：收集 upvalue 捕获（类方法也是普通函数，可捕获外层变量）。
        if matches!(
            body_context,
            FunctionBodyContext::Ordinary | FunctionBodyContext::Arrow | FunctionBodyContext::ClassElement
        ) {
            // 只有创建点父作用域链内可解析的父捕获名才可能成为子函数 upvalue：
            // 捕获集是函数级名字并集，块级 let/const 在块退出后 cell 仍存留但名字
            // 已不可解析，块外嵌套函数按名命中会错误读取已失效的 cell。父自身
            // upvalue 代表的祖父绑定在父函数体内恒可见，不参与过滤。
            // C 风格 for 头 let/const 名在 init 内尚未 declare，但同头声明会为其
            // 建 cell，前向捕获须按父捕获集保留（见 `pending_for_head_names`）。
            let visible_parent_captured: BTreeMap<String, u8> = parent_ctx
                .captured_bindings
                .iter()
                .filter(|(n, _)| {
                    parent_ctx.visible_binding_reg(n.as_str()).is_some()
                        || parent_ctx.pending_for_head_names.contains(n.as_str())
                })
                .map(|(n, &i)| (n.clone(), i))
                .collect();
            ctx.current_upvalue_captures = collect_upvalue_names(
                body_stmts,
                &capture_exprs,
                &visible_parent_captured,
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

        // var 与块级函数外层 var 绑定函数入口实例化：被捕获名统一
        // MAKE_CELL(undefined)。规范上 var 在函数入口即初始化为 undefined
        // （HoistDeclaration），sloppy 下块级函数名同建外层 var 绑定；声明语句只是
        // 赋值。缺此实例化时，声明点前创建的闭包读取占位 cell → TDZ 误报，非捕获面
        // 则取调用方寄存器残留。参数与 arguments 已在上方初始化（跳过以免覆盖参数
        // 值）；let/const/class 保持 TDZ 语义不动。声明语句的 MAKE_CELL 按占位更新
        // 语义覆盖此初值。
        let mut captured_entry_names = collect_var_binding_names(body_stmts);
        // 块级函数外层 var 名与 instantiate_var_bindings 同源并入；抑制名（形参/
        // 词法声明同名）无外层绑定，不并入。抑制集在此独立计算：本循环早于函数体
        // 的 block_fn_suppressed 构建。
        if !ctx.is_strict {
            let suppressed = self.collect_block_fn_suppressed_names(body_stmts, &ctx.param_names);
            for name in self.collect_block_function_names(body_stmts) {
                if !suppressed.contains(&name) {
                    captured_entry_names.insert(name);
                }
            }
        }
        let mut var_names: Vec<String> = captured_entry_names
            .into_iter()
            .filter(|n| !param_names.contains(&n.as_str()) && n != "arguments")
            .filter(|n| ctx.captured_bindings.contains_key(n))
            .collect();
        // HashSet 迭代序带随机种子，排序后入口 MAKE_CELL 发射序跨进程稳定。
        var_names.sort();
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

        // 被捕获函数作用域词法（let/const/class）入口 TDZ 占位 cell：函数声明
        // hoisting 先于声明语句执行时，闭包指向此未初始化 cell，声明前读写经
        // 运行时抛真 TDZ；声明语句的 MAKE_CELL 按占位更新语义原位翻转。名集只取
        // 直接子级（块级名不提升到函数作用域，与块内重执行 cell 族零交集），
        // 排序保证发射序跨进程稳定。
        let mut lex_names: Vec<String> = collect_direct_lexical_names(body_stmts)
            .into_iter()
            .filter(|n| !param_names.contains(&n.as_str()) && n != "arguments")
            .filter(|n| ctx.captured_bindings.contains_key(n))
            .collect();
        lex_names.sort();
        if !lex_names.is_empty() {
            let undef_reg = self.emit_undefined(ctx);
            for name in lex_names {
                if let Some(&cell_idx) = ctx.captured_bindings.get(&name) {
                    // 未初始化标志折入 16 位立即数高字节（0x0100），dispatch 侧
                    // 按字节拆回两字段。
                    ctx.inst(Inst::new(
                        OpCode::MAKE_CELL,
                        Operand::Reg(undef_reg),
                        Operand::Imm(cell_idx as u16 | 0x0100),
                        Operand::None,
                    ));
                }
            }
        }

        Ok(param_base)
    }

    /// 非捕获 `var` 名与块级函数外层 var 名的函数入口实例化：把每个未捕获槽在入口
    /// 写为 undefined。
    ///
    /// # 边界与前提
    /// - 须在 `var` 预声明与块级函数外层 var 预声明之后、body 语句发射之前调用：
    ///   预声明只登记槽位不发定义指令，声明语句对已绑定名又跳过 undefined 写，缺失
    ///   入口写会让首次写入前的读取取到调用方遗留的寄存器值。
    /// - 形参与 `arguments` 由参数 prologue 写入实参，跳过以免覆盖。
    /// - 捕获名由参数 prologue 的 MAKE_CELL(undefined) 实例化；STORE_VAR 不更新
    ///   cell，跳过以免冗余。
    /// - 块级函数名仅在 sloppy 模式并入：strict 下块函数是块级词法绑定，不建外层
    ///   var；被形参/词法同名抑制的名字无外层绑定，不并入。直接子标签函数声明由
    ///   函数入口单独物化闭包，不并入。
    /// - 符号表查不到的名字（如解构 pattern 叶名未被预声明）跳过，不报错。
    fn instantiate_var_bindings(&self, body_stmts: &[Statement], ctx: &mut CompileCtx) {
        let mut name_set = collect_var_binding_names(body_stmts);

        // 块级函数外层 var 绑定同样只预声明不发射定义指令，缺失入口写会让块前读取
        // 取到调用方残留；入口写须并入（strict 与抑制名除外）。
        if !ctx.is_strict {
            // 直接子标签函数声明在函数入口首 sub-pass 物化闭包，声明点前读即为函数；
            // 此处按名排除只省一次冗余的非捕获 undefined 写，非正确性必需。
            let labeled_hoisted: HashSet<&str> = body_stmts
                .iter()
                .filter_map(|stmt| Self::labeled_function_decl(stmt))
                .filter_map(|fd| fd.id.as_ref())
                .map(|id| id.name.as_str())
                .collect();
            for name in self.collect_block_function_names(body_stmts) {
                if !ctx.block_fn_suppressed.contains(&name) && !labeled_hoisted.contains(name.as_str()) {
                    name_set.insert(name);
                }
            }
        }

        let mut names: Vec<String> = name_set
            .into_iter()
            .filter(|n| !ctx.param_names.contains(n) && n != "arguments")
            .filter(|n| !ctx.captured_bindings.contains_key(n))
            .collect();
        if names.is_empty() {
            return;
        }
        // 按名排序发射：哈希集迭代序随进程变化，排序使字节码确定。
        names.sort();
        let undef_reg = self.emit_undefined(ctx);
        for name in names {
            if let Some(reg) = ctx.scopes.symbols.lookup_any(&name) {
                ctx.inst(Inst::new(OpCode::STORE_VAR, Operand::Reg(reg), Operand::Reg(undef_reg), Operand::Imm(0)));
            }
        }
    }

    /// 标签链直接包裹的函数声明（Annex B.3.2 Labelled Function Declarations）：
    /// 展开任意层 `LabeledStatement` 后取内层函数声明。至少经过一层标签才返回
    /// `Some`；直接子函数声明由调用方单独匹配。
    ///
    /// # 边界与前提
    /// - 标签体为块/分支等其它语句时返回 `None`（归块面或普通标签发射处理）。
    /// - 不递归进函数体：嵌套函数是独立编译单元。
    pub(crate) fn labeled_function_decl<'a, 'b>(stmt: &'b Statement<'a>) -> Option<&'b oxide_parser::Function<'a>> {
        let mut cur: &'b Statement<'a> = match stmt {
            Statement::LabeledStatement(labeled) => &labeled.body,
            _ => return None,
        };
        loop {
            match cur {
                Statement::LabeledStatement(labeled) => cur = &labeled.body,
                Statement::FunctionDeclaration(fd) => return Some(fd),
                _ => return None,
            }
        }
    }

    /// 函数体内参与入口提升的函数声明语句：直接子函数声明，或标签链直接包裹的
    /// 函数声明（sloppy 下标签不改变执行流，二者同等在函数入口物化闭包）。
    fn is_body_hoisted_function_decl(stmt: &Statement) -> bool {
        matches!(stmt, Statement::FunctionDeclaration(_)) || Self::labeled_function_decl(stmt).is_some()
    }

    /// 双 sub-pass emit body：先函数声明（hoisting），再其余语句。返回最后结果寄存器。
    ///
    /// 首 sub-pass 覆盖直接子函数声明与标签链直接包裹的函数声明：二者均在函数
    /// 入口物化闭包，次 sub-pass 跳过声明语句本身，声明点不重编以保函数对象同一性。
    fn emit_body_stmts(&self, body_stmts: &[Statement], ctx: &mut CompileCtx) -> Result<Option<u32>, String> {
        let mut last_result_reg = None;
        for stmt in body_stmts {
            if Self::is_body_hoisted_function_decl(stmt) {
                if let Some(reg) = self.emit_statement(stmt, ctx)? {
                    last_result_reg = Some(reg);
                }
            }
        }
        for stmt in body_stmts {
            if Self::is_body_hoisted_function_decl(stmt) {
                continue;
            }
            if let Some(reg) = self.emit_statement(stmt, ctx)? {
                last_result_reg = Some(reg);
            }
        }
        Ok(last_result_reg)
    }
}
