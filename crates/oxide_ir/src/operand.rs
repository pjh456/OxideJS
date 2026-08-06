//! 类型安全操作数。IR 中不出现裸寄存器下标。
//!
//! - `Reg(u32)`：寄存器，vreg 表示（RegAlloc 后收敛到物理号 ≤253）；254/255 为 VM 保留，语义由 `This`/`NewTarget` 表达
//! - `Const(u16)`：常量池下标
//! - `Label(u32)`：跳转目标（`LabelId`）
//! - `Imm(u16)`：立即数
//! - `This` / `NewTarget`：语义化特殊寄存器，由 lowering 映射物理 254/255（IR 中禁止裸下标）
//! - `None`：操作数槽未使用

/// 统一跳转目标标识。id 连续递增，`label_pos` 的 Vec 索引即 id。
pub type LabelId = u32;

/// 类型安全操作数。IR 中不出现裸寄存器下标，语义见文件头 `//!`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    Reg(u32),
    Const(u16),
    Label(u32),
    Imm(u16),
    This,
    NewTarget,
    None,
}
