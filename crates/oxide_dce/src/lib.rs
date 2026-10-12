//! DCE 改写 pass：IRFunction → 死代码消除后 compact 重建。
//!
//! 改写 pass：`&mut IRFunction` 就地 compact（以指令下标索引 keep 位图，无借用冲突），
//! 三 Pass 各居一模块，便于逐模块独立单测：
//! - `reachability`（Pass A）：块级可达性，消费 `oxide_cfg::build_cfg`，Exception 边保守遍历
//! - `iter_sweep`（Pass B）：全函数 use 计数迭代删除死指令到不动点（连锁删上游写者）
//! - `rebuild`（Pass C）：mark-sweep 一次性重建 insts + label_pos 重映射（不用就地逐删平移）
//!
//! 本 pass 不碰 nested（不递归）、常量池（不清理）、n_registers（不收缩）、
//! label_count（不读不改）。中间产物（keep/use_count）用局部 Vec 显式传参，
//! 不做 struct 状态持有（无共享可变状态约定）。零 unsafe。
//!
//! 精确二轮 `dce_precise`：liveness 驱动的死指令 + 局部死 STORE_VAR 删除，
//! 消费 `oxide_liveness::LiveInfo`，mark-sweep 重建复用 Pass C。
//!
//! 两入口均暴露零删除事实供管线复用：`dce` 零删除时返回 Pass A 的 `Cfg`
//! （`Some`，有删除为 `None`），`dce_precise` 返回 `PreciseOutcome`
//! （`live_reusable` 标记第一轮活集是否仍有效）。

mod dce_log;
mod iter_sweep;
mod precise_sweep;
mod reachability;
mod rebuild;

use oxide_cfg::Cfg;
use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

/// 死代码消除：块级不可达删除 + 全函数 use 计数迭代删除到不动点 + mark-sweep 重建。
///
/// 返回 `Some(Cfg)`：零删除（含空函数早退），insts 未变，Pass A 的 CFG 仍有效，
/// 调用方可直接复用于 liveness（无需重建）。返回 `None`：有删除，insts 已 compact，
/// 旧 CFG 失效，调用方须重建。
pub fn dce(f: &mut IRFunction) -> Option<Cfg> {
    // 空函数无指令可删，直接返回（no-op，f 不变）；退化 CFG 与编译器对空函数自建
    // 的退化形态一致，保持「零删除 → Some」语义。
    if f.insts.is_empty() {
        return Some(oxide_cfg::build_cfg(f));
    }
    // Pass A：块级可达性（消费 build_cfg，Exception 边保守）→ 指令粒度 keep。
    let cfg = oxide_cfg::build_cfg(f);
    let mut keep = reachability::pass_a_reachable(f, &cfg);
    // Pass B：可达存活指令上全函数 use 计数迭代删除到不动点（连锁删上游写者）。
    iter_sweep::pass_b_dead_code(f, &mut keep);
    let dead = keep.iter().filter(|k| !**k).count();
    dce_info!("DCE: removing {} dead instructions", dead);
    // Pass C：mark-sweep 一次性重建 insts + label_pos 重映射。
    // keep 全真时重建为恒等变换（新旧下标相同、label 重映射为恒等），零删除直接跳过。
    if dead > 0 {
        rebuild::pass_c_sweep(f, &keep);
    }
    // 零删除 ⇒ 函数未变 ⇒ Pass A CFG 仍有效（复用守卫与零删除判定同源）。
    if dead == 0 {
        Some(cfg)
    } else {
        None
    }
}

/// 精确二轮结果：`live_reusable` 为第一轮 liveness 是否仍可复用（零删除时真）。
pub struct PreciseOutcome {
    /// 零删除：inst 下标未位移，传入活集仍有效，调用方可复用第一轮 CFG 与活集。
    pub live_reusable: bool,
}

/// 精确二轮：liveness 驱动的死指令 + 局部死 STORE_VAR 删除。
///
/// 消费 oxide_liveness::LiveInfo（RegAlloc 管线传入；若 LiveInfo 已过期，重建 liveness
/// 是调用方 RegAlloc 的职责，本 pass 不重复建分析引擎）。mark-sweep 重建复用既有 Pass C。
/// 返回 `PreciseOutcome`：零删除时 `live_reusable` 为真（函数未变，第一轮活集仍有效）。
pub fn dce_precise(f: &mut IRFunction, live: &LiveInfo) -> PreciseOutcome {
    // 空函数无指令可删，直接返回（no-op，f 不变）；未改任何指令，活集可复用。
    if f.insts.is_empty() {
        return PreciseOutcome { live_reusable: true };
    }
    // 维度守卫：live 与 insts 对齐才可信（删改后的过期 LiveInfo 索引错位会误删活指令）；
    // 拍平后外层长度 = N×words，空 LiveInfo 的 words 为 0（0==N*0 会误判对齐），
    // 须同时判 words > 0。守卫不过时活集必过期，调用方须重建。
    if live.inst_live_after.len() != f.insts.len() * live.words || live.words == 0 {
        return PreciseOutcome { live_reusable: false };
    }
    let mut keep = vec![true; f.insts.len()];
    precise_sweep::pass_dead_with_liveness(f, live, &mut keep);
    // keep 全真时重建为恒等变换，零删除直接跳过；重建条件与快速路径守卫同源（仅 keep 计数）。
    let dead = keep.iter().filter(|k| !**k).count();
    if dead > 0 {
        rebuild::pass_c_sweep(f, &keep);
    }
    // 零删除 ⇒ 函数未变 ⇒ 第一轮活集仍有效（复用守卫与零删除判定同源）。
    PreciseOutcome { live_reusable: dead == 0 }
}
