//! oxide_emit：AST → IR 代码生成层（编译管线中段）。
//!
//! 入口 `Emitter::emit_program` 把 parser 产出的 `Program` 编译为分域组合的
//! `IRFunction`（code/data/bindings/builtins/closures/meta/nested），再由
//! `oxide_ir::lower` 降为 bytecode。语法按域拆分：`expr/`、`stmt/`、`class/`、
//! `shared/`；作用域与闭包捕获分析在 emit 前完成，消除符号表时序依赖。

mod capture;
pub mod class;
mod compile_ctx;
pub mod emit;
pub mod emit_ctx;
mod emit_log;
mod errors;
pub mod expr;
mod function_body;
pub mod module;
pub mod prepass;
mod program;
pub mod shared;
pub mod stmt;
pub mod symbol_table;

pub use compile_ctx::CompileCtx;
pub use emit::Emitter;
pub use emit::{is_anonymous_function_definition, is_int_literal, is_side_effect_free};
pub use emit_ctx::LabelScope;
pub use function_body::{FunctionBodyContext, ParamSpec};
pub use oxide_bytecode::module::Constant;
pub use oxide_parser::{AssignmentOperator, BinaryOperator, UnaryOperator};
