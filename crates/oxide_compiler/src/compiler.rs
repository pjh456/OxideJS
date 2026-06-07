use std::collections::HashMap;

use crate::module::CompiledModule;
use crate::opcode::{self, OpCode};
use crate::symbol_table::{Binding, SymbolTable};

pub use crate::hash::structural_hash;
pub use crate::module::Constant;
pub use oxide_parser::{AssignmentOperator, BinaryOperator, Expression, Statement, UnaryOperator};

pub struct Compiler;

pub(crate) fn is_int_literal(value: f64) -> bool {
    value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64
}

pub(crate) fn is_side_effect_free(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::NumericLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::Identifier(_)
    )
}

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
    SwitchEnd(u32),
    SwitchCase(u32),
    CatchBody(u32),
    FinallyBody(u32),
    TryEnd(u32),
}

pub(crate) struct CompileCtx {
    pub(crate) bytecode: Vec<opcode::Instr>,
    pub(crate) constants: Vec<Constant>,
    next_reg: u8,
    pub(crate) max_regs: u8,
    symbols: SymbolTable,
    pub(crate) label_map: HashMap<Label, usize>,
    pub(crate) loop_stack: Vec<(Label, Label)>,
    #[allow(dead_code)]
    pub(crate) switch_stack: Vec<Label>,
    pub(crate) label_counter: u32,
    pub(crate) projected_pc: usize,
    pub(crate) builtin_reg_map: Vec<(String, u8)>,
    pub(crate) sub_modules: Vec<CompiledModule>,
}

impl CompileCtx {
    pub(crate) fn new() -> Self {
        Self {
            bytecode: Vec::new(),
            constants: Vec::new(),
            next_reg: 0,
            max_regs: 0,
            symbols: SymbolTable::new(),
            label_map: HashMap::new(),
            loop_stack: Vec::new(),
            switch_stack: Vec::new(),
            label_counter: 0,
            projected_pc: 0,
            builtin_reg_map: Vec::new(),
            sub_modules: Vec::new(),
        }
    }

    pub(crate) fn emit(&mut self, instr: opcode::Instr) {
        self.bytecode.push(instr);
    }

    pub(crate) fn alloc_reg(&mut self) -> u8 {
        let r = self.next_reg;
        self.next_reg = self.next_reg.wrapping_add(1);
        if self.next_reg > self.max_regs {
            self.max_regs = self.next_reg;
        }
        r
    }

    pub(crate) fn reset_regs(&mut self) {
        self.next_reg = self.builtin_reg_map.len() as u8;
        self.projected_pc = 0;
        self.label_counter = 0;
    }

    pub(crate) fn add_constant(&mut self, c: Constant) -> u16 {
        if let Some(idx) = self.constants.iter().position(|x| x == &c) {
            return idx as u16;
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

    pub(crate) fn declare(&mut self, name: &str, reg: u8) -> Result<(), String> {
        self.symbols.declare(name, reg)
    }

    pub(crate) fn declare_initialized(&mut self, name: &str, reg: u8) -> Result<(), String> {
        self.symbols.declare_initialized(name, reg)
    }

    pub(crate) fn lookup(&self, name: &str) -> Result<u8, String> {
        self.symbols.lookup(name)
    }

    pub(crate) fn lookup_or_global(&mut self, name: &str) -> u8 {
        let reg = self.alloc_reg();
        self.symbols.lookup_or_global(name, reg)
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

    pub(crate) fn pre_register_builtins(&mut self) {
        let builtins = [
            "NaN",
            "undefined",
            "Infinity",
            "Object",
            "Array",
            "String",
            "Number",
            "Boolean",
            "Function",
            "Error",
            "Math",
            "JSON",
            "Promise",
            "parseInt",
            "parseFloat",
            "isNaN",
            "isFinite",
        ];
        for name in &builtins {
            let reg = self.alloc_reg();
            self.symbols.pre_register_global(name, reg);
            self.builtin_reg_map.push((name.to_string(), reg));
        }
    }
}

impl Compiler {
    pub fn new() -> Self {
        Self
    }

    /// Compile a function body (used for FD and FE).
    /// This performs both counting and emitting in one pass.
    pub(crate) fn compile_function_body<'a>(
        &self,
        param_names: &[String],
        body_stmts: &[Statement<'a>],
        parent_ctx: &CompileCtx,
    ) -> Result<CompiledModule, String> {
        let mut ctx = CompileCtx::new();

        // Inherit parent's builtin_reg_map so builtin identifiers (Math, Object, etc.)
        // resolve to the correct pre-allocated registers in the sub-module's register file.
        ctx.builtin_reg_map = parent_ctx.builtin_reg_map.clone();

        // Inherit parent's global scope entries so previously-declared function names
        // are visible from within the body.
        for (name, binding) in &parent_ctx.symbols.scopes[0] {
            ctx.symbols.scopes[0].insert(
                name.clone(),
                Binding {
                    reg: binding.reg,
                    initialized: binding.initialized,
                },
            );
        }

        // Align next_reg with builtin count so both count and emit passes start at the
        // same register offset (params go after builtin slots).
        ctx.reset_regs();

        // Function body scope - params and local vars
        ctx.push_scope();

        // Register parameters as initialized.
        for name in param_names {
            let reg = ctx.alloc_reg();
            ctx.declare_initialized(name, reg)?;
        }

        // Count pass
        for stmt in body_stmts {
            self.count_statement(stmt, &mut ctx);
        }
        ctx.max_regs = ctx.max_regs.max(1);
        ctx.reset_regs();

        // Emit pass - reallocate params (same order = same regs after reset)
        for name in param_names {
            let reg = ctx.alloc_reg();
            ctx.declare_initialized(name, reg)?;
        }

        // Emit body statements.
        // After all statements, emit an implicit RETURN undefined.
        for stmt in body_stmts {
            let _ = self.emit_statement(stmt, &mut ctx)?;
        }

        let undef_idx = ctx.add_constant(Constant::Undefined);
        let undef_reg = ctx.alloc_reg();
        ctx.emit(opcode::encode(
            OpCode::LOAD_CONST,
            undef_reg,
            (undef_idx & 0xFF) as u8,
            ((undef_idx >> 8) & 0xFF) as u8,
        ));
        ctx.emit(opcode::encode(OpCode::RETURN, undef_reg, 0, 0));

        Ok(CompiledModule {
            bytecode: ctx.bytecode,
            constants: ctx.constants,
            n_registers: ctx.max_regs,
            n_args: param_names.len() as u8,
            builtin_reg_map: ctx.builtin_reg_map,
            sub_modules: ctx.sub_modules,
        })
    }

    pub fn compile(&self, program: &oxide_parser::Program) -> Result<CompiledModule, String> {
        let mut ctx = CompileCtx::new();
        ctx.pre_register_builtins();

        for stmt in &program.body {
            self.count_statement(stmt, &mut ctx);
        }
        ctx.max_regs = ctx.max_regs.max(1);
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

        Ok(CompiledModule {
            bytecode: ctx.bytecode,
            constants: ctx.constants,
            n_registers: ctx.max_regs,
            n_args: 0,
            builtin_reg_map: ctx.builtin_reg_map,
            sub_modules: ctx.sub_modules,
        })
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
