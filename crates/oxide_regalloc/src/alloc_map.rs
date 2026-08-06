//! AllocMap：寄存器染色决策输出视图（D-01：pass 输出，不住进 IR）。
//!
//! `AllocMap.map` 含全部真实 + fresh vreg 的 Phys/Spill 去向；`spills` 为
//! SPILL/UNSPILL 插入决策表（05-08 rewrite 消费）；`phys_peak` 回写 n_registers；
//! `arg_window_base` 供调用点参数连续性 MOV 补位。BTreeMap 保确定性（B010，禁 HashMap）。

use std::collections::BTreeMap;

/// vreg 最终去向：Phys(物理色) / Spill(spill 栈帧内 slot)。
/// Phys 色域 1..=253（预着色节点可含保留号之外的值，见 graph.rs）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alloc {
    Phys(u32),
    Spill(u16),
}

/// 拆分 fresh vreg 的语义：Def = SPILL 在 def 指令后插入，Use = UNSPILL 在 use 指令前插入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshKind {
    Def,
    Use,
}

/// 单点活度新 vreg（活点 = inst 下标）。owner = 所属被 spill 的 vreg。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshVreg {
    pub id: u32,
    pub at: usize,
    pub kind: FreshKind,
    pub owner: u32,
}

/// 溢出决策表：被 spill 的 vreg 的 def/use 点 + fresh vreg id 对照。
/// defs/uses 两 Vec 按下标升序。05-08 rewrite 消费此结构插 SPILL/UNSPILL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpillPlan {
    pub vreg: u32,
    pub slot: u16,
    pub defs: Vec<(usize, u32)>,
    pub uses: Vec<(usize, u32)>,
}

/// 寄存器分配决策：vreg → Phys/Spill 映射 + spill 决策表 + 物理峰值 + 参数窗口基址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocMap {
    /// 含真实 + fresh 全部 vreg
    pub map: BTreeMap<u32, Alloc>,
    /// spill 决策表（按 vreg 升序）
    pub spills: Vec<SpillPlan>,
    /// 物理峰值（05-08 回写 n_registers，≤253）
    pub phys_peak: u32,
    /// 参数窗口基址（05-08 调用点 MOV 补位，max_nargs=0 时 = 254 无窗口）
    pub arg_window_base: u32,
}

impl AllocMap {
    /// 构造空 AllocMap：全空/置零，等价于 Default。
    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
            spills: Vec::new(),
            phys_peak: 0,
            arg_window_base: 254,
        }
    }
}

impl Default for AllocMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_map_new_is_empty() {
        let m = AllocMap::new();
        assert!(m.map.is_empty());
        assert!(m.spills.is_empty());
        assert_eq!(m.phys_peak, 0);
        assert_eq!(m.arg_window_base, 254);
        assert!(AllocMap::default().map.is_empty());
    }
}
