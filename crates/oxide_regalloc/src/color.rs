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
    let max_real = graph::collect_real_vregs(f).into_iter().max().unwrap_or(0);
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

    assemble(f, colors, spill_set, fresh, slot_for_vreg, arg_window_base)
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
    f: &IRFunction, colors: BTreeMap<u32, u32>, spill_set: BTreeSet<u32>, fresh: Vec<FreshVreg>,
    slot_for_vreg: BTreeMap<u32, u16>, arg_window_base: u32,
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
        .chain(std::iter::once(f.param_layout.base + f.param_layout.count))
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_bytecode::opcode::OpCode;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;
    use oxide_ir::IRFunction;

    fn empty_function() -> IRFunction {
        IRFunction::new()
    }

    fn color_of(
        insts: Vec<Inst>, param_layout: oxide_ir::ParamLayout, nested: Vec<IRFunction>,
    ) -> Result<AllocMap, String> {
        let mut f = empty_function();
        f.insts = insts;
        f.param_layout = param_layout;
        f.nested = nested;
        let cfg = oxide_cfg::build_cfg(&f);
        let live = oxide_liveness::liveness(&f, &cfg);
        crate::color(&f, &live)
    }

    #[test]
    fn overlapping_vregs_get_distinct_colors() {
        // r1/r2 同时 live（ADD 前）→ 必须不同色；r3（ADD 结果）与二者不相交
        let m = color_of(
            vec![
                Inst::load_const(Operand::Reg(1), 0),
                Inst::load_const(Operand::Reg(2), 0),
                Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
                Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None),
            ],
            oxide_ir::ParamLayout { base: 0, count: 0 },
            Vec::new(),
        )
        .unwrap();
        assert_ne!(m.map[&1], m.map[&2], "同时 live 的 r1/r2 必须不同色");
    }

    #[test]
    fn disjoint_live_ranges_share_color() {
        // r1 死于 ADD0，r4 生于 ADD1——活度不相交 → 可同色
        let m = color_of(
            vec![
                Inst::load_const(Operand::Reg(1), 0),
                Inst::load_const(Operand::Reg(2), 0),
                Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
                Inst::load_const(Operand::Reg(4), 0),
                Inst::load_const(Operand::Reg(5), 0),
                Inst::new(OpCode::ADD, Operand::Reg(6), Operand::Reg(4), Operand::Reg(5)),
                Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None),
            ],
            oxide_ir::ParamLayout { base: 0, count: 0 },
            Vec::new(),
        )
        .unwrap();
        if let (Alloc::Phys(c1), Alloc::Phys(c4)) = (m.map[&1], m.map[&4]) {
            assert_eq!(c1, c4, "不相交活度应共享颜色");
        } else {
            panic!("r1/r4 应为 Phys");
        }
    }

    #[test]
    fn param_segment_keeps_colors() {
        let m = color_of(
            vec![
                Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)),
                Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None),
            ],
            oxide_ir::ParamLayout { base: 1, count: 2 },
            Vec::new(),
        )
        .unwrap();
        assert_eq!(m.map[&1], Alloc::Phys(1));
        assert_eq!(m.map[&2], Alloc::Phys(2));
        assert_eq!(m.map.values().filter(|&&a| a == Alloc::Phys(1)).count(), 1);
        assert_eq!(m.map.values().filter(|&&a| a == Alloc::Phys(2)).count(), 1);
    }

    #[test]
    fn escaped_vreg_keeps_color_and_not_spilled() {
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(5), Operand::Reg(3), Operand::Reg(4)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        let mut sub = empty_function();
        sub.insts
            .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(9), Operand::Reg(3), Operand::None));
        f.nested.push(sub);
        let cfg = oxide_cfg::build_cfg(&f);
        let live = oxide_liveness::liveness(&f, &cfg);
        let m = crate::color(&f, &live).unwrap();
        assert_eq!(m.map[&3], Alloc::Phys(3), "escaped 槽保持原色");
        assert!(m.spills.iter().all(|s| s.vreg != 3), "escaped vreg 不得被 spill");
    }

    #[test]
    fn coloring_is_deterministic() {
        let src_insts = vec![
            Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(0), Operand::Reg(1)),
            Inst::new(OpCode::RETURN, Operand::Reg(2), Operand::None, Operand::None),
        ];
        let a = color_of(src_insts.clone(), oxide_ir::ParamLayout { base: 0, count: 0 }, Vec::new()).unwrap();
        let b = color_of(src_insts, oxide_ir::ParamLayout { base: 0, count: 0 }, Vec::new()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn spill_split_recolors_fresh() {
        // 长活 L + 254 短活 B_i（每个 B_i 与 L 在同一 ADD 指令 live）：
        // L 的 degree = 254 ≥ k=253 → kemp 卡住 → spill L → 各 def/use 点拆成单点 fresh →
        // fresh 全部单点活度 < k → 重染色成功。这是"成功 spill"的标准场景。
        let mut insts = Vec::new();
        insts.push(Inst::load_const(Operand::Reg(1000), 0)); // L
        for i in 0..254u32 {
            insts.push(Inst::load_const(Operand::Reg(2000 + i), 0)); // B_i
            insts.push(Inst::new(OpCode::ADD, Operand::Reg(3000 + i), Operand::Reg(2000 + i), Operand::Reg(1000)));
        }
        insts.push(Inst::new(OpCode::RETURN, Operand::Reg(1000), Operand::None, Operand::None));
        let m = color_of(insts, oxide_ir::ParamLayout { base: 0, count: 0 }, Vec::new()).unwrap();
        assert!(!m.spills.is_empty(), "长活 L 应被 spill");
        assert!(m.spills.iter().any(|s| s.vreg == 1000), "spill 的是 L");
        assert!(m.phys_peak <= 253, "phys_peak ≤ 253");
        // 全部 fresh 应有 Phys 分配
        let fresh_phys: Vec<&Alloc> = m.map.values().filter(|a| matches!(a, Alloc::Phys(_))).collect();
        assert!(!fresh_phys.is_empty(), "fresh vreg 应着色为 Phys");
    }

    #[test]
    fn uncolorable_returns_error() {
        // 255 参数 CALL：nargs=255 → arg_window_base=0 → k=0 → 全部 spill → fresh 失败 → Err
        let insts = vec![
            Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3000), 255),
            Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None),
        ];
        let result = color_of(insts, oxide_ir::ParamLayout { base: 0, count: 0 }, Vec::new());
        assert!(result.is_err(), "单点活度 ≥ k 应报 Err");
        assert!(result.unwrap_err().contains("too many registers"), "Err 消息应含 too many registers");
    }
}
