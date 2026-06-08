use std::fmt;

use crate::opcode::{self, OpCode};

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Number(f64),
    Int(i32),
    String(String),
    Boolean(bool),
    Null,
    Undefined,
    BytecodeFunc(u32),
    RegExp(String, String),
}

pub struct CompiledModule {
    pub bytecode: Vec<opcode::Instr>,
    pub constants: Vec<Constant>,
    pub n_registers: u8,
    pub n_args: u8,
    pub param_base: u8,
    pub builtin_reg_map: Vec<(String, u8)>,
    pub sub_modules: Vec<CompiledModule>,
    /// True when this module is an arrow function body (D-01).
    /// Arrow functions capture lexical `this` from the enclosing scope.
    pub is_arrow: bool,
    /// Index into `constants` holding the captured `this` JsValue.
    /// 0 means "not captured - use standard this binding".
    pub captured_this_const_idx: u16,
    /// Function name inferred from assignment context (D-04).
    /// Set at the VariableDeclaration / ObjectProperty assignment site.
    pub function_name: Option<String>,
}

impl CompiledModule {
    pub fn new() -> Self {
        Self {
            bytecode: Vec::new(),
            constants: Vec::new(),
            n_registers: 0,
            n_args: 0,
            param_base: 0,
            builtin_reg_map: Vec::new(),
            sub_modules: Vec::new(),
            is_arrow: false,
            captured_this_const_idx: 0,
            function_name: None,
        }
    }
}

impl Default for CompiledModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CompiledModule {
    fn clone(&self) -> Self {
        Self {
            bytecode: self.bytecode.clone(),
            constants: self.constants.clone(),
            n_registers: self.n_registers,
            n_args: self.n_args,
            param_base: self.param_base,
            builtin_reg_map: self.builtin_reg_map.clone(),
            sub_modules: self.sub_modules.clone(),
            is_arrow: self.is_arrow,
            captured_this_const_idx: self.captured_this_const_idx,
            function_name: self.function_name.clone(),
        }
    }
}

impl fmt::Display for CompiledModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "; n_registers = {}", self.n_registers)?;
        writeln!(f, "; constants:")?;
        for (i, c) in self.constants.iter().enumerate() {
            match c {
                Constant::BytecodeFunc(idx) => {
                    writeln!(f, ";   [{i}] = BytecodeFunc(sub_module[{idx}])")?
                }
                other => writeln!(f, ";   [{i}] = {other:?}")?,
            }
        }
        writeln!(f)?;
        for (offset, &instr) in self.bytecode.iter().enumerate() {
            let op = opcode::opcode(instr);
            let rd = opcode::rd(instr);
            let a = opcode::a(instr);
            let b = opcode::b(instr);
            write!(f, "  {offset:04}  {op}")?;
            match op {
                OpCode::LOAD_CONST => {
                    write!(f, " r{rd}, const[{}]", opcode::imm16(instr))?;
                }
                OpCode::JMP | OpCode::JMP_IF_FALSE | OpCode::JMP_IF_TRUE => {
                    write!(f, " r{rd}, {offset:+}", offset = opcode::offset16(instr))?;
                }
                OpCode::SWITCH_TABLE => {
                    let n_cases = rd as u16 | ((b as u16) << 8);
                    write!(f, " r{disc_reg}={a}, {n_cases} cases", disc_reg = a)?;
                }
                OpCode::RETURN | OpCode::HALT | OpCode::NOP => {
                    write!(f, " r{rd}")?;
                }
                OpCode::NEG => {
                    write!(f, " r{rd}, r{a}")?;
                }
                _ => {
                    write!(f, " r{rd}, r{a}, r{b}")?;
                }
            }
            writeln!(f)?;
        }
        Ok(())
    }
}
