#![doc = "OxideJS - Shared bytecode protocol and compiled module ABI"]

/// 编译产物与常量池 ABI（[`CompiledModule`] / [`Constant`]）。
pub mod module;
/// 操作码表与指令编解码（[`OpCode`] / [`Instr`]）。
pub mod opcode;

/// 重导出 [`module`] 的编译产物与常量池类型。
pub use module::{CompiledModule, Constant};
/// 重导出 [`opcode`] 的操作码与指令类型。
pub use opcode::{Instr, OpCode};
