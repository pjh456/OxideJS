//! DCE 改写 pass：IRFunction → 死代码消除后 compact 重建。
//!
//! 改写 pass：`&mut IRFunction` 就地 compact（以指令下标索引 keep 位图，无借用冲突），
//! 三 Pass 各居一模块（照 oxide_cfg 四阶段拆文件先例）：
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

mod dce_log;
mod iter_sweep;
mod precise_sweep;
mod reachability;
mod rebuild;

use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

/// 死代码消除：块级不可达删除 + 全函数 use 计数迭代删除到不动点 + mark-sweep 重建。
pub fn dce(f: &mut IRFunction) {
    // 空 IRFunction 退化形态：无指令可删（照 oxide_cfg empty_function 先例）。
    if f.insts.is_empty() {
        return;
    }
    // Pass A：块级可达性（消费 build_cfg，Exception 边保守）→ 指令粒度 keep。
    let mut keep = reachability::pass_a_reachable(f);
    // Pass B：可达存活指令上全函数 use 计数迭代删除到不动点（连锁删上游写者）。
    iter_sweep::pass_b_dead_code(f, &mut keep);
    let dead = keep.iter().filter(|k| !**k).count();
    dce_info!("DCE: removing {} dead instructions", dead);
    // Pass C：mark-sweep 一次性重建 insts + label_pos 重映射。
    rebuild::pass_c_sweep(f, &keep);
}

/// 精确二轮：liveness 驱动的死指令 + 局部死 STORE_VAR 删除。
///
/// 消费 oxide_liveness::LiveInfo（RegAlloc 管线传入；若 LiveInfo 已过期，重建 liveness
/// 是调用方 RegAlloc 的职责，本 pass 不重复建分析引擎）。mark-sweep 重建复用既有 Pass C。
pub fn dce_precise(f: &mut IRFunction, live: &LiveInfo) {
    // 空函数退化（照 dce 先例）
    if f.insts.is_empty() {
        return;
    }
    // 维度守卫：live 与 insts 对齐才可信（删改后的过期 LiveInfo 索引错位会误删活指令）
    if live.inst_live_after.len() != f.insts.len() {
        return;
    }
    let mut keep = vec![true; f.insts.len()];
    precise_sweep::pass_dead_with_liveness(f, live, &mut keep);
    // mark-sweep 一次性重建
    rebuild::pass_c_sweep(f, &keep);
}
