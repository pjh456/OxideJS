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
}

pub struct CompiledModule {
    pub bytecode: Vec<opcode::Instr>,
    pub constants: Vec<Constant>,
    pub n_registers: u8,
    pub builtin_reg_map: Vec<(String, u8)>,
}

impl fmt::Display for CompiledModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "; n_registers = {}", self.n_registers)?;
        writeln!(f, "; constants:")?;
        for (i, c) in self.constants.iter().enumerate() {
            writeln!(f, ";   [{i}] = {:?}", c)?;
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
