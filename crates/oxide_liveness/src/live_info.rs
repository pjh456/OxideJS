//! LiveInfo：liveness 分析输出视图（pass 输出，不住进 IR）。
//!
//! 全部 live 集为按 reg 号索引的 bitset（`Vec<bool>`，长度 = reg_count+1，由
//! dataflow 求上界）。四字段语义：
//! - `block_live_in` / `block_live_out`：块级数据流迭代结果（精确 DCE 消费）
//! - `inst_live_before` / `inst_live_after`：逐指令活集（RegAlloc 干涉图消费 live_before）

/// liveness 输出：块级 liveIn/liveOut + 逐指令 live_before/live_after。
#[derive(Debug, Clone)]
pub struct LiveInfo {
    pub block_live_in: Vec<Vec<bool>>,
    pub block_live_out: Vec<Vec<bool>>,
    pub inst_live_before: Vec<Vec<bool>>,
    pub inst_live_after: Vec<Vec<bool>>,
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
