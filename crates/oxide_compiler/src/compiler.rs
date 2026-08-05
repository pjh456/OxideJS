use std::collections::{HashMap, HashSet};

use crate::emit_ctx::{LabelCtx, ScopeCtx};
use crate::ir::inst::Inst;
use crate::ir::lower::lower;
use crate::ir::operand::{LabelId, Operand};
use crate::ir::IRFunction;
use crate::symbol_table::{Binding, SymbolTable};
use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::module::UpvalueCapture;
use oxide_bytecode::opcode::OpCode;

pub use crate::hash::{compiled_module_hash, structural_hash};
use crate::symbol_table::ScopeKind;
pub use oxide_bytecode::module::Constant;
pub use oxide_parser::VariableDeclarationKind;
pub use oxide_parser::{AssignmentOperator, BinaryOperator, Expression, Statement, UnaryOperator};

pub struct Compiler;

pub(crate) fn is_int_literal(value: f64) -> bool {
    value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64
}

pub(crate) fn is_side_effect_free(expr: &Expression) -> bool {
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

/// A labeled-statement scope active during emission. `break label` targets
/// `break_label`; `continue label` targets `continue_label` (only set when the
/// labeled statement directly wraps an iteration statement).
#[derive(Debug, Clone)]
pub(crate) struct LabelScope {
    pub(crate) name: String,
    pub(crate) break_label: LabelId,
    pub(crate) continue_label: Option<LabelId>,
}

pub(crate) struct CompileCtx {
    pub(crate) insts: Vec<Inst>,
    pub(crate) constants: Vec<Constant>,
    constant_map: HashMap<ConstantKey, u16>,
    next_reg: u8,
    pub(crate) max_regs: u8,
    reserved_reg_start: u8,
    pub(crate) labels: LabelCtx,
    pub(crate) scopes: ScopeCtx,
    pub(crate) nested: Vec<IRFunction>,
    /// Register holding `this` in the enclosing function context.
    /// Used by arrow functions to capture lexical `this`.
    /// Initialized to 254 (conventional this register) at the top level.
    pub(crate) enclosing_this_reg: u8,
    pub(crate) in_derived_constructor: bool,
    pub(crate) in_instance_method: bool,
    pub(crate) in_static_method: bool,
    pub(crate) static_block_this_reg: Option<u8>,
    pub(crate) field_buffer: Option<FieldBuffer>,
    pub(crate) current_upvalue_captures: Vec<UpvalueCapture>,
    /// 本函数作用域声明的绑定名（参数 + 变量/函数声明，AST 收集，emit 前确定）。
    pub(crate) own_bindings: HashSet<String>,
    /// 本函数被嵌套函数捕获的绑定名（AST 分析，emit 前确定）。
    /// 捕获判断（MAKE_CELL / CELL_GET / CELL_SET）统一查此集合，消除符号表时序依赖。
    pub(crate) captured_bindings: HashSet<String>,
    /// Set when alloc_reg() overflows into the reserved this/new.target range (≥254).
    /// Checked after each emit phase to produce a compile error rather than silent corruption.
    pub(crate) reg_overflow: bool,
    pub(crate) const_overflow: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum FunctionBodyContext {
    Ordinary,
    Arrow,
    ClassElement,
}

pub(crate) enum ParamSpec<'a> {
    Identifier(String),
    Pattern {
        synthetic_name: String,
        pattern: &'a oxide_parser::BindingPattern<'a>,
    },
}

impl ParamSpec<'_> {
    pub(crate) fn register_name(&self) -> &str {
        match self {
            Self::Identifier(name) => name,
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
                cell_registry: Vec::new(),
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
            captured_bindings: HashSet::new(),
            reg_overflow: false,
            const_overflow: false,
        }
    }

    pub(crate) fn inst(&mut self, inst: Inst) {
        self.insts.push(inst);
    }

    pub(crate) fn alloc_reg(&mut self) -> u8 {
        let r = self.next_reg;
        // Registers 254 (this) and 255 (new.target) are reserved by the VM.
        // Allocating into them silently corrupts the call convention, turning
        // method calls' `this` into garbage. Clamp to 253 and set a flag so
        // the compiler can surface a proper error after the emit pass.
        if r >= 254 {
            self.reg_overflow = true;
            return 253; // clamp to last safe register; emit continues but reg_overflow triggers error
        }
        self.next_reg = self.next_reg.wrapping_add(1);
        if self.next_reg > self.max_regs {
            self.max_regs = self.next_reg;
        }
        r
    }

    pub(crate) fn reset_regs(&mut self) {
        self.next_reg = self.builtin_reg_floor().max(self.reserved_reg_start);
        self.labels.label_counter = 0;
    }

    pub(crate) fn reg_checkpoint(&self) -> u8 {
        self.next_reg
    }

    pub(crate) fn restore_reg_checkpoint(&mut self, checkpoint: u8) {
        self.next_reg = checkpoint;
    }

    pub(crate) fn reserve_reg(&mut self, reg: u8) {
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
        &mut self, name: &str, reg: u8, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.scopes.symbols.declare(name, reg, kind, is_const)
    }

    pub(crate) fn declare_initialized(
        &mut self, name: &str, reg: u8, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.scopes.symbols.declare_initialized(name, reg, kind, is_const)
    }

    pub(crate) fn push_scope_with_kind(&mut self, kind: ScopeKind) {
        self.scopes.symbols.push_scope_with_kind(kind);
    }

    pub(crate) fn lookup(&self, name: &str) -> Result<u8, String> {
        self.scopes.symbols.lookup(name)
    }

    pub(crate) fn lookup_or_builtin(&mut self, name: &str) -> Result<u8, String> {
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

    pub(crate) fn lookup_or_global(&mut self, name: &str) -> u8 {
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

    /// Queue a label name to be bound to the continue target of the next loop
    /// emitted as the labeled statement's body. Rejects duplicates in the active
    /// or pending sets.
    pub(crate) fn queue_loop_label(&mut self, name: &str) -> Result<(), String> {
        if self.labels.label_scopes.iter().any(|s| s.name == name)
            || self.labels.pending_loop_labels.iter().any(|n| n == name)
        {
            return Err(format!("SyntaxError: Label '{name}' has already been declared"));
        }
        self.labels.pending_loop_labels.push(name.to_string());
        Ok(())
    }

    /// Drain queued loop labels into active scopes bound to this loop's break and
    /// continue targets. Returns how many scopes were pushed (to pop after).
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

    fn builtin_reg_floor(&self) -> u8 {
        self.scopes
            .builtin_reg_map
            .iter()
            .map(|(_, reg)| reg.saturating_add(1))
            .max()
            .unwrap_or(0)
    }

    pub(crate) fn pre_register_builtins(&mut self) {
        // Builtin globals are resolved lazily by lookup_or_builtin(). Keeping this
        // hook preserves the compile pipeline without reserving ~60 registers in
        // every module.
    }

    /// 组装 IRFunction（两出口共用），take 走编译产物状态。
    /// `parent_ctx` 用于补全 upvalue_captures 的 enclosing_reg（父符号表在父 emit 完成后完整）。
    fn assemble_ir(&mut self, param_layout: crate::ir::ParamLayout, parent_ctx: Option<&CompileCtx>) -> IRFunction {
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
            reg_overflow: self.reg_overflow,
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

impl Compiler {
    pub fn new() -> Self {
        Self
    }

    // ── 闭包捕获分析（AST 级，时序无关）──

    /// 收集当前函数作用域声明的绑定名（参数 + 变量/函数声明，含嵌套 block，不含嵌套函数体）。
    fn collect_own_binding_names(&self, param_names: &[&str], stmts: &[Statement]) -> HashSet<String> {
        let mut names = HashSet::new();
        for p in param_names {
            names.insert(p.to_string());
        }
        self.collect_decl_names_stmt(stmts, &mut names);
        names
    }

    fn collect_decl_names_stmt(&self, stmts: &[Statement], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                Statement::VariableDeclaration(vd) => {
                    for d in &vd.declarations {
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                            out.insert(bi.name.to_string());
                        }
                    }
                }
                Statement::FunctionDeclaration(fd) => {
                    if let Some(id) = &fd.id {
                        out.insert(id.name.to_string());
                    }
                }
                Statement::BlockStatement(b) => self.collect_decl_names_stmt(&b.body, out),
                Statement::IfStatement(is) => {
                    self.collect_decl_names_stmt(std::slice::from_ref(&is.consequent), out);
                    if let Some(alt) = &is.alternate {
                        self.collect_decl_names_stmt(std::slice::from_ref(alt), out);
                    }
                }
                Statement::WhileStatement(w) => self.collect_decl_names_stmt(std::slice::from_ref(&w.body), out),
                Statement::DoWhileStatement(d) => self.collect_decl_names_stmt(std::slice::from_ref(&d.body), out),
                Statement::ForStatement(f) => {
                    if let Some(oxide_parser::ForStatementInit::VariableDeclaration(vd)) = &f.init {
                        for d in &vd.declarations {
                            if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                                out.insert(bi.name.to_string());
                            }
                        }
                    }
                    self.collect_decl_names_stmt(std::slice::from_ref(&f.body), out);
                }
                Statement::SwitchStatement(sw) => {
                    for case in &sw.cases {
                        self.collect_decl_names_stmt(&case.consequent, out);
                    }
                }
                Statement::TryStatement(ts) => {
                    self.collect_decl_names_stmt(&ts.block.body, out);
                    if let Some(h) = &ts.handler {
                        self.collect_decl_names_stmt(&h.body.body, out);
                    }
                    if let Some(f) = &ts.finalizer {
                        self.collect_decl_names_stmt(&f.body, out);
                    }
                }
                Statement::LabeledStatement(ls) => self.collect_decl_names_stmt(std::slice::from_ref(&ls.body), out),
                _ => {}
            }
        }
    }

    /// 扫描 stmts 内（含任意深度嵌套函数）对 `ref_set` 的引用，写入 out。
    /// 递归进入嵌套函数时累加其局部绑定为遮蔽集，避免把内层局部误判为捕获。
    fn collect_capture_names(&self, stmts: &[Statement], ref_set: &HashSet<String>, out: &mut HashSet<String>) {
        let shadow = HashSet::new();
        self.collect_capture_names_shadowed(stmts, ref_set, &shadow, out);
    }

    fn collect_capture_names_shadowed(
        &self, stmts: &[Statement], ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        for stmt in stmts {
            self.collect_capture_names_stmt(stmt, ref_set, shadow, out);
        }
    }

    fn collect_capture_names_stmt(
        &self, stmt: &Statement, ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::ExpressionStatement(es) => self.collect_capture_names_expr(&es.expression, ref_set, shadow, out),
            Statement::ReturnStatement(rs) => {
                if let Some(a) = &rs.argument {
                    self.collect_capture_names_expr(a, ref_set, shadow, out);
                }
            }
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    if let Some(init) = &d.init {
                        self.collect_capture_names_expr(init, ref_set, shadow, out);
                    }
                }
            }
            Statement::FunctionDeclaration(fd) => {
                let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let mut inner = shadow.clone();
                inner.extend(self.collect_own_binding_names(&[], body));
                self.collect_capture_names_shadowed(body, ref_set, &inner, out);
            }
            Statement::IfStatement(is) => {
                self.collect_capture_names_expr(&is.test, ref_set, shadow, out);
                self.collect_capture_names_stmt(&is.consequent, ref_set, shadow, out);
                if let Some(alt) = &is.alternate {
                    self.collect_capture_names_stmt(alt, ref_set, shadow, out);
                }
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                    if let oxide_parser::ForStatementInit::VariableDeclaration(vd) = init {
                        for d in &vd.declarations {
                            if let Some(i) = &d.init {
                                self.collect_capture_names_expr(i, ref_set, shadow, out);
                            }
                        }
                    }
                }
                if let Some(t) = &fs.test {
                    self.collect_capture_names_expr(t, ref_set, shadow, out);
                }
                if let Some(u) = &fs.update {
                    self.collect_capture_names_expr(u, ref_set, shadow, out);
                }
                self.collect_capture_names_stmt(&fs.body, ref_set, shadow, out);
            }
            Statement::WhileStatement(w) => {
                self.collect_capture_names_expr(&w.test, ref_set, shadow, out);
                self.collect_capture_names_stmt(&w.body, ref_set, shadow, out);
            }
            Statement::DoWhileStatement(d) => {
                self.collect_capture_names_stmt(&d.body, ref_set, shadow, out);
                self.collect_capture_names_expr(&d.test, ref_set, shadow, out);
            }
            Statement::ForInStatement(fi) => {
                self.collect_capture_names_expr(&fi.right, ref_set, shadow, out);
                self.collect_capture_names_stmt(&fi.body, ref_set, shadow, out);
            }
            Statement::ForOfStatement(fo) => {
                self.collect_capture_names_expr(&fo.right, ref_set, shadow, out);
                self.collect_capture_names_stmt(&fo.body, ref_set, shadow, out);
            }
            Statement::BlockStatement(b) => self.collect_capture_names_shadowed(&b.body, ref_set, shadow, out),
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.collect_capture_names_stmt(s, ref_set, shadow, out);
                }
                if let Some(h) = &ts.handler {
                    for s in &h.body.body {
                        self.collect_capture_names_stmt(s, ref_set, shadow, out);
                    }
                }
                if let Some(f) = &ts.finalizer {
                    for s in &f.body {
                        self.collect_capture_names_stmt(s, ref_set, shadow, out);
                    }
                }
            }
            Statement::ThrowStatement(ts) => self.collect_capture_names_expr(&ts.argument, ref_set, shadow, out),
            Statement::SwitchStatement(sw) => {
                self.collect_capture_names_expr(&sw.discriminant, ref_set, shadow, out);
                for case in &sw.cases {
                    for s in &case.consequent {
                        self.collect_capture_names_stmt(s, ref_set, shadow, out);
                    }
                }
            }
            Statement::LabeledStatement(ls) => self.collect_capture_names_stmt(&ls.body, ref_set, shadow, out),
            _ => {}
        }
    }

    fn collect_capture_names_expr(
        &self, expr: &Expression, ref_set: &HashSet<String>, shadow: &HashSet<String>, out: &mut HashSet<String>,
    ) {
        match expr {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if ref_set.contains(name) && !shadow.contains(name) {
                    out.insert(id.name.to_string());
                }
            }
            Expression::AssignmentExpression(ae) => {
                if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(ati) = &ae.left {
                    let name = ati.name.as_str();
                    if ref_set.contains(name) && !shadow.contains(name) {
                        out.insert(ati.name.to_string());
                    }
                }
                self.collect_capture_names_expr(&ae.right, ref_set, shadow, out);
            }
            Expression::UpdateExpression(ue) => {
                if let oxide_parser::SimpleAssignmentTarget::AssignmentTargetIdentifier(ati) = &ue.argument {
                    let name = ati.name.as_str();
                    if ref_set.contains(name) && !shadow.contains(name) {
                        out.insert(ati.name.to_string());
                    }
                }
            }
            Expression::FunctionExpression(fe) => {
                let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let mut inner = shadow.clone();
                inner.extend(self.collect_own_binding_names(&[], body));
                self.collect_capture_names_shadowed(body, ref_set, &inner, out);
            }
            Expression::ArrowFunctionExpression(ae) => {
                let mut inner = shadow.clone();
                inner.extend(self.collect_own_binding_names(&[], &ae.body.statements));
                self.collect_capture_names_shadowed(&ae.body.statements, ref_set, &inner, out);
            }
            Expression::BinaryExpression(be) => {
                self.collect_capture_names_expr(&be.left, ref_set, shadow, out);
                self.collect_capture_names_expr(&be.right, ref_set, shadow, out);
            }
            Expression::UnaryExpression(ue) => self.collect_capture_names_expr(&ue.argument, ref_set, shadow, out),
            Expression::CallExpression(ce) => {
                self.collect_capture_names_expr(&ce.callee, ref_set, shadow, out);
                for a in &ce.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            }
            Expression::NewExpression(ne) => {
                self.collect_capture_names_expr(&ne.callee, ref_set, shadow, out);
                for a in &ne.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            }
            Expression::SequenceExpression(se) => {
                for e in &se.expressions {
                    self.collect_capture_names_expr(e, ref_set, shadow, out);
                }
            }
            Expression::ConditionalExpression(ce) => {
                self.collect_capture_names_expr(&ce.test, ref_set, shadow, out);
                self.collect_capture_names_expr(&ce.consequent, ref_set, shadow, out);
                self.collect_capture_names_expr(&ce.alternate, ref_set, shadow, out);
            }
            Expression::ArrayExpression(ae) => {
                for e in &ae.elements {
                    if let Some(e) = e.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            }
            Expression::LogicalExpression(le) => {
                self.collect_capture_names_expr(&le.left, ref_set, shadow, out);
                self.collect_capture_names_expr(&le.right, ref_set, shadow, out);
            }
            Expression::ComputedMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
                self.collect_capture_names_expr(&m.expression, ref_set, shadow, out);
            }
            Expression::StaticMemberExpression(m) => self.collect_capture_names_expr(&m.object, ref_set, shadow, out),
            Expression::PrivateFieldExpression(m) => self.collect_capture_names_expr(&m.object, ref_set, shadow, out),
            Expression::ParenthesizedExpression(p) => self.collect_capture_names_expr(&p.expression, ref_set, shadow, out),
            Expression::TemplateLiteral(tl) => {
                for e in &tl.expressions {
                    self.collect_capture_names_expr(e, ref_set, shadow, out);
                }
            }
            Expression::TaggedTemplateExpression(tt) => {
                self.collect_capture_names_expr(&tt.tag, ref_set, shadow, out);
                for e in &tt.quasi.expressions {
                    self.collect_capture_names_expr(e, ref_set, shadow, out);
                }
            }
            Expression::ObjectExpression(o) => {
                for prop in &o.properties {
                    if let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop {
                        self.collect_capture_names_expr(&p.value, ref_set, shadow, out);
                    }
                }
            }
            Expression::ChainExpression(c) => self.collect_capture_names_chain(&c.expression, ref_set, shadow, out),
            _ => {}
        }
    }

    fn collect_capture_names_chain(
        &self, element: &oxide_parser::ChainElement, ref_set: &HashSet<String>, shadow: &HashSet<String>,
        out: &mut HashSet<String>,
    ) {
        match element {
            oxide_parser::ChainElement::StaticMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
            }
            oxide_parser::ChainElement::ComputedMemberExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
                self.collect_capture_names_expr(&m.expression, ref_set, shadow, out);
            }
            oxide_parser::ChainElement::PrivateFieldExpression(m) => {
                self.collect_capture_names_expr(&m.object, ref_set, shadow, out);
            }
            oxide_parser::ChainElement::CallExpression(call) => {
                self.collect_capture_names_expr(&call.callee, ref_set, shadow, out);
                for a in &call.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_capture_names_expr(e, ref_set, shadow, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// 分析本函数：哪些绑定被任意深度嵌套函数捕获 → captured_bindings。
    fn collect_captured_bindings(&self, stmts: &[Statement], own: &HashSet<String>) -> HashSet<String> {
        let mut captured = HashSet::new();
        for stmt in stmts {
            self.collect_captured_stmt(stmt, own, &mut captured);
        }
        captured
    }

    /// 只从嵌套函数节点进入扫描（本函数直接引用不算捕获）。
    fn collect_captured_stmt(&self, stmt: &Statement, own: &HashSet<String>, out: &mut HashSet<String>) {
        match stmt {
            Statement::FunctionDeclaration(fd) => {
                let body: &[Statement] = fd.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                self.collect_capture_names(body, own, out);
            }
            Statement::ExpressionStatement(es) => self.collect_captured_expr(&es.expression, own, out),
            Statement::ReturnStatement(rs) => {
                if let Some(a) = &rs.argument {
                    self.collect_captured_expr(a, own, out);
                }
            }
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    if let Some(init) = &d.init {
                        self.collect_captured_expr(init, own, out);
                    }
                }
            }
            Statement::IfStatement(is) => {
                self.collect_captured_expr(&is.test, own, out);
                self.collect_captured_stmt(&is.consequent, own, out);
                if let Some(alt) = &is.alternate {
                    self.collect_captured_stmt(alt, own, out);
                }
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.collect_captured_expr(e, own, out);
                    }
                }
                if let Some(t) = &fs.test {
                    self.collect_captured_expr(t, own, out);
                }
                if let Some(u) = &fs.update {
                    self.collect_captured_expr(u, own, out);
                }
                self.collect_captured_stmt(&fs.body, own, out);
            }
            Statement::WhileStatement(w) => {
                self.collect_captured_expr(&w.test, own, out);
                self.collect_captured_stmt(&w.body, own, out);
            }
            Statement::DoWhileStatement(d) => {
                self.collect_captured_stmt(&d.body, own, out);
                self.collect_captured_expr(&d.test, own, out);
            }
            Statement::ForInStatement(fi) => {
                self.collect_captured_expr(&fi.right, own, out);
                self.collect_captured_stmt(&fi.body, own, out);
            }
            Statement::ForOfStatement(fo) => {
                self.collect_captured_expr(&fo.right, own, out);
                self.collect_captured_stmt(&fo.body, own, out);
            }
            Statement::BlockStatement(b) => {
                for s in &b.body {
                    self.collect_captured_stmt(s, own, out);
                }
            }
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.collect_captured_stmt(s, own, out);
                }
                if let Some(h) = &ts.handler {
                    for s in &h.body.body {
                        self.collect_captured_stmt(s, own, out);
                    }
                }
                if let Some(f) = &ts.finalizer {
                    for s in &f.body {
                        self.collect_captured_stmt(s, own, out);
                    }
                }
            }
            Statement::ThrowStatement(ts) => self.collect_captured_expr(&ts.argument, own, out),
            Statement::SwitchStatement(sw) => {
                self.collect_captured_expr(&sw.discriminant, own, out);
                for case in &sw.cases {
                    for s in &case.consequent {
                        self.collect_captured_stmt(s, own, out);
                    }
                }
            }
            Statement::LabeledStatement(ls) => self.collect_captured_stmt(&ls.body, own, out),
            _ => {}
        }
    }

    fn collect_captured_expr(&self, expr: &Expression, own: &HashSet<String>, out: &mut HashSet<String>) {
        match expr {
            Expression::FunctionExpression(fe) => {
                let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                self.collect_capture_names(body, own, out);
            }
            Expression::ArrowFunctionExpression(ae) => {
                self.collect_capture_names(&ae.body.statements, own, out);
            }
            Expression::CallExpression(ce) => {
                self.collect_captured_expr(&ce.callee, own, out);
                for a in &ce.arguments {
                    if let Some(e) = a.as_expression() {
                        self.collect_captured_expr(e, own, out);
                    }
                }
            }
            Expression::BinaryExpression(be) => {
                self.collect_captured_expr(&be.left, own, out);
                self.collect_captured_expr(&be.right, own, out);
            }
            Expression::UnaryExpression(ue) => self.collect_captured_expr(&ue.argument, own, out),
            Expression::LogicalExpression(le) => {
                self.collect_captured_expr(&le.left, own, out);
                self.collect_captured_expr(&le.right, own, out);
            }
            Expression::ConditionalExpression(ce) => {
                self.collect_captured_expr(&ce.test, own, out);
                self.collect_captured_expr(&ce.consequent, own, out);
                self.collect_captured_expr(&ce.alternate, own, out);
            }
            Expression::SequenceExpression(se) => {
                for e in &se.expressions {
                    self.collect_captured_expr(e, own, out);
                }
            }
            Expression::AssignmentExpression(ae) => self.collect_captured_expr(&ae.right, own, out),
            Expression::ArrayExpression(ae) => {
                for e in &ae.elements {
                    if let Some(e) = e.as_expression() {
                        self.collect_captured_expr(e, own, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// 分析子函数 body：引用父级绑定的名字 → upvalue_captures（enclosing_reg 由父 emit 完成后填充）。
    fn collect_upvalue_names(
        &self, body_stmts: &[Statement], parent_own: &HashSet<String>, sub_own: &HashSet<String>,
    ) -> Vec<UpvalueCapture> {
        let mut names = HashSet::new();
        self.collect_capture_names_shadowed(body_stmts, parent_own, sub_own, &mut names);
        let mut captures = Vec::with_capacity(names.len());
        for (cell_idx, name) in names.iter().enumerate() {
            captures.push(UpvalueCapture {
                name: name.clone(),
                enclosing_reg: 0, // assemble_ir 时从父符号表填充
                cell_idx: cell_idx as u8,
            });
        }
        captures
    }


    pub(crate) fn extract_function_parts<'a>(
        &self, function: &'a oxide_parser::Function<'a>,
    ) -> Result<(Vec<ParamSpec<'a>>, &'a [Statement<'a>]), String> {
        let mut param_specs = Vec::new();
        for (idx, param) in function.params.items.iter().enumerate() {
            match &param.pattern {
                oxide_parser::BindingPattern::BindingIdentifier(bi) => {
                    param_specs.push(ParamSpec::Identifier(bi.name.to_string()));
                }
                pattern => {
                    param_specs.push(ParamSpec::Pattern {
                        synthetic_name: format!("@@param_{idx}"),
                        pattern,
                    });
                }
            }
        }
        let body_stmts: &[Statement] = if let Some(body) = &function.body { &body.statements } else { &[] };
        Ok((param_specs, body_stmts))
    }

    /// Compile a function body (used for FD, FE, and arrow functions).
    /// This performs both counting and emitting in one pass.
    /// When `is_expression_body` is true (arrow function with expression body),
    /// the last expression's value is returned instead of undefined.
    /// `is_arrow` controls whether super flags are inherited from the parent scope
    /// (true for arrow functions, which have lexical super) or reset to false
    /// (false for regular functions, which create a new super scope).
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
        is_expression_body: bool, extra_bindings: &[(&str, u8)], body_context: FunctionBodyContext,
    ) -> Result<IRFunction, String> {
        self.compile_function_body_with_field_hooks(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            extra_bindings,
            body_context,
            None::<fn(&Compiler, &mut CompileCtx) -> Result<(), String>>,
            false,
        )
    }


    /// Pre-register all builtin identifiers referenced anywhere in this body
    /// (expressions, member objects, call args, class fields, etc.) so their
    /// register slots are reserved *before* any temporary register is emitted.
    ///
    /// Without this, single-pass emission lazily allocates builtin slots via
    /// `lookup_or_builtin` at first use, which can collide with a temporary
    /// register already reused by `restore_reg_checkpoint`. Pre-scanning moves
    /// builtin slot allocation ahead of the temporary register pool.
    pub(crate) fn pre_register_builtin_references(&self, stmts: &[Statement], ctx: &mut CompileCtx) {
        for stmt in stmts {
            self.pre_scan_builtin_stmt(stmt, ctx);
        }
    }

    fn pre_scan_builtin_stmt(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::ExpressionStatement(es) => self.pre_scan_builtin_expr(&es.expression, ctx),
            Statement::VariableDeclaration(vd) => {
                for d in &vd.declarations {
                    if let Some(init) = &d.init {
                        self.pre_scan_builtin_expr(init, ctx);
                    }
                }
            }
            Statement::ReturnStatement(rs) => {
                if let Some(a) = &rs.argument {
                    self.pre_scan_builtin_expr(a, ctx);
                }
            }
            Statement::IfStatement(is) => {
                self.pre_scan_builtin_expr(&is.test, ctx);
                self.pre_scan_builtin_stmt(&is.consequent, ctx);
                if let Some(alt) = &is.alternate {
                    self.pre_scan_builtin_stmt(alt, ctx);
                }
            }
            Statement::WhileStatement(wh) => {
                self.pre_scan_builtin_expr(&wh.test, ctx);
                self.pre_scan_builtin_stmt(&wh.body, ctx);
            }
            Statement::DoWhileStatement(dw) => {
                self.pre_scan_builtin_stmt(&dw.body, ctx);
                self.pre_scan_builtin_expr(&dw.test, ctx);
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.pre_scan_builtin_expr(e, ctx);
                    } else if let oxide_parser::ForStatementInit::VariableDeclaration(decl) = init {
                        for d in &decl.declarations {
                            if let Some(init_expr) = &d.init {
                                self.pre_scan_builtin_expr(init_expr, ctx);
                            }
                        }
                    }
                }
                if let Some(t) = &fs.test {
                    self.pre_scan_builtin_expr(t, ctx);
                }
                if let Some(u) = &fs.update {
                    self.pre_scan_builtin_expr(u, ctx);
                }
                self.pre_scan_builtin_stmt(&fs.body, ctx);
            }
            Statement::ForInStatement(fi) => {
                self.pre_scan_builtin_expr(&fi.right, ctx);
                self.pre_scan_builtin_stmt(&fi.body, ctx);
            }
            Statement::ForOfStatement(fo) => {
                self.pre_scan_builtin_expr(&fo.right, ctx);
                self.pre_scan_builtin_stmt(&fo.body, ctx);
            }
            Statement::SwitchStatement(sw) => {
                self.pre_scan_builtin_expr(&sw.discriminant, ctx);
                for case in &sw.cases {
                    if let Some(test) = &case.test {
                        self.pre_scan_builtin_expr(test, ctx);
                    }
                    for s in &case.consequent {
                        self.pre_scan_builtin_stmt(s, ctx);
                    }
                }
            }
            Statement::ThrowStatement(ts) => {
                self.pre_scan_builtin_expr(&ts.argument, ctx);
            }
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.pre_scan_builtin_stmt(s, ctx);
                }
                if let Some(handler) = &ts.handler {
                    for s in &handler.body.body {
                        self.pre_scan_builtin_stmt(s, ctx);
                    }
                }
                if let Some(finalizer) = &ts.finalizer {
                    for s in &finalizer.body {
                        self.pre_scan_builtin_stmt(s, ctx);
                    }
                }
            }
            Statement::BlockStatement(bs) => {
                for s in &bs.body {
                    self.pre_scan_builtin_stmt(s, ctx);
                }
            }
            Statement::LabeledStatement(ls) => self.pre_scan_builtin_stmt(&ls.body, ctx),
            Statement::ClassDeclaration(cd) => {
                if let Some(super_class) = &cd.super_class {
                    self.pre_scan_builtin_expr(super_class, ctx);
                }
            }
            Statement::FunctionDeclaration(_) | Statement::BreakStatement(_) | Statement::ContinueStatement(_) => {}
            _ => {}
        }
    }

    fn pre_scan_builtin_expr(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::Identifier(ident) => {
                if CompileCtx::is_known_builtin(ident.name.as_str()) {
                    let _ = ctx.lookup_or_builtin(ident.name.as_str());
                }
            }
            Expression::BinaryExpression(bin) => {
                self.pre_scan_builtin_expr(&bin.left, ctx);
                self.pre_scan_builtin_expr(&bin.right, ctx);
            }
            Expression::UnaryExpression(un) => {
                self.pre_scan_builtin_expr(&un.argument, ctx);
            }
            Expression::CallExpression(call) => {
                self.pre_scan_builtin_expr(&call.callee, ctx);
                for arg in &call.arguments {
                    if let Some(e) = arg.as_expression() {
                        self.pre_scan_builtin_expr(e, ctx);
                    }
                }
            }
            Expression::NewExpression(ne) => {
                self.pre_scan_builtin_expr(&ne.callee, ctx);
                for arg in &ne.arguments {
                    if let Some(e) = arg.as_expression() {
                        self.pre_scan_builtin_expr(e, ctx);
                    }
                }
            }
            Expression::LogicalExpression(log) => {
                self.pre_scan_builtin_expr(&log.left, ctx);
                self.pre_scan_builtin_expr(&log.right, ctx);
            }
            Expression::ConditionalExpression(cond) => {
                self.pre_scan_builtin_expr(&cond.test, ctx);
                self.pre_scan_builtin_expr(&cond.consequent, ctx);
                self.pre_scan_builtin_expr(&cond.alternate, ctx);
            }
            Expression::PrivateInExpression(pin) => {
                self.pre_scan_builtin_expr(&pin.right, ctx);
            }
            Expression::SequenceExpression(seq) => {
                for e in &seq.expressions {
                    self.pre_scan_builtin_expr(e, ctx);
                }
            }
            Expression::AssignmentExpression(assign) => {
                if let Some(target) = assign.left.as_simple_assignment_target() {
                    self.pre_scan_builtin_target(target, ctx);
                }
                self.pre_scan_builtin_expr(&assign.right, ctx);
            }
            Expression::UpdateExpression(update) => {
                self.pre_scan_builtin_target(&update.argument, ctx);
            }
            Expression::TemplateLiteral(tl) => {
                for e in &tl.expressions {
                    self.pre_scan_builtin_expr(e, ctx);
                }
            }
            Expression::TaggedTemplateExpression(tt) => {
                self.pre_scan_builtin_expr(&tt.tag, ctx);
                for e in &tt.quasi.expressions {
                    self.pre_scan_builtin_expr(e, ctx);
                }
            }
            Expression::ObjectExpression(obj) => {
                for prop in &obj.properties {
                    if let oxide_parser::ObjectPropertyKind::ObjectProperty(p) = prop {
                        if p.computed {
                            self.pre_scan_builtin_expr(p.key.to_expression(), ctx);
                        }
                        self.pre_scan_builtin_expr(&p.value, ctx);
                    }
                }
            }
            Expression::ArrayExpression(arr) => {
                for e in &arr.elements {
                    if let Some(e) = e.as_expression() {
                        self.pre_scan_builtin_expr(e, ctx);
                    }
                }
            }
            Expression::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            Expression::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            Expression::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
                self.pre_scan_builtin_expr(&member.expression, ctx);
            }
            Expression::ChainExpression(chain) => {
                self.pre_scan_builtin_chain(&chain.expression, ctx);
            }
            Expression::ParenthesizedExpression(p) => self.pre_scan_builtin_expr(&p.expression, ctx),
            Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ClassExpression(_) => {}
            _ => {}
        }
    }

    fn pre_scan_builtin_target(&self, target: &oxide_parser::SimpleAssignmentTarget, ctx: &mut CompileCtx) {
        match target {
            oxide_parser::SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            oxide_parser::SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
                self.pre_scan_builtin_expr(&member.expression, ctx);
            }
            oxide_parser::SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            _ => {}
        }
    }

    fn pre_scan_builtin_chain(&self, element: &oxide_parser::ChainElement, ctx: &mut CompileCtx) {
        match element {
            oxide_parser::ChainElement::StaticMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            oxide_parser::ChainElement::ComputedMemberExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
                self.pre_scan_builtin_expr(&member.expression, ctx);
            }
            oxide_parser::ChainElement::PrivateFieldExpression(member) => {
                self.pre_scan_builtin_expr(&member.object, ctx);
            }
            oxide_parser::ChainElement::CallExpression(call) => {
                self.pre_scan_builtin_expr(&call.callee, ctx);
                for arg in &call.arguments {
                    if let Some(e) = arg.as_expression() {
                        self.pre_scan_builtin_expr(e, ctx);
                    }
                }
            }
            _ => {}
        }
    }


    fn predeclare_function_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            let Statement::FunctionDeclaration(function) = statement else {
                continue;
            };
            let Some(identifier) = &function.id else {
                continue;
            };
            let reg = ctx.alloc_reg();
            let _ = ctx.declare_initialized(identifier.name.as_str(), reg, VariableDeclarationKind::Var, false);
        }
    }

    /// Pre-declare all `var` bindings in this statement list so hoisted function
    /// declarations emitted earlier can resolve them. Mirrors the removed count
    /// pass behavior (see fe8bd86): top-level `var` names must be visible while
    /// compiling function bodies that close over them.
    fn predeclare_var_declarations(&self, statements: &[Statement], ctx: &mut CompileCtx) {
        for statement in statements {
            match statement {
                Statement::VariableDeclaration(decl) => {
                    if !matches!(decl.kind, VariableDeclarationKind::Var) {
                        continue;
                    }
                    for d in &decl.declarations {
                        if let oxide_parser::BindingPattern::BindingIdentifier(bi) = &d.id {
                            let reg = ctx.alloc_reg();
                            let _ = ctx.declare_initialized(bi.name.as_str(), reg, VariableDeclarationKind::Var, false);
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
                                    let reg = ctx.alloc_reg();
                                    let _ =
                                        ctx.declare_initialized(bi.name.as_str(), reg, VariableDeclarationKind::Var, false);
                                }
                            }
                        }
                    }
                    self.predeclare_var_declarations(std::slice::from_ref(&fs.body), ctx);
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
                _ => {}
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compile_function_body_with_field_hooks<'a, E>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, extra_bindings: &[(&str, u8)], body_context: FunctionBodyContext,
        mut emit_fields: Option<E>, fields_after_super: bool,
    ) -> Result<IRFunction, String>
    where
        E: FnMut(&Compiler, &mut CompileCtx) -> Result<(), String>,
    {
        let mut ctx = CompileCtx::new();

        // Inherit parent's builtin_reg_map so builtin identifiers (Math, Object, etc.)
        // resolve to the correct pre-allocated registers in the sub-module's register file.
        ctx.scopes.builtin_reg_map = parent_ctx.scopes.builtin_reg_map.clone();
        ctx.scopes.private_name_map = parent_ctx.scopes.private_name_map.clone();
        ctx.scopes.next_private_name_id = parent_ctx.scopes.next_private_name_id;

        // Propagate enclosing_this_reg so nested arrow functions capture the correct `this`.
        ctx.enclosing_this_reg = parent_ctx.enclosing_this_reg;
        // Arrow functions inherit lexical super. Class method bodies also need the
        // class-provided super context for their top-level body compilation.
        if matches!(body_context, FunctionBodyContext::Arrow | FunctionBodyContext::ClassElement) {
            ctx.in_derived_constructor = parent_ctx.in_derived_constructor;
            ctx.in_instance_method = parent_ctx.in_instance_method;
            ctx.in_static_method = parent_ctx.in_static_method;
        } else {
            ctx.in_derived_constructor = false;
            ctx.in_instance_method = false;
            ctx.in_static_method = false;
        }

        // Inherit parent's global scope entries so previously-declared function names
        // are visible from within the body.
        let mut inherited_reg_start = 1u8.max(ctx.builtin_reg_floor());
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

        // Align next_reg with builtin count so both count and emit passes start at the
        // same register offset (params go after builtin slots).
        ctx.reset_regs();

        // Function body scope - params and local vars
        ctx.push_scope_with_kind(ScopeKind::FunctionScope);

        let param_base = ctx.next_reg;

        // Emit parameters and destructuring prologue.
        for spec in param_specs {
            let name = spec.register_name();
            let reg = ctx.alloc_reg();
            ctx.declare_initialized(name, reg, VariableDeclarationKind::Var, false)?;
        }

        for spec in param_specs {
            if let ParamSpec::Pattern { synthetic_name, pattern } = spec {
                let src_reg = ctx.lookup(synthetic_name)?;
                self.emit_binding_pattern(pattern, src_reg, VariableDeclarationKind::Var, false, &mut ctx)?;
            }
        }

        // 闭包捕获分析（AST 级，emit 前确定，时序无关）
        let param_names: Vec<&str> = param_specs.iter().map(|s| s.register_name()).collect();
        ctx.own_bindings = self.collect_own_binding_names(&param_names, body_stmts);
        ctx.captured_bindings = self.collect_captured_bindings(body_stmts, &ctx.own_bindings);

        // Free variable analysis for upvalue capture (Ordinary + Arrow functions only)
        if matches!(body_context, FunctionBodyContext::Ordinary | FunctionBodyContext::Arrow) {
            let captures = self.collect_upvalue_names(body_stmts, &parent_ctx.own_bindings, &ctx.own_bindings);
            ctx.current_upvalue_captures = captures;
            ctx.scopes.cell_registry = ctx
                .current_upvalue_captures
                .iter()
                .map(|u| (u.name.clone(), u.cell_idx))
                .collect();
        }

        self.predeclare_function_declarations(body_stmts, &mut ctx);

        // Pre-register builtin identifier references before emitting any temporary
        // register, so builtin slots never collide with reused temporaries.
        self.pre_register_builtin_references(body_stmts, &mut ctx);

        // Pre-declare `var` names so hoisted function declarations (emitted in the
        // first sub-pass below) can resolve the outer vars they close over.
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

        // Emit body statements.
        // First sub-pass: emit function declarations (hoisting + marks parent captures)
        let mut last_result_reg = None;
        for stmt in body_stmts {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                if let Some(reg) = self.emit_statement(stmt, &mut ctx)? {
                    last_result_reg = Some(reg);
                }
            }
        }
        // Second sub-pass: emit all other statements
        for stmt in body_stmts {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                continue;
            }
            if let Some(reg) = self.emit_statement(stmt, &mut ctx)? {
                last_result_reg = Some(reg);
            }
        }

        // Emit implicit RETURN: expression body returns the last expression,
        // statement body returns undefined.
        if is_expression_body {
            if let Some(reg) = last_result_reg {
                ctx.inst(Inst::new(OpCode::RETURN, Operand::Reg(reg as u32), Operand::None, Operand::None));
            } else {
                let undef_idx = ctx.add_constant(Constant::Undefined);
                let undef_reg = ctx.alloc_reg();
                ctx.inst(Inst::load_const(Operand::Reg(undef_reg as u32), undef_idx));
                ctx.inst(Inst::new(OpCode::RETURN, Operand::Reg(undef_reg as u32), Operand::None, Operand::None));
            }
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            let undef_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(undef_reg as u32), undef_idx));
            ctx.inst(Inst::new(OpCode::RETURN, Operand::Reg(undef_reg as u32), Operand::None, Operand::None));
        }

        let ir = ctx.assemble_ir(
            crate::ir::ParamLayout {
                base: param_base as u32,
                count: param_specs.len() as u32,
            },
            Some(parent_ctx),
        );
        Ok(ir)
    }

    pub(crate) fn emit_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) -> Result<Option<u8>, String> {
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

    pub(crate) fn emit_expression(&self, expr: &Expression, ctx: &mut CompileCtx) -> Result<u8, String> {
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

    pub fn compile(&self, program: &oxide_parser::Program) -> Result<CompiledModule, String> {
        crate::compiler_debug!("compile: starting...");
        let mut ctx = CompileCtx::new();
        ctx.pre_register_builtins();
        self.predeclare_function_declarations(&program.body, &mut ctx);

        // Pre-register builtin identifier references before any temporary register
        // is emitted, keeping builtin slots clear of the temporary register pool.
        self.pre_register_builtin_references(&program.body, &mut ctx);

        // Pre-declare top-level `var` names so hoisted function declarations
        // (emitted in the first sub-pass below) can resolve the outer vars.
        self.predeclare_var_declarations(&program.body, &mut ctx);

        // 闭包捕获分析（AST 级，emit 前确定）
        ctx.own_bindings = self.collect_own_binding_names(&[], &program.body);
        ctx.captured_bindings = self.collect_captured_bindings(&program.body, &ctx.own_bindings);

        // First sub-pass: emit FunctionDeclarations (hoisting)
        // This ensures function objects are available before any code runs.
        for stmt in &program.body {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                self.emit_statement(stmt, &mut ctx)?;
            }
        }

        // Second sub-pass: emit all other statements
        let mut last_result: Option<u8> = None;
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
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::None, Operand::Reg(r as u32), Operand::None));
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.inst(Inst::load_const(Operand::None, undef_idx));
        }
        ctx.inst(Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None));

        crate::compiler_debug!("compile: done, {} instructions, {} constants", ctx.insts.len(), ctx.constants.len());

        let ir = ctx.assemble_ir(
            crate::ir::ParamLayout {
                base: ctx.scopes.builtin_reg_map.len() as u32,
                count: 0,
            },
            None,
        );
        lower(&ir)
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
