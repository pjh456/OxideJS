//! emit：AST → IR 代码生成（parse → IR → bytecode 中段）。
//!
//! `Emitter` 提供各语法域的 emit_* 方法；核心状态集中在 `CompileCtx`：
//! 执行流字段（insts/registers/constants/labels）平铺其上，标识符绑定与
//! 闭包捕获分别下沉到 `SymbolTable` / `captured_bindings`。产出分域组合的
//! `IRFunction`，由 `oxide_ir::lower` 降为 bytecode。

use std::collections::{BTreeMap, HashMap, HashSet};

use oxide_bytecode::module::UpvalueCapture;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::{LabelId, Operand};
use oxide_ir::IRFunction;

use crate::emit_ctx::{LabelCtx, ScopeCtx};
use crate::symbol_table::{Binding, ScopeKind, SymbolTable};

/// 常量池项（bytecode module 类型）re-export，供调用方构造常量。
pub use oxide_bytecode::module::Constant;
/// 变量声明种类（var/let/const），re-export 自 parser。
pub use oxide_parser::VariableDeclarationKind;
/// AST 语法树节点与运算符类型，re-export 自 parser。
pub use oxide_parser::{AssignmentOperator, BinaryOperator, Expression, Statement, UnaryOperator};

/// 编译入口（marker 类型）。方法按语法域组织在 `impl Emitter` 中。
pub struct Emitter;

/// 判断 f64 是否为整数值且在 i32 范围内（整数常量编码用）。
pub fn is_int_literal(value: f64) -> bool {
    value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64
}

/// 判断表达式是否无副作用（字面量/标识符/纯二元运算等）。
/// 用于可丢弃值的优化路径。
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
            Expression::BinaryExpression(bin) => {
                stack.push(&bin.left);
                stack.push(&bin.right);
            }
            Expression::UnaryExpression(un) if !matches!(un.operator, UnaryOperator::Delete) => {
                stack.push(&un.argument);
            }
            _ => return false,
        }
    }
    true
}

const BUILTIN_GLOBALS: &[&str] = &[
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
    "Math",
    "JSON",
    "Promise",
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
];

pub(crate) struct FieldBuffer {
    pub(crate) insts: Vec<Inst>,
    pub(crate) labels: Vec<(LabelId, usize)>,
}

/// 标签语句作用域：编译期内登记 `break label` / `continue label` 的跳转目标。
/// `continue_label` 仅在标签直接包裹迭代语句时存在。
#[derive(Debug, Clone)]
pub struct LabelScope {
    pub(crate) name: String,
    pub(crate) break_label: LabelId,
    pub(crate) continue_label: Option<LabelId>,
}

/// 单函数编译上下文：执行流 + 作用域 + 闭包捕获的聚合状态。
/// 指令、常量池、寄存器分配平铺于此，作用域/绑定见 `ScopeCtx`，跳转见 `LabelCtx`。
pub struct CompileCtx {
    pub(crate) insts: Vec<Inst>,
    pub(crate) constants: Vec<Constant>,
    constant_map: HashMap<ConstantKey, u16>,
    next_reg: u32,
    pub(crate) max_regs: u32,
    reserved_reg_start: u32,
    pub(crate) labels: LabelCtx,
    pub(crate) scopes: ScopeCtx,
    pub(crate) nested: Vec<IRFunction>,
    /// 外层函数上下文中持有 `this` 的寄存器。
    /// 箭头函数用它捕获词法 `this`；顶层初始化为 254（约定 this 寄存器）。
    pub(crate) enclosing_this_reg: u8,
    pub(crate) in_derived_constructor: bool,
    pub(crate) in_instance_method: bool,
    pub(crate) in_static_method: bool,
    pub(crate) static_block_this_reg: Option<u8>,
    pub(crate) field_buffer: Option<FieldBuffer>,
    pub(crate) current_upvalue_captures: Vec<UpvalueCapture>,
    /// 本函数作用域声明的绑定名（参数 + 变量/函数声明，AST 收集，emit 前确定）。
    pub(crate) own_bindings: HashSet<String>,
    /// 本函数被嵌套函数捕获的绑定名 → cell_idx（名字排序分配，稳定跨 run）。
    /// 捕获判断（MAKE_CELL / CELL_GET / CELL_SET）与子函数 upvalue cell_idx 统一查此映射，
    /// 消除符号表时序依赖与 cell 索引错位。
    pub(crate) captured_bindings: BTreeMap<String, u8>,
    pub(crate) const_overflow: bool,
}

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

/// 参数规格：普通形参为标识符（可带默认值 initializer），解构形参用合成名 + 原始 pattern。
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
}

impl ParamSpec<'_> {
    pub(crate) fn register_name(&self) -> &str {
        match self {
            Self::Identifier { name, .. } => name,
            Self::Pattern { synthetic_name, .. } => synthetic_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ConstantKey {
    Number(u64),
    Int(i32),
    String(String),
    Boolean(bool),
    Null,
    Undefined,
}

impl CompileCtx {
    pub(crate) fn new() -> Self {
        Self {
            insts: Vec::new(),
            constants: Vec::new(),
            constant_map: HashMap::new(),
            next_reg: 1,
            max_regs: 1,
            reserved_reg_start: 1,
            labels: LabelCtx {
                label_pos: Vec::new(),
                loop_stack: Vec::new(),
                switch_stack: Vec::new(),
                label_scopes: Vec::new(),
                pending_loop_labels: Vec::new(),
                label_counter: 0,
            },
            scopes: ScopeCtx {
                symbols: SymbolTable::new(),
                builtin_reg_map: Vec::new(),
                private_name_map: Vec::new(),
                next_private_name_id: 1,
            },
            nested: Vec::new(),
            enclosing_this_reg: 254, // conventional this register at top level
            in_derived_constructor: false,
            in_instance_method: false,
            in_static_method: false,
            static_block_this_reg: None,
            field_buffer: None,
            current_upvalue_captures: Vec::new(),
            own_bindings: HashSet::new(),
            captured_bindings: BTreeMap::new(),
            const_overflow: false,
        }
    }

    pub(crate) fn inst(&mut self, inst: Inst) {
        self.insts.push(inst);
    }

    pub(crate) fn alloc_reg(&mut self) -> u32 {
        let r = self.next_reg;
        // vreg 化：寄存器号无上限，RegAlloc 阶段负责压缩到物理域（≤253）。
        // 254/255 是 VM 保留的 this/new.target，vreg 世界允许虚拟号越过它们，
        // 只有 RegAlloc 完成映射后 lower 的物理域检查才相关。
        self.next_reg += 1;
        if self.next_reg > self.max_regs {
            self.max_regs = self.next_reg;
        }
        r
    }

    pub(crate) fn reset_regs(&mut self) {
        self.next_reg = self.builtin_reg_floor().max(self.reserved_reg_start);
        self.labels.label_counter = 0;
    }

    pub(crate) fn reserve_reg(&mut self, reg: u32) {
        let next = reg.wrapping_add(1);
        if self.next_reg <= reg {
            self.next_reg = next;
        }
        if self.max_regs < next {
            self.max_regs = next;
        }
    }

    pub(crate) fn add_constant(&mut self, c: Constant) -> u16 {
        if let Some(key) = ConstantKey::from_constant(&c) {
            if let Some(&idx) = self.constant_map.get(&key) {
                return idx;
            }

            if self.constants.len() >= u16::MAX as usize {
                self.const_overflow = true;
                return u16::MAX;
            }
            let idx = self.constants.len() as u16;
            self.constants.push(c);
            self.constant_map.insert(key, idx);
            return idx;
        }

        let idx = self.constants.len();
        if idx >= u16::MAX as usize {
            self.const_overflow = true;
            return u16::MAX;
        }
        self.constants.push(c);
        idx as u16
    }

    pub(crate) fn push_scope(&mut self) {
        self.scopes.symbols.push_scope();
    }

    pub(crate) fn pop_scope(&mut self) {
        self.scopes.symbols.pop_scope();
    }

    pub(crate) fn declare(
        &mut self, name: &str, reg: u32, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.scopes.symbols.declare(name, reg, kind, is_const)
    }

    pub(crate) fn declare_initialized(
        &mut self, name: &str, reg: u32, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.scopes.symbols.declare_initialized(name, reg, kind, is_const)
    }

    pub(crate) fn push_scope_with_kind(&mut self, kind: ScopeKind) {
        self.scopes.symbols.push_scope_with_kind(kind);
    }

    pub(crate) fn lookup(&self, name: &str) -> Result<u32, String> {
        self.scopes.symbols.lookup(name)
    }

    pub(crate) fn lookup_or_builtin(&mut self, name: &str) -> Result<u32, String> {
        match self.scopes.symbols.lookup(name) {
            Ok(reg) => Ok(reg),
            Err(err) if Self::is_known_builtin(name) && err.contains("is not defined") => {
                let reg = self.alloc_reg();
                self.scopes.symbols.pre_register_global(name, reg);
                self.scopes.builtin_reg_map.push((name.to_string(), reg));
                Ok(reg)
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) fn lookup_or_global(&mut self, name: &str) -> u32 {
        if let Some(reg) = self.scopes.symbols.lookup_any(name) {
            return reg;
        }
        let reg = self.alloc_reg();
        self.scopes.symbols.lookup_or_global(name, reg)
    }

    pub(crate) fn lookup_const_flag(&self, name: &str) -> bool {
        self.scopes.symbols.lookup_is_const(name)
    }

    pub(crate) fn init_var(&mut self, name: &str) {
        self.scopes.symbols.init_var(name);
    }

    pub(crate) fn next_label_id(&mut self) -> u32 {
        let id = self.labels.label_counter;
        self.labels.label_counter += 1;
        id
    }

    pub(crate) fn push_loop(&mut self, break_label: LabelId, continue_label: LabelId) {
        self.labels.loop_stack.push((break_label, continue_label));
    }

    pub(crate) fn pop_loop(&mut self) {
        self.labels.loop_stack.pop();
    }

    pub(crate) fn current_loop(&self) -> Option<&(LabelId, LabelId)> {
        self.labels.loop_stack.last()
    }

    pub(crate) fn push_switch(&mut self, break_label: LabelId) {
        self.labels.switch_stack.push(break_label);
    }

    pub(crate) fn pop_switch(&mut self) {
        self.labels.switch_stack.pop();
    }

    pub(crate) fn current_switch(&self) -> Option<&LabelId> {
        self.labels.switch_stack.last()
    }

    pub(crate) fn push_label_scope(
        &mut self, name: &str, break_label: LabelId, continue_label: Option<LabelId>,
    ) -> Result<(), String> {
        if self.labels.label_scopes.iter().any(|s| s.name == name) {
            return Err(format!("SyntaxError: Label '{name}' has already been declared"));
        }
        self.labels.label_scopes.push(LabelScope {
            name: name.to_string(),
            break_label,
            continue_label,
        });
        Ok(())
    }

    pub(crate) fn pop_label_scope(&mut self) {
        self.labels.label_scopes.pop();
    }

    pub(crate) fn find_label(&self, name: &str) -> Option<&LabelScope> {
        self.labels.label_scopes.iter().rev().find(|s| s.name == name)
    }

    /// 登记一个待绑定标签名：该标签将作为下一个 emit 的循环（标签语句体）的
    /// continue 目标。活动集合与待绑定集合中出现重名报错。
    pub(crate) fn queue_loop_label(&mut self, name: &str) -> Result<(), String> {
        if self.labels.label_scopes.iter().any(|s| s.name == name)
            || self.labels.pending_loop_labels.iter().any(|n| n == name)
        {
            return Err(format!("SyntaxError: Label '{name}' has already been declared"));
        }
        self.labels.pending_loop_labels.push(name.to_string());
        Ok(())
    }

    /// 把待绑定标签名落地为活动标签作用域，绑定到本次循环的 break/continue 目标。
    /// 返回压入的作用域个数（供事后对称弹出）。
    pub(crate) fn take_pending_loop_labels(&mut self, break_label: LabelId, continue_label: LabelId) -> usize {
        let names = std::mem::take(&mut self.labels.pending_loop_labels);
        let count = names.len();
        for name in names {
            self.labels.label_scopes.push(LabelScope {
                name,
                break_label,
                continue_label: Some(continue_label),
            });
        }
        count
    }

    pub(crate) fn pop_label_scopes(&mut self, n: usize) {
        for _ in 0..n {
            self.labels.label_scopes.pop();
        }
    }

    pub(crate) fn is_builtin(&self, name: &str) -> bool {
        self.scopes.builtin_reg_map.iter().any(|(n, _)| n == name)
    }

    pub(crate) fn is_known_builtin(name: &str) -> bool {
        BUILTIN_GLOBALS.contains(&name)
    }

    fn builtin_reg_floor(&self) -> u32 {
        self.scopes
            .builtin_reg_map
            .iter()
            .map(|(_, reg)| reg.saturating_add(1))
            .max()
            .unwrap_or(0)
    }

    pub(crate) fn pre_register_builtins(&mut self) {
        // builtin 全局由 lookup_or_builtin() 惰性解析。保留此钩子维持编译管线形态，
        // 避免在每个模块预留约 60 个寄存器槽。
    }

    /// 组装 IRFunction（两出口共用），take 走编译产物状态。
    /// `parent_ctx` 用于补全 upvalue_captures 的 enclosing_reg（父符号表在父 emit 完成后完整）。
    fn assemble_ir(&mut self, param_layout: oxide_ir::ParamLayout, parent_ctx: Option<&CompileCtx>) -> IRFunction {
        let upvalue_captures = self
            .current_upvalue_captures
            .iter()
            .map(|u| {
                let enclosing_reg = parent_ctx
                    .and_then(|p| p.scopes.symbols.lookup_any(u.name.as_str()))
                    .unwrap_or(0);
                UpvalueCapture {
                    name: u.name.clone(),
                    enclosing_reg,
                    cell_idx: u.cell_idx,
                    parent_uv_idx: u.parent_uv_idx,
                }
            })
            .collect();
        IRFunction {
            insts: std::mem::take(&mut self.insts),
            label_pos: std::mem::take(&mut self.labels.label_pos),
            label_count: self.labels.label_counter,
            constants: std::mem::take(&mut self.constants),
            param_layout,
            builtin_reg_map: std::mem::take(&mut self.scopes.builtin_reg_map),
            upvalue_captures,
            cells_needed: self.captured_bindings.len() as u8,
            n_registers: self.max_regs,
            is_arrow: false,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
            captured_this_const_idx: 0,
            function_name: None,
            is_top_level: parent_ctx.is_none(),
            const_overflow: self.const_overflow,
            nested: std::mem::take(&mut self.nested),
        }
    }
}

impl ConstantKey {
    fn from_constant(value: &Constant) -> Option<Self> {
        match value {
            Constant::Number(v) => Some(Self::Number(v.to_bits())),
            Constant::Int(v) => Some(Self::Int(*v)),
            Constant::String(v) => Some(Self::String(v.clone())),
            Constant::Boolean(v) => Some(Self::Boolean(*v)),
            Constant::Null => Some(Self::Null),
            Constant::Undefined => Some(Self::Undefined),
        }
    }
}

impl Emitter {
    /// 构造空 `Emitter`（无内部状态，所有状态在 `CompileCtx` 中）。
    pub fn new() -> Self {
        Self
    }

    /// 遍历解构 pattern 收集内嵌默认值表达式（AssignmentPattern.right）。
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
                for elem in &ap.elements {
                    if let Some(p) = elem {
                        self.collect_pattern_default_exprs(p, out);
                    }
                }
                if let Some(rest) = &ap.rest {
                    self.collect_pattern_default_exprs(&rest.argument, out);
                }
            }
            BindingPattern::ObjectPattern(op) => {
                for prop in &op.properties {
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
        is_expression_body: bool, is_arrow: bool,
    ) -> Result<IRFunction, String> {
        let body_context = if is_arrow {
            FunctionBodyContext::Arrow
        } else {
            FunctionBodyContext::Ordinary
        };
        self.compile_function_body_with_bindings(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            &[],
            body_context,
        )
    }

    pub(crate) fn compile_function_body_with_bindings<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, extra_bindings: &[(&str, u32)], body_context: FunctionBodyContext,
    ) -> Result<IRFunction, String> {
        self.compile_function_body_with_field_hooks(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            extra_bindings,
            body_context,
            None::<fn(&Emitter, &mut CompileCtx) -> Result<(), String>>,
            false,
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
        mut emit_fields: Option<E>, fields_after_super: bool,
    ) -> Result<IRFunction, String>
    where
        E: FnMut(&Emitter, &mut CompileCtx) -> Result<(), String>,
    {
        let mut ctx = CompileCtx::new();

        // 继承父内置寄存器映射：子模块寄存器文件中，内置标识符（Math、Object 等）
        // 解析到父预先分配的槽位。
        ctx.scopes.builtin_reg_map = parent_ctx.scopes.builtin_reg_map.clone();
        ctx.scopes.private_name_map = parent_ctx.scopes.private_name_map.clone();
        ctx.scopes.next_private_name_id = parent_ctx.scopes.next_private_name_id;

        // 传递 enclosing_this_reg：嵌套箭头函数捕获正确的 `this`。
        ctx.enclosing_this_reg = parent_ctx.enclosing_this_reg;

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
                },
            );
            inherited_reg_start = inherited_reg_start.max(binding.reg.saturating_add(1));
        }
        for (name, reg) in extra_bindings {
            ctx.scopes.symbols.scopes[0].bindings.insert(
                (*name).to_string(),
                Binding {
                    reg: *reg,
                    initialized: true,
                    is_const: true,
                },
            );
            inherited_reg_start = inherited_reg_start.max(reg.saturating_add(1));
        }
        ctx.reserved_reg_start = inherited_reg_start.max(1);

        // 让 next_reg 与 builtin 槽位对齐，参数在 builtin 槽之后分配。
        ctx.reset_regs();

        let param_base = self.emit_params_prologue(param_specs, body_stmts, parent_ctx, &mut ctx, body_context)?;

        self.predeclare_function_declarations(body_stmts, &mut ctx);

        // 预注册 builtin 引用（先于任何临时寄存器），builtin 槽不与被复用的临时值冲突。
        self.pre_register_builtin_references(body_stmts, &mut ctx);

        // 预声明 `var` 名，使首个 sub-pass 中提升的函数声明能解析其闭包引用的外层 var。
        self.predeclare_var_declarations(body_stmts, &mut ctx);

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
        if is_expression_body {
            if let Some(reg) = last_result_reg {
                ctx.inst(Inst::new(OpCode::RETURN, Operand::Reg(reg), Operand::None, Operand::None));
            } else {
                let undef_idx = ctx.add_constant(Constant::Undefined);
                let undef_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(undef_reg), undef_idx));
                ctx.inst(Inst::new(OpCode::RETURN, Operand::Reg(undef_reg), Operand::None, Operand::None));
            }
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            let undef_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(undef_reg), undef_idx));
            ctx.inst(Inst::new(OpCode::RETURN, Operand::Reg(undef_reg), Operand::None, Operand::None));
        }

        let ir = ctx.assemble_ir(
            oxide_ir::ParamLayout {
                base: param_base,
                count: param_specs.len() as u32,
            },
            Some(parent_ctx),
        );
        Ok(ir)
    }

    /// 参数 prologue：函数作用域 + 参数声明/解构 + 闭包捕获与 upvalue 分析。返回 param_base。
    fn emit_params_prologue<'a>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        ctx: &mut CompileCtx, body_context: FunctionBodyContext,
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
            }
        }
        ctx.own_bindings = self.collect_own_binding_names(&param_names, body_stmts);
        ctx.captured_bindings = self.collect_captured_bindings(body_stmts, &param_defaults, &ctx.own_bindings);
        // 自由变量分析：收集 upvalue 捕获（类方法也是普通函数，可捕获外层变量）。
        if matches!(
            body_context,
            FunctionBodyContext::Ordinary | FunctionBodyContext::Arrow | FunctionBodyContext::ClassElement
        ) {
            ctx.current_upvalue_captures = self.collect_upvalue_names(
                body_stmts,
                &param_defaults,
                &parent_ctx.captured_bindings,
                &parent_ctx.current_upvalue_captures,
                &ctx.own_bindings,
            );
        }

        for spec in param_specs {
            match spec {
                ParamSpec::Pattern { synthetic_name, pattern, initializer } => {
                    let src_reg = ctx.lookup(synthetic_name)?;
                    let src_reg = if let Some(init) = initializer {
                        self.emit_default_if_undefined(src_reg, init, Some(synthetic_name), ctx)?
                    } else {
                        src_reg
                    };
                    self.emit_binding_pattern(pattern, src_reg, VariableDeclarationKind::Var, false, ctx)?;
                }
                ParamSpec::Identifier { name, initializer } => {
                    if let Some(init) = initializer {
                        // 默认参数：实参为 undefined 时用默认值。
                        let reg = ctx.lookup(name)?;
                        self.emit_default_if_undefined(reg, init, Some(name), ctx)?;
                    }
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
            _ => Ok(None),
        }
    }

    pub(crate) fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u32, String> {
        match expr {
            Expression::NumericLiteral(_)
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
            Expression::CallExpression(_) => self.emit_call_domain(expr, ctx),
            Expression::ThisExpression(_) => self.emit_this_expression(ctx),
            Expression::SequenceExpression(seq) => self.emit_sequence_expression(seq, ctx),
            Expression::ParenthesizedExpression(p) => self.emit_parenthesized_expression(p, ctx),
            _ => self.emit_unsupported_expression(expr, ctx),
        }
    }

    /// 把完整程序编译为顶层模块体的 IRFunction。
    ///
    /// 调用方为 `oxide_compiler::Compiler::compile`：本函数完成 emit 半程，
    /// 随后由 `oxide_ir::lower::lower` 降为字节码。
    pub fn emit_program(&self, program: &oxide_parser::Program) -> Result<IRFunction, String> {
        let mut ctx = CompileCtx::new();
        ctx.pre_register_builtins();
        self.predeclare_function_declarations(&program.body, &mut ctx);

        // 预注册 builtin 引用（先于任何临时寄存器），builtin 槽不进入临时寄存器池。
        self.pre_register_builtin_references(&program.body, &mut ctx);

        // 预声明顶层 `var` 名，使首个 sub-pass 中提升的函数声明能解析外层 var。
        self.predeclare_var_declarations(&program.body, &mut ctx);

        // 闭包捕获分析（AST 级，emit 前确定）
        ctx.own_bindings = self.collect_own_binding_names(&[], &program.body);
        ctx.captured_bindings = self.collect_captured_bindings(&program.body, &[], &ctx.own_bindings);

        // 首个 sub-pass：发函数声明（hoisting），保证任何代码运行前函数对象已就绪。
        for stmt in &program.body {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                self.emit_statement(stmt, &mut ctx)?;
            }
        }

        // 第二个 sub-pass：发其余所有语句。
        let mut last_result: Option<u32> = None;
        for stmt in &program.body {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                continue; // Already emitted above
            }
            match self.emit_statement(stmt, &mut ctx)? {
                Some(r) => last_result = Some(r),
                None => last_result = None,
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

