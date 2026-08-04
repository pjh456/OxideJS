use std::cell::Cell;
use std::collections::HashMap;

use crate::emit_ctx::{LabelCtx, ScopeCtx};
use crate::symbol_table::{Binding, SymbolTable};
use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::module::UpvalueCapture;
use oxide_bytecode::opcode::{self, OpCode};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub(crate) enum Label {
    IfElse(u32),
    IfEnd(u32),
    WhileStart(u32),
    WhileEnd(u32),
    ForStart(u32),
    ForUpdate(u32),
    ForEnd(u32),
    TernaryEnd(u32),
    TernaryElse(u32),
    DoWhileStart(u32),
    DoWhileEnd(u32),
    ForInStart(u32),
    ForInEnd(u32),
    ForOfStart(u32),
    ForOfEnd(u32),
    SwitchEnd(u32),
    SwitchCase(u32, u32),
    CatchBody(u32),
    FinallyBody(u32),
    TryEnd(u32),
    LabeledEnd(u32),
}

pub(crate) struct JumpFixup {
    pub(crate) pc: usize,
    pub(crate) label: Label,
    pub(crate) opcode: OpCode,
    pub(crate) rd: u8,
}

/// A labeled-statement scope active during emission. `break label` targets
/// `break_label`; `continue label` targets `continue_label` (only set when the
/// labeled statement directly wraps an iteration statement).
#[derive(Debug, Clone)]
pub(crate) struct LabelScope {
    pub(crate) name: String,
    pub(crate) break_label: Label,
    pub(crate) continue_label: Option<Label>,
}

pub(crate) struct CompileCtx {
    pub(crate) bytecode: Vec<opcode::Instr>,
    pub(crate) fixups: Vec<JumpFixup>,
    pub(crate) constants: Vec<Constant>,
    constant_map: HashMap<ConstantKey, u16>,
    next_reg: u8,
    pub(crate) max_regs: u8,
    reserved_reg_start: u8,
    pub(crate) labels: LabelCtx,
    pub(crate) scopes: ScopeCtx,
    pub(crate) projected_pc: usize,
    pub(crate) sub_modules: Vec<CompiledModule>,
    /// Register holding `this` in the enclosing function context.
    /// Used by arrow functions to capture lexical `this`.
    /// Initialized to 254 (conventional this register) at the top level.
    pub(crate) enclosing_this_reg: u8,
    pub(crate) in_derived_constructor: bool,
    pub(crate) in_instance_method: bool,
    pub(crate) in_static_method: bool,
    pub(crate) static_block_this_reg: Option<u8>,
    pub(crate) after_super_insert: Option<Vec<opcode::Instr>>,
    pub(crate) after_super_inserted: bool,
    /// Count-pass mirror of `after_super_insert`: the instruction count of a derived
    /// constructor's instance-field code, added at the super() call site during body
    /// counting so projected_pc matches where the emit pass splices the field bytecode.
    pub(crate) after_super_count_words: Option<usize>,
    pub(crate) current_upvalue_captures: Vec<UpvalueCapture>,
    /// Set when alloc_reg() overflows into the reserved this/new.target range (≥254).
    /// Checked after each emit phase to produce a compile error rather than silent corruption.
    pub(crate) reg_overflow: bool,
    pub(crate) const_overflow: bool,
    pub(crate) jump_overflow: bool,
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
            bytecode: Vec::new(),
            fixups: Vec::new(),
            constants: Vec::new(),
            constant_map: HashMap::new(),
            next_reg: 1,
            max_regs: 1,
            reserved_reg_start: 1,
            labels: LabelCtx {
                label_map: HashMap::new(),
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
            projected_pc: 0,
            sub_modules: Vec::new(),
            enclosing_this_reg: 254, // conventional this register at top level
            in_derived_constructor: false,
            in_instance_method: false,
            in_static_method: false,
            static_block_this_reg: None,
            after_super_insert: None,
            after_super_inserted: false,
            after_super_count_words: None,
            current_upvalue_captures: Vec::new(),
            reg_overflow: false,
            const_overflow: false,
            jump_overflow: false,
        }
    }

    pub(crate) fn emit(&mut self, instr: opcode::Instr) {
        self.bytecode.push(instr);
    }

    pub(crate) fn emit_load_const(&mut self, reg: u8, idx: u16) {
        self.emit(opcode::encode(OpCode::LOAD_CONST, reg, (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8));
    }

    pub(crate) fn emit_create_closure(&mut self, reg: u8, sub_idx: u32) {
        let idx = sub_idx as u16;
        self.emit(opcode::encode(OpCode::CREATE_CLOSURE, reg, (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8));
    }

    #[allow(dead_code)]
    pub(crate) fn emit_jmp_labeled(&mut self, label: Label) {
        let pc = self.bytecode.len();
        self.emit(opcode::encode_jmp(0));
        self.fixups.push(JumpFixup {
            pc,
            label,
            opcode: OpCode::JMP,
            rd: 0,
        });
    }

    #[allow(dead_code)]
    pub(crate) fn emit_jmp_if_false_labeled(&mut self, rd: u8, label: Label) {
        let pc = self.bytecode.len();
        self.emit(opcode::encode_jmp_if_false(rd, 0));
        self.fixups.push(JumpFixup {
            pc,
            label,
            opcode: OpCode::JMP_IF_FALSE,
            rd,
        });
    }

    #[allow(dead_code)]
    pub(crate) fn emit_jmp_if_true_labeled(&mut self, rd: u8, label: Label) {
        let pc = self.bytecode.len();
        self.emit(opcode::encode_jmp_if_true(rd, 0));
        self.fixups.push(JumpFixup {
            pc,
            label,
            opcode: OpCode::JMP_IF_TRUE,
            rd,
        });
    }

    #[allow(dead_code)]
    pub(crate) fn emit_jmp_if_nullish_labeled(&mut self, rd: u8, label: Label) {
        let pc = self.bytecode.len();
        self.emit(opcode::encode_jmp_if_nullish(rd, 0));
        self.fixups.push(JumpFixup {
            pc,
            label,
            opcode: OpCode::JMP_IF_NULLISH,
            rd,
        });
    }

    #[allow(dead_code)]
    pub(crate) fn emit_try_begin_labeled(&mut self, label: Label) {
        let pc = self.bytecode.len();
        self.emit(opcode::encode_try_begin(0));
        self.fixups.push(JumpFixup {
            pc,
            label,
            opcode: OpCode::TRY_BEGIN,
            rd: 0,
        });
    }

    pub(crate) fn count_word(&mut self) {
        self.projected_pc += 1;
    }

    pub(crate) fn count_words(&mut self, words: usize) {
        self.projected_pc += words;
    }

    pub(crate) fn count_instr(&mut self) {
        self.count_word();
    }

    pub(crate) fn count_instr_with_ext(&mut self, ext_words: usize) {
        self.count_words(1 + ext_words);
    }

    pub(crate) fn count_load_const(&mut self) {
        self.alloc_reg();
        self.count_instr();
    }

    pub(crate) fn count_load_var(&mut self) {
        self.alloc_reg();
        self.count_instr();
    }

    pub(crate) fn count_ic_instr_with_ext(&mut self) {
        self.count_instr_with_ext(3);
    }

    pub(crate) fn count_ic_set_with_ext(&mut self) {
        self.count_ic_instr_with_ext();
    }

    pub(crate) fn count_call_instr_with_arg_ext(&mut self) {
        self.count_instr_with_ext(1);
    }

    pub(crate) fn count_delete_static(&mut self) {
        self.count_instr_with_ext(1);
    }

    pub(crate) fn count_define_accessor(&mut self) {
        self.count_instr_with_ext(1);
    }

    pub(crate) fn count_private_access(&mut self) {
        self.count_load_const();
        self.alloc_reg();
        self.count_instr();
    }

    pub(crate) fn count_template_str(&mut self, segment_count: usize) {
        self.alloc_reg();
        self.count_instr_with_ext(1 + segment_count);
    }

    pub(crate) fn count_jump(&mut self) {
        self.count_instr();
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
        self.projected_pc = 0;
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

    pub(crate) fn checked_jump_offset(&mut self, offset: isize) -> i16 {
        if offset < i16::MIN as isize || offset > i16::MAX as isize {
            self.jump_overflow = true;
            0
        } else {
            offset as i16
        }
    }

    pub(crate) fn resolve_label(&self, label: Label) -> Result<usize, String> {
        self.labels
            .label_map
            .get(&label)
            .copied()
            .ok_or_else(|| format!("Label {:?} not found in bytecode map", label))
    }

    pub(crate) fn resolve_fixups(&mut self) -> Result<(), String> {
        let fixups = std::mem::take(&mut self.fixups);
        for fixup in fixups {
            let target_pc = self.resolve_label(fixup.label)?;
            let offset = self.checked_jump_offset(target_pc as isize - fixup.pc as isize);
            self.bytecode[fixup.pc] = match fixup.opcode {
                OpCode::JMP => opcode::encode_jmp(offset),
                OpCode::JMP_IF_FALSE => opcode::encode_jmp_if_false(fixup.rd, offset),
                OpCode::JMP_IF_TRUE => opcode::encode_jmp_if_true(fixup.rd, offset),
                OpCode::JMP_IF_NULLISH => opcode::encode_jmp_if_nullish(fixup.rd, offset),
                OpCode::TRY_BEGIN => opcode::encode_try_begin(offset),
                _ => return Err(format!("Unsupported fixup opcode {:?}", fixup.opcode)),
            };
        }
        Ok(())
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

    #[allow(dead_code)]
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

    pub(crate) fn push_loop(&mut self, break_label: Label, continue_label: Label) {
        self.labels.loop_stack.push((break_label, continue_label));
    }

    pub(crate) fn pop_loop(&mut self) {
        self.labels.loop_stack.pop();
    }

    pub(crate) fn current_loop(&self) -> Option<&(Label, Label)> {
        self.labels.loop_stack.last()
    }

    pub(crate) fn push_switch(&mut self, break_label: Label) {
        self.labels.switch_stack.push(break_label);
    }

    pub(crate) fn pop_switch(&mut self) {
        self.labels.switch_stack.pop();
    }

    pub(crate) fn current_switch(&self) -> Option<&Label> {
        self.labels.switch_stack.last()
    }

    pub(crate) fn push_label_scope(
        &mut self, name: &str, break_label: Label, continue_label: Option<Label>,
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
    pub(crate) fn take_pending_loop_labels(&mut self, break_label: Label, continue_label: Label) -> usize {
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

    #[allow(dead_code)]
    pub(crate) fn analyze_upvalue_captures(
        &self, body_stmts: &[Statement], parent_ctx: &CompileCtx, nested_symbols: &SymbolTable,
    ) -> (Vec<UpvalueCapture>, u8) {
        let mut captures: Vec<UpvalueCapture> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();

        for stmt in body_stmts {
            self.collect_upvalue_stmt(stmt, parent_ctx, nested_symbols, &mut captures, &mut seen);
        }

        let count = captures.len() as u8;
        (captures, count)
    }

    #[allow(dead_code)]
    fn collect_upvalue_stmt(
        &self, stmt: &Statement, parent_ctx: &CompileCtx, nested_symbols: &SymbolTable,
        captures: &mut Vec<UpvalueCapture>, seen: &mut HashMap<String, usize>,
    ) {
        match stmt {
            Statement::ExpressionStatement(es) => {
                self.collect_upvalue_expr(&es.expression, parent_ctx, nested_symbols, captures, seen);
            }
            Statement::VariableDeclaration(vd) => {
                for decl in &vd.declarations {
                    if let Some(init) = &decl.init {
                        self.collect_upvalue_expr(init, parent_ctx, nested_symbols, captures, seen);
                    }
                }
            }
            Statement::ReturnStatement(rs) => {
                if let Some(expr) = &rs.argument {
                    self.collect_upvalue_expr(expr, parent_ctx, nested_symbols, captures, seen);
                }
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.collect_upvalue_expr(e, parent_ctx, nested_symbols, captures, seen);
                    }
                }
                if let Some(test) = &fs.test {
                    self.collect_upvalue_expr(test, parent_ctx, nested_symbols, captures, seen);
                }
                if let Some(update) = &fs.update {
                    self.collect_upvalue_expr(update, parent_ctx, nested_symbols, captures, seen);
                }
                self.collect_upvalue_stmt(&fs.body, parent_ctx, nested_symbols, captures, seen);
            }
            Statement::IfStatement(is) => {
                self.collect_upvalue_expr(&is.test, parent_ctx, nested_symbols, captures, seen);
                self.collect_upvalue_stmt(&is.consequent, parent_ctx, nested_symbols, captures, seen);
                if let Some(alt) = &is.alternate {
                    self.collect_upvalue_stmt(alt, parent_ctx, nested_symbols, captures, seen);
                }
            }
            Statement::WhileStatement(ws) => {
                self.collect_upvalue_expr(&ws.test, parent_ctx, nested_symbols, captures, seen);
                self.collect_upvalue_stmt(&ws.body, parent_ctx, nested_symbols, captures, seen);
            }
            Statement::DoWhileStatement(dw) => {
                self.collect_upvalue_stmt(&dw.body, parent_ctx, nested_symbols, captures, seen);
                self.collect_upvalue_expr(&dw.test, parent_ctx, nested_symbols, captures, seen);
            }
            Statement::BlockStatement(bs) => {
                for s in &bs.body {
                    self.collect_upvalue_stmt(s, parent_ctx, nested_symbols, captures, seen);
                }
            }
            Statement::TryStatement(ts) => {
                for s in &ts.block.body {
                    self.collect_upvalue_stmt(s, parent_ctx, nested_symbols, captures, seen);
                }
                if let Some(handler) = &ts.handler {
                    for s in &handler.body.body {
                        self.collect_upvalue_stmt(s, parent_ctx, nested_symbols, captures, seen);
                    }
                }
                if let Some(finalizer) = &ts.finalizer {
                    for s in &finalizer.body {
                        self.collect_upvalue_stmt(s, parent_ctx, nested_symbols, captures, seen);
                    }
                }
            }
            Statement::ThrowStatement(ts) => {
                self.collect_upvalue_expr(&ts.argument, parent_ctx, nested_symbols, captures, seen);
            }
            Statement::SwitchStatement(ss) => {
                self.collect_upvalue_expr(&ss.discriminant, parent_ctx, nested_symbols, captures, seen);
                for case in &ss.cases {
                    for s in &case.consequent {
                        self.collect_upvalue_stmt(s, parent_ctx, nested_symbols, captures, seen);
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(dead_code)]
    fn collect_upvalue_expr(
        &self, expr: &Expression, parent_ctx: &CompileCtx, nested_symbols: &SymbolTable,
        captures: &mut Vec<UpvalueCapture>, seen: &mut HashMap<String, usize>,
    ) {
        match expr {
            Expression::Identifier(ident) => {
                let name = ident.name.as_str();
                if nested_symbols.lookup_any_binding(name).is_some() {
                    // Check if this is a parent function-scope binding (real upvalue)
                    let is_parent_func_var = parent_ctx
                        .scopes
                        .symbols
                        .scopes
                        .iter()
                        .skip(1)
                        .any(|s| s.bindings.contains_key(name));
                    if !is_parent_func_var {
                        return; // true local or global, not upvalue
                    }
                    // falls through to capture
                }
                if let Some((binding, _)) = parent_ctx.scopes.symbols.lookup_any_binding(name) {
                    if seen.contains_key(name) {
                        return;
                    }
                    let cell_idx = captures.len() as u8;
                    seen.insert(name.to_string(), cell_idx as usize);
                    captures.push(UpvalueCapture {
                        name: name.to_string(),
                        enclosing_reg: binding.reg,
                        cell_idx,
                    });
                }
            }
            Expression::AssignmentExpression(ae) => {
                if let oxide_parser::AssignmentTarget::AssignmentTargetIdentifier(ati) = &ae.left {
                    let name = ati.name.as_str();
                    let in_nested = nested_symbols.lookup_any_binding(name).is_some();
                    let is_parent_func_var = parent_ctx
                        .scopes
                        .symbols
                        .scopes
                        .iter()
                        .skip(1)
                        .any(|s| s.bindings.contains_key(name));
                    if !in_nested || is_parent_func_var {
                        if let Some((binding, _)) = parent_ctx.scopes.symbols.lookup_any_binding(name) {
                            if !seen.contains_key(name) {
                                let cell_idx = captures.len() as u8;
                                seen.insert(name.to_string(), cell_idx as usize);
                                captures.push(UpvalueCapture {
                                    name: name.to_string(),
                                    enclosing_reg: binding.reg,
                                    cell_idx,
                                });
                            }
                        }
                    }
                }
                self.collect_upvalue_expr(&ae.right, parent_ctx, nested_symbols, captures, seen);
            }
            Expression::BinaryExpression(be) => {
                self.collect_upvalue_expr(&be.left, parent_ctx, nested_symbols, captures, seen);
                self.collect_upvalue_expr(&be.right, parent_ctx, nested_symbols, captures, seen);
            }
            Expression::UnaryExpression(ue) => {
                self.collect_upvalue_expr(&ue.argument, parent_ctx, nested_symbols, captures, seen);
            }
            Expression::UpdateExpression(ue) => {
                if let oxide_parser::SimpleAssignmentTarget::AssignmentTargetIdentifier(ati) = &ue.argument {
                    let name = ati.name.as_str();
                    let in_nested = nested_symbols.lookup_any_binding(name).is_some();
                    let is_parent_func_var = parent_ctx
                        .scopes
                        .symbols
                        .scopes
                        .iter()
                        .skip(1)
                        .any(|s| s.bindings.contains_key(name));
                    if !in_nested || is_parent_func_var {
                        if let Some((binding, _)) = parent_ctx.scopes.symbols.lookup_any_binding(name) {
                            if !seen.contains_key(name) {
                                let cell_idx = captures.len() as u8;
                                seen.insert(name.to_string(), cell_idx as usize);
                                captures.push(UpvalueCapture {
                                    name: name.to_string(),
                                    enclosing_reg: binding.reg,
                                    cell_idx,
                                });
                            }
                        }
                    }
                }
            }
            Expression::CallExpression(ce) => {
                self.collect_upvalue_expr(&ce.callee, parent_ctx, nested_symbols, captures, seen);
                for arg in &ce.arguments {
                    if let Some(e) = arg.as_expression() {
                        self.collect_upvalue_expr(e, parent_ctx, nested_symbols, captures, seen);
                    }
                }
            }
            Expression::SequenceExpression(se) => {
                for sub_expr in &se.expressions {
                    self.collect_upvalue_expr(sub_expr, parent_ctx, nested_symbols, captures, seen);
                }
            }
            Expression::ConditionalExpression(ce) => {
                self.collect_upvalue_expr(&ce.test, parent_ctx, nested_symbols, captures, seen);
                self.collect_upvalue_expr(&ce.consequent, parent_ctx, nested_symbols, captures, seen);
                self.collect_upvalue_expr(&ce.alternate, parent_ctx, nested_symbols, captures, seen);
            }
            Expression::ArrayExpression(ae) => {
                for elem in &ae.elements {
                    if let Some(e) = elem.as_expression() {
                        self.collect_upvalue_expr(e, parent_ctx, nested_symbols, captures, seen);
                    }
                }
            }
            _ => {}
        }
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
    ) -> Result<CompiledModule, String> {
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
    ) -> Result<CompiledModule, String> {
        self.compile_function_body_with_field_hooks(
            param_specs,
            body_stmts,
            parent_ctx,
            is_expression_body,
            extra_bindings,
            body_context,
            None::<fn(&Compiler, &mut CompileCtx)>,
            None::<fn(&Compiler, &mut CompileCtx) -> Result<(), String>>,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn pre_scan_function_expressions(&self, stmts: &[Statement], parent_ctx: &mut CompileCtx) -> Result<(), String> {
        for stmt in stmts {
            self.pre_scan_stmt(stmt, parent_ctx)?;
        }
        Ok(())
    }

    fn pre_scan_stmt(&self, stmt: &Statement, parent_ctx: &mut CompileCtx) -> Result<(), String> {
        match stmt {
            Statement::ExpressionStatement(es) => self.pre_scan_expr(&es.expression, parent_ctx)?,
            Statement::ReturnStatement(rs) => {
                if let Some(a) = &rs.argument {
                    self.pre_scan_expr(a, parent_ctx)?;
                }
            }
            Statement::IfStatement(is) => {
                self.pre_scan_expr(&is.test, parent_ctx)?;
                self.pre_scan_stmt(&is.consequent, parent_ctx)?;
                if let Some(alt) = &is.alternate {
                    self.pre_scan_stmt(alt, parent_ctx)?;
                }
            }
            Statement::ForStatement(fs) => {
                if let Some(init) = &fs.init {
                    if let Some(e) = init.as_expression() {
                        self.pre_scan_expr(e, parent_ctx)?;
                    }
                }
                if let Some(t) = &fs.test {
                    self.pre_scan_expr(t, parent_ctx)?;
                }
                if let Some(u) = &fs.update {
                    self.pre_scan_expr(u, parent_ctx)?;
                }
                self.pre_scan_stmt(&fs.body, parent_ctx)?;
            }
            Statement::BlockStatement(bs) => self.pre_scan_function_expressions(&bs.body, parent_ctx)?,
            _ => {}
        }
        Ok(())
    }

    fn pre_scan_expr(&self, expr: &Expression, parent_ctx: &mut CompileCtx) -> Result<(), String> {
        match expr {
            Expression::FunctionExpression(fe) => {
                let body: &[Statement] = fe.body.as_ref().map(|b| &b.statements[..]).unwrap_or(&[]);
                let nested_symbols = SymbolTable::new();
                let (captures, _) = self.analyze_upvalue_captures(body, parent_ctx, &nested_symbols);
                for up in &captures {
                    if let Some((binding, _)) = parent_ctx.scopes.symbols.lookup_any_binding(&up.name) {
                        binding.is_captured.set(true);
                    }
                }
            }
            Expression::ArrowFunctionExpression(_ae) => {}
            Expression::CallExpression(ce) => {
                self.pre_scan_expr(&ce.callee, parent_ctx)?;
                for arg in &ce.arguments {
                    if let Some(e) = arg.as_expression() {
                        self.pre_scan_expr(e, parent_ctx)?;
                    }
                }
            }
            Expression::BinaryExpression(be) => {
                self.pre_scan_expr(&be.left, parent_ctx)?;
                self.pre_scan_expr(&be.right, parent_ctx)?;
            }
            Expression::ConditionalExpression(ce) => {
                self.pre_scan_expr(&ce.test, parent_ctx)?;
                self.pre_scan_expr(&ce.consequent, parent_ctx)?;
                self.pre_scan_expr(&ce.alternate, parent_ctx)?;
            }
            Expression::ArrayExpression(ae) => {
                for e in &ae.elements {
                    if let Some(e) = e.as_expression() {
                        self.pre_scan_expr(e, parent_ctx)?;
                    }
                }
            }
            Expression::SequenceExpression(se) => {
                for e in &se.expressions {
                    self.pre_scan_expr(e, parent_ctx)?;
                }
            }
            Expression::AssignmentExpression(ae) => {
                self.pre_scan_expr(&ae.right, parent_ctx)?;
            }
            Expression::UnaryExpression(ue) => {
                self.pre_scan_expr(&ue.argument, parent_ctx)?;
            }
            _ => {}
        }
        Ok(())
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compile_function_body_with_field_hooks<'a, C, E>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, extra_bindings: &[(&str, u8)], body_context: FunctionBodyContext,
        _count_fields: Option<C>, mut emit_fields: Option<E>, fields_after_super: bool,
    ) -> Result<CompiledModule, String>
    where
        C: FnMut(&Compiler, &mut CompileCtx),
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
                    is_captured: binding.is_captured.clone(),
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
                    is_captured: Cell::new(false),
                },
            );
            inherited_reg_start = inherited_reg_start.max(reg.saturating_add(1));
        }
        ctx.reserved_reg_start = inherited_reg_start.max(1);

        // Align next_reg with builtin count so both count and emit passes start at the
        // same register offset (params go after builtin slots).
        ctx.reset_regs();

        // Free variable analysis for upvalue capture (Ordinary + Arrow functions only)
        if matches!(body_context, FunctionBodyContext::Ordinary | FunctionBodyContext::Arrow) {
            let (captures, _cells) = self.analyze_upvalue_captures(body_stmts, parent_ctx, &ctx.scopes.symbols);
            ctx.current_upvalue_captures = captures;
            ctx.scopes.cell_registry = ctx
                .current_upvalue_captures
                .iter()
                .map(|u| (u.name.clone(), u.cell_idx))
                .collect();
            for up in &ctx.current_upvalue_captures {
                if let Some((binding, _)) = parent_ctx.scopes.symbols.lookup_any_binding(&up.name) {
                    binding.is_captured.set(true);
                }
            }
        }

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

        self.predeclare_function_declarations(body_stmts, &mut ctx);

        // Pre-scan: run analysis for nested function expressions to mark parent captures
        self.pre_scan_function_expressions(body_stmts, &mut ctx)?;

        if let Some(emit) = emit_fields.as_mut() {
            if fields_after_super {
                let start = ctx.bytecode.len();
                emit(self, &mut ctx)?;
                let field_code = ctx.bytecode.split_off(start);
                ctx.after_super_insert = Some(field_code);
                ctx.after_super_inserted = false;
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

        ctx.resolve_fixups()?;

        // Emit implicit RETURN: expression body returns the last expression,
        // statement body returns undefined.
        if is_expression_body {
            if let Some(reg) = last_result_reg {
                ctx.emit(opcode::encode(OpCode::RETURN, reg, 0, 0));
            } else {
                let undef_idx = ctx.add_constant(Constant::Undefined);
                let undef_reg = ctx.alloc_reg();
                ctx.emit(opcode::encode(
                    OpCode::LOAD_CONST,
                    undef_reg,
                    (undef_idx & 0xFF) as u8,
                    ((undef_idx >> 8) & 0xFF) as u8,
                ));
                ctx.emit(opcode::encode(OpCode::RETURN, undef_reg, 0, 0));
            }
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            let undef_reg = ctx.alloc_reg();
            ctx.emit(opcode::encode(
                OpCode::LOAD_CONST,
                undef_reg,
                (undef_idx & 0xFF) as u8,
                ((undef_idx >> 8) & 0xFF) as u8,
            ));
            ctx.emit(opcode::encode(OpCode::RETURN, undef_reg, 0, 0));
        }

        if ctx.reg_overflow {
            return Err("RangeError: function body uses too many registers (max 253)".into());
        }
        if ctx.const_overflow {
            return Err("RangeError: too many constants".into());
        }
        if ctx.jump_overflow {
            return Err("RangeError: jump offset out of range".into());
        }

        Ok(CompiledModule {
            bytecode: ctx.bytecode,
            constants: ctx.constants,
            n_registers: ctx.max_regs,
            n_args: param_specs.len() as u8,
            param_base,
            builtin_reg_map: ctx.scopes.builtin_reg_map,
            sub_modules: ctx.sub_modules,
            is_arrow: false,
            captured_this_const_idx: 0,
            function_name: None,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
            upvalue_captures: ctx.current_upvalue_captures.clone(),
            cells_needed: ctx
                .scopes
                .symbols
                .scopes
                .iter()
                .flat_map(|s| s.bindings.values())
                .filter(|b| b.is_captured.get())
                .count() as u8,
        })
    }

    pub(crate) fn count_statement(&self, stmt: &Statement, ctx: &mut CompileCtx) {
        match stmt {
            Statement::BreakStatement(_) | Statement::ContinueStatement(_) | Statement::LabeledStatement(_) => {
                self.count_basic(stmt, ctx)
            }
            Statement::IfStatement(_) => self.count_control_domain(stmt, ctx),
            Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_) => self.count_iteration_domain(stmt, ctx),
            Statement::SwitchStatement(_) => self.count_switch_domain(stmt, ctx),
            Statement::TryStatement(_) => self.count_exception_domain(stmt, ctx),
            _ => {}
        }
    }

    pub(crate) fn count_expression(&self, expr: &Expression, ctx: &mut CompileCtx) {
        match expr {
            Expression::BinaryExpression(_)
            | Expression::PrivateInExpression(_)
            | Expression::UnaryExpression(_)
            | Expression::UpdateExpression(_) => self.count_operator(expr, ctx),
            Expression::CallExpression(_) | Expression::NewExpression(_) => self.count_call_domain(expr, ctx),
            Expression::AssignmentExpression(_) => self.count_assignment(expr, ctx),
            Expression::ConditionalExpression(_)
            | Expression::SequenceExpression(_)
            | Expression::LogicalExpression(_) => self.count_conditional_chain(expr, ctx),
            Expression::ChainExpression(_) => self.count_chain_expression(expr, ctx),
            Expression::ObjectExpression(_) | Expression::ArrayExpression(_) => self.count_object_domain(expr, ctx),
            Expression::TemplateLiteral(_) | Expression::TaggedTemplateExpression(_) => {
                self.count_template_domain(expr, ctx)
            }
            Expression::ArrowFunctionExpression(_)
            | Expression::FunctionExpression(_)
            | Expression::ClassExpression(_) => self.count_function_domain(expr, ctx),
            Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_) => self.count_member_domain(expr, ctx),
            Expression::ParenthesizedExpression(_) => self.count_parenthesized_expression(expr, ctx),
            Expression::ThisExpression(_) => self.count_this_expression(ctx),
            Expression::Identifier(_) => self.count_identifier_expression(expr, ctx),
            Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::RegExpLiteral(_) => self.count_literal(expr, ctx),
            _ => self.count_default_expression(ctx),
        }
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
        ctx.resolve_fixups()?;
        crate::compiler_debug!("emitter: {} bytes emitted", ctx.bytecode.len());

        if let Some(r) = last_result {
            ctx.emit(opcode::encode(OpCode::LOAD_VAR, 0, r, 0));
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.emit(opcode::encode(
                OpCode::LOAD_CONST,
                0,
                (undef_idx & 0xFF) as u8,
                ((undef_idx >> 8) & 0xFF) as u8,
            ));
        }
        ctx.emit(opcode::encode(OpCode::HALT, 0, 0, 0));

        if ctx.reg_overflow {
            return Err("RangeError: function body uses too many registers (max 253)".into());
        }
        if ctx.const_overflow {
            return Err("RangeError: too many constants".into());
        }
        if ctx.jump_overflow {
            return Err("RangeError: jump offset out of range".into());
        }

        crate::compiler_debug!("compile: done, {} instructions, {} constants", ctx.bytecode.len(), ctx.constants.len());

        Ok(CompiledModule {
            bytecode: ctx.bytecode,
            constants: ctx.constants,
            n_registers: ctx.max_regs,
            n_args: 0,
            param_base: ctx.scopes.builtin_reg_map.len() as u8,
            builtin_reg_map: ctx.scopes.builtin_reg_map,
            sub_modules: ctx.sub_modules,
            is_arrow: false,
            captured_this_const_idx: 0,
            function_name: None,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
            upvalue_captures: ctx.current_upvalue_captures.clone(),
            cells_needed: ctx
                .scopes
                .symbols
                .scopes
                .iter()
                .flat_map(|s| s.bindings.values())
                .filter(|b| b.is_captured.get())
                .count() as u8,
        })
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::CompileCtx;

    #[test]
    fn test_jump_offset_overflow_range_error() {
        let mut ctx = CompileCtx::new();
        let offset = ctx.checked_jump_offset(i16::MAX as isize + 1);
        assert_eq!(offset, 0);
        assert!(ctx.jump_overflow);
    }
}
