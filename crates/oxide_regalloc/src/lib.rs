//! RegAlloc 改写 pass（前半）：干涉图染色决策，产出 AllocMap。
//!
//! 消费 `oxide_liveness::LiveInfo`（inst_live_before 逐指令活集，05-04 交付）与
//! `oxide_ir::IRFunction`（param_layout 参数段 + nested escaped 收集），产出 AllocMap
//! 分析视图（D-01：pass 输出不住进 IR）。染色：干涉图（graph.rs）+ Kemp 简化/贪心/
//! spill 区间拆分（color.rs）。**本模块不改写指令**——05-08 rewrite/finish 消费 AllocMap。

mod alloc_map;
mod color;
mod graph;

pub use alloc_map::{Alloc, AllocMap, FreshKind, FreshVreg, SpillPlan};

use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

/// 染色：`&IRFunction + &LiveInfo → Result<AllocMap, String>`。纯函数，只读 IR。
/// 空 IRFunction 退化：Ok(AllocMap::new())。Err = 无可行染色（D-02 RangeError 路径，
/// 05-08 接线时转 lower 错误）。
pub fn color(f: &IRFunction, live: &LiveInfo) -> Result<AllocMap, String> {
    if f.insts.is_empty() {
        return Ok(AllocMap::new());
    }
    let graph = graph::build(f, live, &std::collections::BTreeSet::new(), &[]);
    color::run(f, live, &graph)
}
