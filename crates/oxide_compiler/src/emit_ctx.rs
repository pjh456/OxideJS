//! Grouped `CompileCtx` sub-contexts.
//!
//! Splits the compiler's central `CompileCtx` so syntax-domain workers borrow
//! only the slice they need: `LabelCtx` for jump-target resolution, `ScopeCtx`
//! for identifier binding. `PatternCtx` is reserved for destructuring work.
//! Execution-stream fields (bytecode, registers, pc, …) stay flat on
//! `CompileCtx`.

use std::collections::HashMap;

use crate::compiler::{Label, LabelScope};
use crate::symbol_table::SymbolTable;

/// Jump-target / labeled-statement resolution state.
pub(crate) struct LabelCtx {
    pub(crate) label_map: HashMap<Label, usize>,
    pub(crate) loop_stack: Vec<(Label, Label)>,
    #[allow(dead_code)]
    pub(crate) switch_stack: Vec<Label>,
    /// Active labeled-statement scopes (resolves `break label` / `continue label`).
    pub(crate) label_scopes: Vec<LabelScope>,
    /// Label names awaiting binding to the next emitted loop's continue target.
    pub(crate) pending_loop_labels: Vec<String>,
    pub(crate) label_counter: u32,
}

/// Identifier-binding state: symbols, builtin registers, private names.
pub(crate) struct ScopeCtx {
    pub(crate) symbols: SymbolTable,
    pub(crate) builtin_reg_map: Vec<(String, u8)>,
    pub(crate) private_name_map: Vec<(String, u32)>,
    pub(crate) next_private_name_id: u32,
}

/// Reserved for destructuring-pattern state (Phase 15). Intentionally empty
/// today; named so future pattern work has a home without touching `ScopeCtx`.
#[allow(dead_code)]
pub(crate) struct PatternCtx;
