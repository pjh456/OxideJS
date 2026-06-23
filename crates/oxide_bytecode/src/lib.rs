#![doc = "OxideJS - Shared bytecode protocol and compiled module ABI"]

pub mod module;
pub mod opcode;

pub use module::{CompiledModule, Constant};
pub use opcode::{Instr, OpCode};
