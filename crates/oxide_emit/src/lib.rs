pub mod class;
pub mod closure;
pub mod emit;
pub mod emit_ctx;
pub mod expr;
pub mod prepass;
pub mod shared;
pub mod stmt;
pub mod symbol_table;

pub use emit::{CompileCtx, Emitter, FunctionBodyContext, LabelScope, ParamSpec};
pub use emit::{is_int_literal, is_side_effect_free};
pub use oxide_bytecode::module::Constant;
pub use oxide_parser::{AssignmentOperator, BinaryOperator, UnaryOperator};
