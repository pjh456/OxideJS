#![doc = "OxideJS - Shared kernel (CodeForge, ShapeForge, PropForge, StringForge, BuiltinWorld)"]

pub mod builtin;
pub mod code_forge;
pub mod kernel;
pub mod kernel_log;
pub mod prop_forge;
pub mod shape_forge;
pub mod string_forge;

pub use kernel::{KernelConfig, KernelCore, KernelSession};
