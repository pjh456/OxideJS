//! Grouped `CompileCtx` sub-contexts.
//!
//! Splits the compiler's central `CompileCtx` so syntax-domain workers borrow
//! only the slice they need: `LabelCtx` for jump-target resolution, `ScopeCtx`
//! for identifier binding. `PatternCtx` is reserved for destructuring work.
//! Execution-stream fields (insts, registers, pc, …) stay flat on
//! `CompileCtx`.

use crate::compiler::LabelScope;
use crate::ir::operand::LabelId;
use crate::symbol_table::SymbolTable;

/// Jump-target / labeled-statement resolution state.
pub(crate) struct LabelCtx {
    /// label id → 指令下标。id 连续递增，Vec 索引即 id；写入前须扩容。
    pub(crate) label_pos: Vec<Option<usize>>,
    pub(crate) loop_stack: Vec<(LabelId, LabelId)>,
    pub(crate) switch_stack: Vec<LabelId>,
    /// Active labeled-statement scopes (resolves `break label` / `continue label`).
    pub(crate) label_scopes: Vec<LabelScope>,
    /// Label names awaiting binding to the next emitted loop's continue target.
    pub(crate) pending_loop_labels: Vec<String>,
    pub(crate) label_counter: u32,
}

impl LabelCtx {
    /// 按 id 扩容后写入 label 定义位置。
    pub(crate) fn set_label_pos(&mut self, id: LabelId, pos: usize) {
        if id as usize >= self.label_pos.len() {
            self.label_pos.resize(id as usize + 1, None);
        }
        self.label_pos[id as usize] = Some(pos);
    }
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
