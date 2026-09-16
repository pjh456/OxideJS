//! 内联同步执行核心的堆上快照 `InlineSyncState`；字段登记表宏
//! `inline_core_fields!` 在 `vm_runtime` 模块。

use std::sync::Arc;

use oxide_bytecode::opcode;
use oxide_types::object::Cell;
use oxide_types::value::JsValue;
use smallvec::SmallVec;

use super::frames::{CallFrame, Completion, ForInIter, TryHandler};
use crate::vm_state::ForOfEntry;

/// `call_bytecode_function_inline` 使用的堆分配快照。
/// 放在堆上避免 JS 代码链式同步字节码调用（如 sort 比较器、accessor）时
/// 耗尽 Rust 栈。
pub(crate) struct InlineSyncState {
    /// 寄存器窗口副本：`regs[0..len]`，len ≤ 254（含 RegAlloc 最高合法物理槽
    /// 253）。`regs[254]/[255]` 不在此列，由 `saved_this`/`saved_new_target`
    /// 单独保存（callee 也会重写这两个槽）。
    pub(crate) regs: Box<[JsValue]>,
    pub(crate) saved_this: JsValue,
    pub(crate) saved_new_target: JsValue,
    pub(crate) pc: usize,
    pub(crate) bytecode: Arc<[opcode::Instr]>,
    pub(crate) active_immutables: *const [JsValue],
    pub(crate) active_reg_limit: u8,
    pub(crate) root_reg_limit: u8,
    pub(crate) try_stack: Vec<TryHandler>,
    pub(crate) frames: SmallVec<[CallFrame; 16]>,
    pub(crate) exception_value: Option<JsValue>,
    pub(crate) pending_exception: Option<JsValue>,
    pub(crate) pending_error_kind: Option<&'static str>,
    pub(crate) pending_completion: Option<Completion>,
    pub(crate) for_in_iters: Vec<*mut ForInIter<'static>>,
    pub(crate) for_of_iters: Vec<ForOfEntry>,
    pub(crate) saved_bytecode_stack: Vec<Arc<[opcode::Instr]>>,
    pub(crate) saved_immutables_stack: Vec<*const [JsValue]>,
    pub(crate) save_stack: Vec<JsValue>,
    pub(crate) spill_stack: Vec<JsValue>,
    pub(crate) cell_stack: Vec<Vec<*mut Cell>>,
    pub(crate) inline_callee: Option<JsValue>,
    /// 三个内嵌 dispatch 调度标志（`Vm::generator_dispatch` / `async_dispatch` /
    /// `construct_dispatch`）的属主快照。save 时记录外层值并清零 VM 侧、restore
    /// 时写回：嵌套 state-swap 调用不得继承外层调度上下文，否则其内部构造帧
    /// 弹出后 frames 清空，`do_return` 会把嵌套调用误判为属主内嵌 dispatch 提前
    /// 交付，跳过被调函数剩余字节码（详见 `do_return` 交付条件注释）。
    pub(crate) generator_dispatch: bool,
    pub(crate) async_dispatch: bool,
    pub(crate) construct_dispatch: bool,
    /// inline 目标函数的严格模式标志（内联执行期间写路径的 strict/sloppy 判定
    /// 来源；嵌套内联时随本快照保存/恢复）。
    pub(crate) inline_strict: bool,
    /// inline 起始时 `frames.len()` 基线（嵌套内联时随本快照保存/恢复）。
    pub(crate) inline_frames_base: usize,
    pub(crate) inline_args_base: u32,
    pub(crate) inline_args_count: u16,
    pub(crate) accessor_frame_target_reg: Option<u8>,
    pub(crate) active_flat_id: u32,
    pub(crate) active_table_gen: u32,
}
