//! 干涉图构建 + 预着色 + 可分配色集（D-09/D-10/D-11 + RESEARCH 发现 4）。
//!
//! - 真实 vreg：字面 `Operand::Reg(r)`（rd/a/b）+ CALL 系参数连续区间 [first, first+nargs)
//! - 干涉边：逐指令 `live_before[i]` 中同时 live 的真实 vreg 两两加边（fresh vreg 按 at==i 并入）
//! - 预着色：参数段 [param_base, base+count) 钉死原号（VM 调用契约 D-10）；escaped vreg
//!   （nested `LOAD_VAR.a`/`STORE_VAR.rd` 直引父槽）钉死原槽号且排除 spill（B012）
//! - 可分配色集：`(1..=253) − 参数色 − escaped 色 − [arg_window_base, 253]`
//!   （arg_window_base = 254 − max_nargs，为 05-08 调用点 MOV 补位留空槽）
//!
//! 确定性：全 BTreeMap + Vec 排序，禁 HashMap 迭代序（B010）。

use std::collections::{BTreeMap, BTreeSet};

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

use crate::alloc_map::FreshVreg;

/// 干涉图节点：vreg（以 map key 表达）+ 预着色 + 邻接表（排序去重）。
#[derive(Debug, Clone)]
pub(super) struct Node {
    pub pre_color: Option<u32>,
    pub adj: Vec<u32>,
}

/// 干涉图：节点表 + 可分配色集 + 色数 k + 参数窗口基址。
#[derive(Debug, Clone)]
pub(super) struct InterferenceGraph {
    pub nodes: BTreeMap<u32, Node>,
    pub allocatable: Vec<u32>,
    pub k: usize,
    pub arg_window_base: u32,
}

/// 收集全部真实 vreg（BTreeSet 保确定性）：
/// 字面 `Operand::Reg(r)`（rd/a/b 三槽）+ CALL/CALL_NATIVE/NEW_EXPRESSION b 槽参数区间
/// + SUPER_CALL a 槽参数区间。0 无条件排除（reg 0 永非真实——emit alloc_reg 从 ≥1 起）。
///
/// 254/255 的 over-approx 规则：live bitset 中位 254/255 无法区分 This/NewTarget 占用与
/// 真实 vreg 活度，若函数存在该号真实 vreg（大函数 B005 场景）则按真实 vreg 建节点染色——
/// 多出的边只造成过度约束（安全方向），缺失边才会错值。
pub(super) fn collect_real_vregs(f: &IRFunction) -> BTreeSet<u32> {
    let mut real = BTreeSet::new();
    for inst in &f.insts {
        for o in [&inst.rd, &inst.a, &inst.b] {
            if let Operand::Reg(r) = o {
                if *r != 0 {
                    real.insert(*r);
                }
            }
        }
        let nargs = inst.ext.first().copied().unwrap_or(0);
        let first = match inst.op {
            OpCode::CALL | OpCode::CALL_NATIVE | OpCode::NEW_EXPRESSION => match inst.b {
                Operand::Reg(fr) => Some(fr),
                _ => None,
            },
            OpCode::SUPER_CALL => match inst.a {
                Operand::Reg(fr) => Some(fr),
                _ => None,
            },
            _ => None,
        };
        if let Some(fr) = first {
            for r in fr..fr + nargs {
                if r != 0 {
                    real.insert(r);
                }
            }
        }
    }
    real
}

/// 构建干涉图：`build(f, live, spill_set, fresh) -> InterferenceGraph`。
/// spill_set 中的 vreg 不再是节点（其 fresh 替代品入图）；fresh 按 at==i 并入该指令活集。
pub(super) fn build(
    f: &IRFunction, live: &LiveInfo, spill_set: &BTreeSet<u32>, fresh: &[FreshVreg],
) -> InterferenceGraph {
    let real = collect_real_vregs(f);
    // inst_live_before 长度 = 指令数（逐指令活集索引）
    let inst_count = live.inst_live_before.len();

    // ── 预着色 ──
    let mut pre_colors: BTreeMap<u32, u32> = BTreeMap::new();
    // 参数段：VM 调用写 regs[param_base+i]，钉死不可动（D-10）
    let pl = f.param_layout;
    if pl.count > 0 {
        for i in 0..pl.count {
            let v = pl.base + i;
            pre_colors.insert(v, v);
        }
    }
    // escaped：递归 nested 收集 LOAD_VAR.a / STORE_VAR.rd 直引的父槽（B012）
    let mut escaped_colors: Vec<u32> = Vec::new();
    collect_escaped(&f.nested, &mut pre_colors, &mut escaped_colors);
    // 对称缺口（B013 延伸）：子模块自身引用的父槽也必须预着色恒等。父侧 collect_escaped
    // 只保证父不移动槽；但子模块 alloc 时，它引用父槽的 LOAD_VAR.a / STORE_VAR.rd 会被
    // 当作子模块自己的 vreg 参与染色而移走 → 子模块读错物理槽。分界线 = param_layout.base
    // （emit 的 inherited_reg_start 继承机制：子模块 vreg ≥ base，父槽引用 < base）。
    collect_own_escaped(f, &mut pre_colors, &mut escaped_colors);

    // ── 可分配色集 ──
    let max_nargs = f
        .insts
        .iter()
        .filter(|i| {
            matches!(
                i.op,
                OpCode::CALL | OpCode::CALL_NATIVE | OpCode::NEW_EXPRESSION | OpCode::SUPER_CALL
            )
        })
        .map(|i| i.ext.first().copied().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let arg_window_base = 254u32.saturating_sub(max_nargs);
    let mut allocatable: Vec<u32> = Vec::new();
    for c in 1u32..=253 {
        if pre_colors.contains_key(&c) || escaped_colors.contains(&c) || c >= arg_window_base {
            continue;
        }
        allocatable.push(c);
    }
    let k = allocatable.len();

    // ── 节点 + 干涉边 ──
    let mut nodes: BTreeMap<u32, Node> = BTreeMap::new();
    for &v in real.iter() {
        if !spill_set.contains(&v) {
            nodes.insert(
                v,
                Node { pre_color: pre_colors.get(&v).copied(), adj: Vec::new() },
            );
        }
    }
    for fr in fresh {
        nodes.entry(fr.id).or_insert(Node { pre_color: None, adj: Vec::new() });
    }
    // 邻接表（BTreeMap<u32, BTreeSet<u32>> 去重后转 Vec 排序）
    let mut adj_sets: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let node_ids: Vec<u32> = nodes.keys().copied().collect();
    for &v in &node_ids {
        adj_sets.entry(v).or_default();
    }
    for i in 0..inst_count {
        let mut at_i: Vec<u32> = Vec::new();
        for &v in &node_ids {
            if live.inst_live_before[i].get(v as usize).copied().unwrap_or(false) {
                at_i.push(v);
            }
        }
        for fr in fresh {
            if fr.at == i {
                at_i.push(fr.id);
            }
        }
        // 两两加边
        for a in 0..at_i.len() {
            for b in a + 1..at_i.len() {
                let (x, y) = (at_i[a], at_i[b]);
                adj_sets.entry(x).or_default().insert(y);
                adj_sets.entry(y).or_default().insert(x);
            }
        }
    }
    for (v, set) in adj_sets {
        if let Some(node) = nodes.get_mut(&v) {
            node.adj = set.into_iter().collect();
        }
    }

    InterferenceGraph { nodes, allocatable, k, arg_window_base }
}

/// 递归收集 nested 树中变量槽引用（LOAD_VAR.a 读槽、STORE_VAR.rd 写槽），
/// 注入 pre_colors 并记录 escaped 色列表（可分配集排除 + spill 候选排除，B012）。
fn collect_escaped(nested: &[IRFunction], pre_colors: &mut BTreeMap<u32, u32>, out: &mut Vec<u32>) {
    for sub in nested {
        for inst in &sub.insts {
            let slot = match inst.op {
                OpCode::LOAD_VAR => inst.a,
                OpCode::STORE_VAR => inst.rd,
                _ => Operand::None,
            };
            if let Operand::Reg(r) = slot {
                pre_colors.entry(r).or_insert(r);
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
        collect_escaped(&sub.nested, pre_colors, out);
    }
    out.sort_unstable();
}

/// 收集当前函数自身对父槽的直接引用（B013 延伸，父侧保护的对称缺口）：
/// `LOAD_VAR.a` / `STORE_VAR.rd` 中槽号 < param_layout.base 的 vreg 是父槽引用
/// （emit inherited_reg_start 分界：子函数自身 vreg 从 base 起分配，父槽引用 < base）。
/// 父侧 collect_escaped 只保护父不移动槽；此处保证子函数 alloc 时这些引用不被染色移走。
fn collect_own_escaped(f: &IRFunction, pre_colors: &mut BTreeMap<u32, u32>, out: &mut Vec<u32>) {
    let base = f.param_layout.base;
    if base == 0 {
        return; // 顶层/无父槽场景无分界，跳过
    }
    for inst in &f.insts {
        let slot = match inst.op {
            OpCode::LOAD_VAR => inst.a,
            OpCode::STORE_VAR => inst.rd,
            _ => Operand::None,
        };
        if let Operand::Reg(r) = slot {
            if r < base {
                pre_colors.entry(r).or_insert(r);
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
    }
    out.sort_unstable();
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_ir::inst::Inst;
    use oxide_ir::operand::Operand;

    fn empty_function() -> IRFunction {
        IRFunction::new()
    }

    fn build_graph(f: &IRFunction) -> InterferenceGraph {
        let cfg = oxide_cfg::build_cfg(f);
        let live = oxide_liveness::liveness(f, &cfg);
        build(f, &live, &BTreeSet::new(), &[])
    }

    #[test]
    fn simultaneously_live_get_edge() {
        // 0: LOAD_CONST r1   1: LOAD_CONST r2   2: ADD r3=r1+r2（r1/r2 同时 live）  3: RETURN r3
        let mut f = empty_function();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts.push(Inst::load_const(Operand::Reg(2), 0));
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert!(g.nodes[&1].adj.contains(&2), "r1 与 r2 应邻接");
        assert!(g.nodes[&2].adj.contains(&1));
        assert!(!g.nodes[&3].adj.contains(&1), "r3 与 r1 无干涉（r1 死于 ADD）");
    }

    #[test]
    fn disjoint_ranges_no_edge() {
        // r1/r2 死于 ADD0，r4/r5 生于 ADD1——两段活度不相交
        let mut f = empty_function();
        f.insts.push(Inst::load_const(Operand::Reg(1), 0));
        f.insts.push(Inst::load_const(Operand::Reg(2), 0));
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts.push(Inst::load_const(Operand::Reg(4), 0));
        f.insts.push(Inst::load_const(Operand::Reg(5), 0));
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(6), Operand::Reg(4), Operand::Reg(5)));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert!(!g.nodes[&1].adj.contains(&4), "r1 与 r4 活度不相交，无边");
    }

    #[test]
    fn param_segment_precolored() {
        let mut f = empty_function();
        f.param_layout = oxide_ir::ParamLayout { base: 1, count: 2 };
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert_eq!(g.nodes[&1].pre_color, Some(1));
        assert_eq!(g.nodes[&2].pre_color, Some(2));
        assert!(!g.allocatable.contains(&1) && !g.allocatable.contains(&2), "参数色排除");
        assert_eq!(g.k, g.allocatable.len());
    }

    #[test]
    fn escaped_slots_precolored() {
        let mut f = empty_function();
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(5), Operand::Reg(3), Operand::Reg(4)));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        let mut sub = empty_function();
        sub.insts.push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(9), Operand::Reg(3), Operand::None));
        f.nested.push(sub);
        let g = build_graph(&f);
        assert_eq!(g.nodes[&3].pre_color, Some(3), "escaped 槽 r3 预着色");
        assert!(!g.allocatable.contains(&3), "escaped 色排除");
    }

    #[test]
    fn reserved_colors_excluded() {
        let mut f = empty_function();
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert!(!g.allocatable.contains(&0), "色 0 排除（CALL 隐式 reg0）");
        assert!(!g.allocatable.contains(&254) && !g.allocatable.contains(&255), "254/255 排除");
        assert!(g.allocatable.iter().all(|&c| (1..=253).contains(&c)));
    }

    #[test]
    fn arg_window_reserved() {
        let mut f = empty_function();
        f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 3));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert_eq!(g.arg_window_base, 251, "max_nargs=3 → base 251");
        assert!(!g.allocatable.contains(&251) && !g.allocatable.contains(&252) && !g.allocatable.contains(&253));
    }

    #[test]
    fn high_numbered_vreg_254_is_real_node() {
        // 大函数场景：真实 vreg 254 必须建节点（B005 正确性前提）
        let mut f = empty_function();
        f.insts.push(Inst::new(OpCode::ADD, Operand::Reg(254), Operand::Reg(252), Operand::Reg(253)));
        f.insts.push(Inst::new(OpCode::RETURN, Operand::Reg(254), Operand::None, Operand::None));
        let real = collect_real_vregs(&f);
        assert!(real.contains(&254), "真实 vreg 254 必须被收集");
        let g = build_graph(&f);
        assert!(g.nodes.contains_key(&254), "vreg 254 建节点");
        assert_eq!(g.nodes[&254].pre_color, None, "254 非预着色");
    }
}
