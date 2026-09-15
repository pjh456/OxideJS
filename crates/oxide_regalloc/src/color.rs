//! 染色：Kemp 简化 + 贪心选色 + spill 区间拆分循环 → AllocMap。
//!
//! 主循环：build 干涉图 → Kemp 简化（degree < k 入栈）→ 卡住选 spill 候选 → 被 spill 的
//! vreg 按 def/use 点拆成单点活度 fresh vreg → 重建图重试（外循环固定点）→ 收敛后贪心
//! 选色。fresh 染色失败 = 单点活度 ≥ k = 无可行染色 → Err（RangeError 路径，消息与
//! lower 逐字一致）。
//!
//! 确定性：BTreeMap/BTreeSet + Vec 排序，禁 HashMap。

use std::collections::{BTreeMap, BTreeSet};

use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

use crate::alloc_map::{Alloc, AllocMap, FreshKind, FreshVreg, SpillPlan};
use crate::graph::{self, InterferenceGraph};

/// 主染色循环：产出 AllocMap。
pub(super) fn run(f: &IRFunction, live: &LiveInfo) -> Result<AllocMap, String> {
    let max_param = if f.param_layout.count == 0 {
        0
    } else {
        f.param_layout.base + f.param_layout.count - 1
    };
    let max_real = graph::collect_real_vregs(f).into_iter().max().unwrap_or(0).max(max_param);
    let mut next_fresh_id = max_real + 1;
    let mut next_slot: u16 = 0;
    let mut spill_set: BTreeSet<u32> = BTreeSet::new();
    let mut fresh: Vec<FreshVreg> = Vec::new();
    let mut slot_for_vreg: BTreeMap<u32, u16> = BTreeMap::new();
    let mut iters = 0usize;

    let (colors, arg_window_base): (BTreeMap<u32, u32>, u32) = loop {
        let graph = graph::build(f, live, &spill_set, &fresh);
        let (colors, failed) = kemp_and_select(&graph);
        if failed.is_empty() {
            break (colors, graph.arg_window_base);
        }
        for v in failed {
            if fresh.iter().any(|fr| fr.id == v) {
                // 单点活度 ≥ k：无可行染色（消息与 lower 逐字一致）
                return Err("RangeError: function body uses too many registers (max 253)".into());
            }
            // 真实 vreg 溢出：按 def/use 点拆成单点活度 fresh，重建图重试
            spill_set.insert(v);
            debug_assert!(u32::from(next_slot) <= u16::MAX as u32, "spill slot 超 u16 上限");
            slot_for_vreg.insert(v, next_slot);
            next_slot += 1;
            for d in def_points(f, v) {
                fresh.push(FreshVreg {
                    id: next_fresh_id,
                    at: d,
                    kind: FreshKind::Def,
                    owner: v,
                });
                next_fresh_id += 1;
            }
            for u in use_points(f, v) {
                fresh.push(FreshVreg {
                    id: next_fresh_id,
                    at: u,
                    kind: FreshKind::Use,
                    owner: v,
                });
                next_fresh_id += 1;
            }
        }
        iters += 1;
        debug_assert!(iters < max_real as usize * 2 + fresh.len() + 8, "RegAlloc 染色疑似不收敛");
    };

    assemble(colors, spill_set, fresh, slot_for_vreg, arg_window_base)
}

/// def 点：`def_reg(insts[i]) == Some(v)` 的指令下标。
fn def_points(f: &IRFunction, v: u32) -> Vec<usize> {
    f.insts
        .iter()
        .enumerate()
        .filter(|(_, i)| i.def_reg() == Some(v))
        .map(|(i, _)| i)
        .collect()
}

/// use 点：`inst.use_regs()` 含 v 的指令下标（精确 use，非 live_before 活集——
/// 活集包含"仅经过不读取"的指令，会产生多余 UNSPILL）。RMW 指令（如 COMPOUND_ADD
/// rd 读旧值写新值）同时是 def 与 use，此处与 def_points 各建一个 fresh，由 rewrite 判
/// rmw_v 决定用 def-fresh（rewrite.rs RMW 分支）。
fn use_points(f: &IRFunction, v: u32) -> Vec<usize> {
    f.insts
        .iter()
        .enumerate()
        .filter(|(_, i)| i.use_regs().contains(&v))
        .map(|(i, _)| i)
        .collect()
}

/// Kemp 简化 + 贪心选色。返回 (vreg → 色, 无色的节点 = spill 候选)。
fn kemp_and_select(graph: &InterferenceGraph) -> (BTreeMap<u32, u32>, Vec<u32>) {
    let k = graph.k;
    // 工作集 = 未入栈的非预着色节点
    let mut worklist: BTreeSet<u32> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.pre_color.is_none())
        .map(|(v, _)| *v)
        .collect();
    let mut stack: Vec<u32> = Vec::new();
    let mut failed: Vec<u32> = Vec::new();

    loop {
        // 找 degree < k 的非预着色节点入栈
        let found = worklist.iter().copied().find(|v| graph.nodes[v].adj.len() < k);
        match found {
            Some(v) => {
                stack.push(v);
                worklist.remove(&v);
            }
            None => {
                if worklist.is_empty() {
                    break;
                }
                // 卡住：选 spill 候选（degree 最小 / vreg 号最小，确定性）
                let cand = worklist
                    .iter()
                    .map(|v| (graph.nodes[v].adj.len(), *v))
                    .min()
                    .map(|(_, v)| v)
                    .unwrap();
                failed.push(cand);
                worklist.remove(&cand);
            }
        }
    }

    // 选择阶段：预着色节点固定；栈顶弹出取最低可用色
    let mut colors: BTreeMap<u32, u32> = BTreeMap::new();
    for (v, n) in &graph.nodes {
        if let Some(pc) = n.pre_color {
            colors.insert(*v, pc);
        }
    }
    while let Some(v) = stack.pop() {
        let mut used: BTreeSet<u32> = BTreeSet::new();
        for &n in &graph.nodes[&v].adj {
            if let Some(&c) = colors.get(&n) {
                used.insert(c);
            }
        }
        match graph.allocatable.iter().copied().find(|c| !used.contains(c)) {
            Some(c) => {
                colors.insert(v, c);
            }
            None => failed.push(v), // 理论上不触发（degree < k 保证有色）
        }
    }
    (colors, failed)
}

/// 装配 AllocMap：map（真实 + fresh）+ spills 决策表 + phys_peak + arg_window_base。
fn assemble(
    colors: BTreeMap<u32, u32>, spill_set: BTreeSet<u32>, fresh: Vec<FreshVreg>, slot_for_vreg: BTreeMap<u32, u16>,
    arg_window_base: u32,
) -> Result<AllocMap, String> {
    let mut map = BTreeMap::new();
    // 已染色 vreg（含 fresh）→ Phys
    for (v, c) in &colors {
        map.insert(*v, Alloc::Phys(*c));
    }
    // spill 决策表
    let mut spills: Vec<SpillPlan> = Vec::new();
    for v in spill_set {
        let slot = slot_for_vreg[&v];
        // defs = 该 vreg 的 def 点（kind=Def 且 owner==v）
        let defs: Vec<(usize, u32)> = fresh
            .iter()
            .filter(|fr| fr.owner == v && fr.kind == FreshKind::Def)
            .map(|fr| (fr.at, fr.id))
            .collect();
        let uses: Vec<(usize, u32)> = fresh
            .iter()
            .filter(|fr| fr.owner == v && fr.kind == FreshKind::Use)
            .map(|fr| (fr.at, fr.id))
            .collect();
        spills.push(SpillPlan { vreg: v, slot, defs, uses });
    }
    spills.sort_by_key(|s| s.vreg);

    let phys_peak = colors
        .values()
        .copied()
        .chain(std::iter::once(1))
        .max()
        .unwrap_or(1)
        .saturating_add(1)
        .min(254);

    Ok(AllocMap {
        map,
        spills,
        phys_peak,
        arg_window_base,
    })
}
