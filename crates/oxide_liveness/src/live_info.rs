//! LiveInfo：liveness 分析输出视图（pass 输出，不住进 IR）。
//!
//! 全部 live 集为按 reg 号索引的 `u64` 位集（长度 = `(reg_count+1 + 63) / 64` 字，
//! 由 dataflow 求上界）。位 r 落在第 `r >> 6` 字的 `r & 63` 位；越界位恒 0（消费方
//! 用 `bitset_get` 越界返回 false 对齐旧 `Vec<bool>::get` 语义）。四字段语义：
//! - `block_live_in` / `block_live_out`：块级数据流迭代结果（精确 DCE 消费）
//! - `inst_live_before` / `inst_live_after`：逐指令活集（RegAlloc 干涉图消费 live_before）

/// liveness 输出：块级 liveIn/liveOut + 逐指令 live_before/live_after。
#[derive(Debug, Clone)]
pub struct LiveInfo {
    pub block_live_in: Vec<Vec<u64>>,
    pub block_live_out: Vec<Vec<u64>>,
    pub inst_live_before: Vec<Vec<u64>>,
    pub inst_live_after: Vec<Vec<u64>>,
}

impl LiveInfo {
    /// 构造空 LiveInfo：全空 vec，等价于 Default。
    pub fn new() -> Self {
        Self {
            block_live_in: Vec::new(),
            block_live_out: Vec::new(),
            inst_live_before: Vec::new(),
            inst_live_after: Vec::new(),
        }
    }
}

impl Default for LiveInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// 查询位集：位 r 置 1 返回 true；越界（r 超出位集容量）返回 false。
/// 语义对齐旧 `Vec<bool>::get(r).copied().unwrap_or(false)`。
pub fn bitset_get(bitset: &[u64], r: usize) -> bool {
    bitset.get(r >> 6).is_some_and(|w| (w >> (r & 63)) & 1 == 1)
}

/// 置位：位 r 写 1。调用方须保证 r 在位集容量内（def/use reg 号 ≤ reg_count 上界）。
pub(crate) fn bitset_set(bitset: &mut [u64], r: usize) {
    bitset[r >> 6] |= 1u64 << (r & 63);
}

/// 清位：位 r 写 0。调用方须保证 r 在位集容量内。
pub(crate) fn bitset_clear(bitset: &mut [u64], r: usize) {
    bitset[r >> 6] &= !(1u64 << (r & 63));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_info_new_is_empty() {
        let info = LiveInfo::new();
        assert!(info.block_live_in.is_empty());
        assert!(info.block_live_out.is_empty());
        assert!(info.inst_live_before.is_empty());
        assert!(info.inst_live_after.is_empty());
        let default = LiveInfo::default();
        assert!(default.block_live_in.is_empty());
        assert!(default.block_live_out.is_empty());
    }
}
