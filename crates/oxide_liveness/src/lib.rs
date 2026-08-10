//! liveness 分析 pass：IRFunction + CFG → LiveInfo（只读）。
//!
//! 消费 `oxide_ir::IRFunction`（只读）与 `oxide_cfg::Cfg`（preds/succs + exit 哨兵），
//! 产出 LiveInfo 分析视图（pass 输出不住进 IR）。两阶段：`dataflow`（块级
//! liveIn/liveOut 不动点）→ `ranges`（块内反向扫描逐指令 live 集）。gen/kill 直接消费
//! contract.rs def_reg/use_regs，None→0 / This→254 / NewTarget→255 零重复映射。

mod dataflow;
mod live_info;
mod liveness_log;
mod ranges;

pub use live_info::LiveInfo;

use oxide_cfg::Cfg;
use oxide_ir::IRFunction;

/// 计算 liveness：`&IRFunction + &Cfg → LiveInfo`。纯函数，只读 IR。
/// 空 IRFunction 退化：返回 LiveInfo::new()。
pub fn liveness(f: &IRFunction, cfg: &Cfg) -> LiveInfo {
    if f.insts.is_empty() {
        return LiveInfo::new();
    }
    liveness_debug!("liveness: {} blocks, {} insts", cfg.blocks.len(), f.insts.len());
    let (block_in, block_out, reg_count) = dataflow::block_liveness(f, cfg);
    let (inst_before, inst_after) = ranges::inst_liveness(f, cfg, &block_out, &block_in, reg_count);
    LiveInfo {
        block_live_in: block_in,
        block_live_out: block_out,
        inst_live_before: inst_before,
        inst_live_after: inst_after,
    }
}
