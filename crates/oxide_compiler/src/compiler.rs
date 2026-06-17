use std::collections::HashMap;

use crate::module::CompiledModule;
use crate::opcode::{self, OpCode};
use crate::symbol_table::{Binding, SymbolTable};

pub use crate::hash::{compiled_module_hash, structural_hash};
pub use crate::module::Constant;
use crate::symbol_table::ScopeKind;
pub use oxide_parser::VariableDeclarationKind;
pub use oxide_parser::{AssignmentOperator, BinaryOperator, Expression, Statement, UnaryOperator};

pub struct Compiler;

pub(crate) fn is_int_literal(value: f64) -> bool {
    value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64
}

pub(crate) fn is_side_effect_free(expr: &Expression) -> bool {
    match expr {
        Expression::NumericLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::Identifier(_)
        | Expression::RegExpLiteral(_)
        | Expression::ThisExpression(_) => true,
        Expression::ParenthesizedExpression(p) => is_side_effect_free(&p.expression),
        Expression::BinaryExpression(bin) => is_side_effect_free(&bin.left) && is_side_effect_free(&bin.right),
        Expression::UnaryExpression(un) => {
            !matches!(un.operator, UnaryOperator::Delete) && is_side_effect_free(&un.argument)
        }
        _ => false,
    }
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
}

pub(crate) struct CompileCtx {
    pub(crate) bytecode: Vec<opcode::Instr>,
    pub(crate) constants: Vec<Constant>,
    constant_map: HashMap<ConstantKey, u16>,
    next_reg: u8,
    pub(crate) max_regs: u8,
    reserved_reg_start: u8,
    symbols: SymbolTable,
    pub(crate) label_map: HashMap<Label, usize>,
    pub(crate) loop_stack: Vec<(Label, Label)>,
    #[allow(dead_code)]
    pub(crate) switch_stack: Vec<Label>,
    pub(crate) label_counter: u32,
    pub(crate) projected_pc: usize,
    pub(crate) builtin_reg_map: Vec<(String, u8)>,
    pub(crate) sub_modules: Vec<CompiledModule>,
    /// Register holding `this` in the enclosing function context.
    /// Used by arrow functions to capture lexical `this` (D-01).
    /// Initialized to 254 (conventional this register) at the top level.
    pub(crate) enclosing_this_reg: u8,
    pub(crate) in_derived_constructor: bool,
    pub(crate) in_instance_method: bool,
    pub(crate) in_static_method: bool,
    pub(crate) static_block_this_reg: Option<u8>,
    pub(crate) private_name_map: Vec<(String, u32)>,
    pub(crate) next_private_name_id: u32,
    pub(crate) after_super_insert: Option<Vec<opcode::Instr>>,
    pub(crate) after_super_inserted: bool,
    /// Set when alloc_reg() overflows into the reserved this/new.target range (≥254).
    /// Checked after each emit phase to produce a compile error rather than silent corruption.
    pub(crate) reg_overflow: bool,
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
    BytecodeFunc(u32),
}

impl CompileCtx {
    pub(crate) fn new() -> Self {
        Self {
            bytecode: Vec::new(),
            constants: Vec::new(),
            constant_map: HashMap::new(),
            next_reg: 1,
            max_regs: 1,
            reserved_reg_start: 1,
            symbols: SymbolTable::new(),
            label_map: HashMap::new(),
            loop_stack: Vec::new(),
            switch_stack: Vec::new(),
            label_counter: 0,
            projected_pc: 0,
            builtin_reg_map: Vec::new(),
            sub_modules: Vec::new(),
            enclosing_this_reg: 254, // conventional this register at top level
            in_derived_constructor: false,
            in_instance_method: false,
            in_static_method: false,
            static_block_this_reg: None,
            private_name_map: Vec::new(),
            next_private_name_id: 1,
            after_super_insert: None,
            after_super_inserted: false,
            reg_overflow: false,
        }
    }

    pub(crate) fn emit(&mut self, instr: opcode::Instr) {
        self.bytecode.push(instr);
    }

    pub(crate) fn emit_load_const(&mut self, reg: u8, idx: u16) {
        self.emit(opcode::encode(OpCode::LOAD_CONST, reg, (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8));
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
        self.label_counter = 0;
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

            let idx = self.constants.len() as u16;
            self.constants.push(c);
            self.constant_map.insert(key, idx);
            return idx;
        }

        let idx = self.constants.len();
        self.constants.push(c);
        idx as u16
    }

    pub(crate) fn resolve_label(&self, label: Label) -> Result<usize, String> {
        self.label_map
            .get(&label)
            .copied()
            .ok_or_else(|| format!("Label {:?} not found in bytecode map", label))
    }

    pub(crate) fn push_scope(&mut self) {
        self.symbols.push_scope();
    }

    pub(crate) fn pop_scope(&mut self) {
        self.symbols.pop_scope();
    }

    pub(crate) fn declare(
        &mut self, name: &str, reg: u8, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.symbols.declare(name, reg, kind, is_const)
    }

    pub(crate) fn declare_initialized(
        &mut self, name: &str, reg: u8, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.symbols.declare_initialized(name, reg, kind, is_const)
    }

    #[allow(dead_code)]
    pub(crate) fn push_scope_with_kind(&mut self, kind: ScopeKind) {
        self.symbols.push_scope_with_kind(kind);
    }

    pub(crate) fn lookup(&self, name: &str) -> Result<u8, String> {
        self.symbols.lookup(name)
    }

    pub(crate) fn lookup_or_builtin(&mut self, name: &str) -> Result<u8, String> {
        match self.symbols.lookup(name) {
            Ok(reg) => Ok(reg),
            Err(err) if Self::is_known_builtin(name) && err.contains("is not defined") => {
                let reg = self.alloc_reg();
                self.symbols.pre_register_global(name, reg);
                self.builtin_reg_map.push((name.to_string(), reg));
                Ok(reg)
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) fn lookup_or_global(&mut self, name: &str) -> u8 {
        if let Some(reg) = self.symbols.lookup_any(name) {
            return reg;
        }
        let reg = self.alloc_reg();
        self.symbols.lookup_or_global(name, reg)
    }

    pub(crate) fn lookup_const_flag(&self, name: &str) -> bool {
        self.symbols.lookup_is_const(name)
    }

    pub(crate) fn init_var(&mut self, name: &str) {
        self.symbols.init_var(name);
    }

    pub(crate) fn next_label_id(&mut self) -> u32 {
        let id = self.label_counter;
        self.label_counter += 1;
        id
    }

    pub(crate) fn push_loop(&mut self, break_label: Label, continue_label: Label) {
        self.loop_stack.push((break_label, continue_label));
    }

    pub(crate) fn pop_loop(&mut self) {
        self.loop_stack.pop();
    }

    pub(crate) fn current_loop(&self) -> Option<&(Label, Label)> {
        self.loop_stack.last()
    }

    pub(crate) fn push_switch(&mut self, break_label: Label) {
        self.switch_stack.push(break_label);
    }

    pub(crate) fn pop_switch(&mut self) {
        self.switch_stack.pop();
    }

    pub(crate) fn current_switch(&self) -> Option<&Label> {
        self.switch_stack.last()
    }

    pub(crate) fn is_builtin(&self, name: &str) -> bool {
        self.builtin_reg_map.iter().any(|(n, _)| n == name)
    }

    pub(crate) fn is_known_builtin(name: &str) -> bool {
        BUILTIN_GLOBALS.contains(&name)
    }

    fn builtin_reg_floor(&self) -> u8 {
        self.builtin_reg_map
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
            Constant::BytecodeFunc(v) => Some(Self::BytecodeFunc(*v)),
            Constant::RegExp(_, _) => None,
        }
    }
}

impl Compiler {
    pub fn new() -> Self {
        Self
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
    pub(crate) fn compile_function_body_with_field_hooks<'a, C, E>(
        &self, param_specs: &[ParamSpec<'a>], body_stmts: &[Statement<'a>], parent_ctx: &CompileCtx,
        is_expression_body: bool, extra_bindings: &[(&str, u8)], body_context: FunctionBodyContext,
        mut count_fields: Option<C>, mut emit_fields: Option<E>, fields_after_super: bool,
    ) -> Result<CompiledModule, String>
    where
        C: FnMut(&Compiler, &mut CompileCtx),
        E: FnMut(&Compiler, &mut CompileCtx) -> Result<(), String>,
    {
        let mut ctx = CompileCtx::new();

        // Inherit parent's builtin_reg_map so builtin identifiers (Math, Object, etc.)
        // resolve to the correct pre-allocated registers in the sub-module's register file.
        ctx.builtin_reg_map = parent_ctx.builtin_reg_map.clone();
        ctx.private_name_map = parent_ctx.private_name_map.clone();
        ctx.next_private_name_id = parent_ctx.next_private_name_id;

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
        for (name, binding) in &parent_ctx.symbols.scopes[0].bindings {
            ctx.symbols.scopes[0].bindings.insert(
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
            ctx.symbols.scopes[0].bindings.insert(
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

        // Register parameters as initialized.
        for spec in param_specs {
            let name = spec.register_name();
            let reg = ctx.alloc_reg();
            ctx.declare_initialized(name, reg, VariableDeclarationKind::Var, false)?;
        }

        // Count pass
        if !fields_after_super {
            if let Some(count) = count_fields.as_mut() {
                count(self, &mut ctx);
            }
        }
        for stmt in body_stmts {
            self.count_statement(stmt, &mut ctx);
        }
        if fields_after_super {
            if let Some(count) = count_fields.as_mut() {
                count(self, &mut ctx);
            }
        }
        ctx.max_regs = ctx.max_regs.max(1);
        ctx.reg_overflow = false;
        ctx.reset_regs();

        // Emit pass - reallocate params (same order = same regs after reset)
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
        // Capture the last statement's expression result for expression-body arrows.
        let mut last_result_reg = None;
        for stmt in body_stmts {
            if let Some(reg) = self.emit_statement(stmt, &mut ctx)? {
                last_result_reg = Some(reg);
            }
        }

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

        Ok(CompiledModule {
            bytecode: ctx.bytecode,
            constants: ctx.constants,
            n_registers: ctx.max_regs,
            n_args: param_specs.len() as u8,
            param_base,
            builtin_reg_map: ctx.builtin_reg_map,
            sub_modules: ctx.sub_modules,
            is_arrow: false,
            captured_this_const_idx: 0,
            function_name: None,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
        })
    }

    pub fn compile(&self, program: &oxide_parser::Program) -> Result<CompiledModule, String> {
        let mut ctx = CompileCtx::new();
        ctx.pre_register_builtins();

        for stmt in &program.body {
            self.count_statement(stmt, &mut ctx);
        }
        ctx.max_regs = ctx.max_regs.max(1);
        ctx.reg_overflow = false;
        ctx.reset_regs();

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

        Ok(CompiledModule {
            bytecode: ctx.bytecode,
            constants: ctx.constants,
            n_registers: ctx.max_regs,
            n_args: 0,
            param_base: ctx.builtin_reg_map.len() as u8,
            builtin_reg_map: ctx.builtin_reg_map,
            sub_modules: ctx.sub_modules,
            is_arrow: false,
            captured_this_const_idx: 0,
            function_name: None,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
        })
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
