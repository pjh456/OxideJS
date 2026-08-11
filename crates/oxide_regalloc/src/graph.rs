//! 干涉图构建 + 预着色 + 可分配色集。
//!
//! - 真实 vreg：字面 `Operand::Reg(r)`（rd/a/b）+ CALL 系参数连续区间 [first, first+nargs)
//! - 干涉边：逐指令 `live_before[i]` 中同时 live 的真实 vreg 两两加边（fresh vreg 按 at==i 并入）
//! - 预着色：参数段 [param_base, base+count) 钉死原号（VM 调用契约）；escaped vreg
//!   （nested `LOAD_VAR.a`/`STORE_VAR.rd` 直引父槽）钉死原槽号且排除 spill
//! - 可分配色集：`(1..=253) − 参数色 − escaped 色 − [arg_window_base, 253]`
//!   （arg_window_base = 254 − max_nargs，为调用点参数连续性 MOV 补位留空槽）
//!
//! 确定性：全 BTreeMap + Vec 排序，禁 HashMap 迭代序。

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
/// + SUPER_CALL a 槽参数区间。0 无条件排除（reg 0 永非真实——emit 分配寄存器从 ≥1 起）。
///
/// 254/255 的过近似规则：live bitset 中位 254/255 无法区分 This/NewTarget 占用与
/// 真实 vreg 活度，若函数存在该号真实 vreg（大函数场景）则按真实 vreg 建节点染色——
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
        // spread 调用系：ext 首字之后每个有序实参字是寄存器号（spread 源带高位标记）
        if matches!(inst.op, OpCode::CALL_SPREAD | OpCode::NEW_EXPRESSION_SPREAD | OpCode::SUPER_CALL_SPREAD) {
            for &w in inst.ext.iter().skip(1) {
                let r = w & 0x7FFF_FFFF;
                if r != 0 {
                    real.insert(r);
                }
            }
        }
        // 计算键访问器：ext[0] 是 key 寄存器（高位标记 `0x8000_0000 | vreg`）。
        if inst.op == OpCode::DEFINE_ACCESSOR_DYNAMIC {
            let r = inst.ext.first().copied().unwrap_or(0) & 0x7FFF_FFFF;
            if r != 0 {
                real.insert(r);
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
    let mut real = collect_real_vregs(f);
    // inst_live_before 长度 = 指令数（逐指令活集索引）
    let inst_count = live.inst_live_before.len();

    // 窗口只为参数连续性 MOV 桥预留；spread 调用实参经 ext 逐个读寄存器，无连续性要求。
    let max_nargs = f
        .insts
        .iter()
        .filter(|i| matches!(i.op, OpCode::CALL | OpCode::CALL_NATIVE | OpCode::NEW_EXPRESSION | OpCode::SUPER_CALL))
        .map(|i| i.ext.first().copied().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let arg_window_base = 254u32.saturating_sub(max_nargs);

    // ── 预着色 ──
    let mut pre_colors: BTreeMap<u32, u32> = BTreeMap::new();
    // 当前函数读取的父槽已经是父函数分配后的物理号，必须恒等保护。
    let mut escaped_colors: Vec<u32> = Vec::new();
    collect_own_escaped(f, &mut pre_colors, &mut escaped_colors);

    // 被子函数捕获的当前函数槽需要固定颜色，但高虚拟槽不能恒等映射到 254+。
    // 父函数完成分配后，alloc 会把选定物理色同步到整个 nested 树的父槽引用。
    let mut nested_escaped = Vec::new();
    collect_escaped(&f.nested, &mut nested_escaped);
    let param_end = f.param_layout.base.saturating_add(f.param_layout.count);
    for vreg in nested_escaped {
        if !real.contains(&vreg) || (f.param_layout.base..param_end).contains(&vreg) {
            continue;
        }
        let identity_available =
            vreg >= 1 && vreg < arg_window_base && !pre_colors.values().any(|color| *color == vreg);
        let physical = if identity_available {
            vreg
        } else {
            (1..arg_window_base)
                .find(|color| !pre_colors.values().any(|existing| existing == color))
                .unwrap_or_else(|| {
                    debug_assert!(false, "escaped 槽无自由色（vreg={vreg}）");
                    1
                })
        };
        pre_colors.insert(vreg, physical);
        escaped_colors.push(physical);
    }
    escaped_colors.sort_unstable();
    escaped_colors.dedup();

    // 参数段必须连续。继承父上下文后虚拟段可能高于 253，此时选择不与 escaped 槽
    // 冲突的低位连续物理段；finish 按首参数颜色同步回写 param_layout.base。
    let pl = f.param_layout;
    if pl.count > 0 {
        let identity_end = pl.base.saturating_add(pl.count);
        let identity_available = pl.base >= 1
            && identity_end <= arg_window_base
            && (pl.base..identity_end).all(|color| !escaped_colors.contains(&color));
        let physical_base = if identity_available {
            pl.base
        } else {
            (1..=arg_window_base.saturating_sub(pl.count))
                .find(|base| {
                    (*base..*base + pl.count).all(|color| {
                        !pre_colors.values().any(|existing| *existing == color) && !escaped_colors.contains(&color)
                    })
                })
                .unwrap_or_else(|| {
                    debug_assert!(false, "参数段无连续自由色（base={}, count={}）", pl.base, pl.count);
                    1
                })
        };
        for i in 0..pl.count {
            let vreg = pl.base + i;
            real.insert(vreg);
            pre_colors.insert(vreg, physical_base + i);
        }
    }

    // ── builtin 槽 ──
    // VM 帧推入时写 regs[slot]=全局值（无指令 def 却活到入口），物理号须恒定且排除出
    // 可分配集，否则 prologue 死定义临时（如解构 FOR_OF_DONE 结果）与它同色，在
    // builtin use 前执行写入 → 覆写全局值。
    // 恒等号 v 在 ≤253 且不落入参数 MOV 窗口时保持恒等；大函数晚引用 builtin 的槽号
    // >253（或 ≥arg_window_base 与 MOV 桥冲突）时恒等号不可编码 → 改分配确定性自由色
    // R（算法与 spilled_builtin_bindings 同源：1..arg_window_base 中最小未占用）。
    let mut builtin_entries: Vec<(String, u32)> = f
        .builtin_reg_map
        .iter()
        .filter(|(_, reg)| real.contains(reg))
        .map(|(name, reg)| (name.clone(), *reg))
        .collect();
    builtin_entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut used_colors: Vec<u32> = Vec::new();
    for (_, v) in builtin_entries {
        let r = if v <= 253 && v < arg_window_base {
            v
        } else {
            (1u32..arg_window_base)
                .find(|c| {
                    !pre_colors.values().any(|p| *p == *c) && !escaped_colors.contains(c) && !used_colors.contains(c)
                })
                .unwrap_or_else(|| {
                    debug_assert!(false, "builtin 槽无自由色（v={v}）");
                    1
                })
        };
        used_colors.push(r);
        pre_colors.insert(v, r);
        if !escaped_colors.contains(&r) {
            escaped_colors.push(r);
        }
    }

    let mut allocatable: Vec<u32> = Vec::new();
    for c in 1u32..=253 {
        if pre_colors.values().any(|color| *color == c) || escaped_colors.contains(&c) || c >= arg_window_base {
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
                Node {
                    pre_color: pre_colors.get(&v).copied(),
                    adj: Vec::new(),
                },
            );
        }
    }
    for fr in fresh {
        nodes.entry(fr.id).or_insert(Node {
            pre_color: None,
            adj: Vec::new(),
        });
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
        // 两两加边（基于 live_before：约束 use/def 与执行前存活值）
        for a in 0..at_i.len() {
            for b in a + 1..at_i.len() {
                let (x, y) = (at_i[a], at_i[b]);
                adj_sets.entry(x).or_default().insert(y);
                adj_sets.entry(y).or_default().insert(x);
            }
        }
        // def 结果与执行后仍存活的值冲突（写覆盖）：live_after 含 def 后存活者。
        // live_before 已 kill def 自身，纯写指令（FOR_OF_NEXT 等）的结果若与长活
        // 寄存器同色会覆盖其值（elision 解构返回值被覆盖），须补此干涉边。
        if let Some(d) = f.insts[i].def_reg() {
            if nodes.contains_key(&d) {
                for &v in &node_ids {
                    if v == d {
                        continue;
                    }
                    if live.inst_live_after[i].get(v as usize).copied().unwrap_or(false) {
                        adj_sets.entry(d).or_default().insert(v);
                        adj_sets.entry(v).or_default().insert(d);
                    }
                }
            }
        }
    }
    for (v, set) in adj_sets {
        if let Some(node) = nodes.get_mut(&v) {
            node.adj = set.into_iter().collect();
        }
    }

    InterferenceGraph {
        nodes,
        allocatable,
        k,
        arg_window_base,
    }
}

/// 递归收集 nested 树中的父槽虚拟号（LOAD_VAR.a 读槽、STORE_VAR.rd 写槽）。
fn collect_escaped(nested: &[IRFunction], out: &mut Vec<u32>) {
    for sub in nested {
        for inst in &sub.insts {
            let slot = match inst.op {
                OpCode::LOAD_VAR => inst.a,
                OpCode::STORE_VAR => inst.rd,
                _ => Operand::None,
            };
            if let Operand::Reg(r) = slot {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
        collect_escaped(&sub.nested, out);
    }
    out.sort_unstable();
}

/// 收集当前函数自身对父槽的直接引用（父侧保护的对称缺口）：
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
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
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
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts.push(Inst::load_const(Operand::Reg(4), 0));
        f.insts.push(Inst::load_const(Operand::Reg(5), 0));
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(6), Operand::Reg(4), Operand::Reg(5)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert!(!g.nodes[&1].adj.contains(&4), "r1 与 r4 活度不相交，无边");
    }

    #[test]
    fn param_segment_precolored() {
        let mut f = empty_function();
        f.param_layout = oxide_ir::ParamLayout { base: 1, count: 2 };
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert_eq!(g.nodes[&1].pre_color, Some(1));
        assert_eq!(g.nodes[&2].pre_color, Some(2));
        assert!(!g.allocatable.contains(&1) && !g.allocatable.contains(&2), "参数色排除");
        assert_eq!(g.k, g.allocatable.len());
    }

    #[test]
    fn escaped_slots_precolored() {
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(5), Operand::Reg(3), Operand::Reg(4)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        let mut sub = empty_function();
        sub.insts
            .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(9), Operand::Reg(3), Operand::None));
        f.nested.push(sub);
        let g = build_graph(&f);
        assert_eq!(g.nodes[&3].pre_color, Some(3), "escaped 槽 r3 预着色");
        assert!(!g.allocatable.contains(&3), "escaped 色排除");
    }

    #[test]
    fn high_escaped_slot_uses_encodable_color() {
        let mut f = empty_function();
        f.insts.push(Inst::load_const(Operand::Reg(254), 0));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(254), Operand::None, Operand::None));
        let mut sub = empty_function();
        sub.param_layout = oxide_ir::ParamLayout { base: 300, count: 0 };
        sub.insts
            .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(301), Operand::Reg(254), Operand::None));
        f.nested.push(sub);
        let g = build_graph(&f);
        let color = g.nodes[&254].pre_color.expect("escaped 槽应预着色");
        assert!((1..=253).contains(&color));
        assert_ne!(color, 254);
    }

    #[test]
    fn reserved_colors_excluded() {
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert!(!g.allocatable.contains(&0), "色 0 排除（CALL 隐式 reg0）");
        assert!(!g.allocatable.contains(&254) && !g.allocatable.contains(&255), "254/255 排除");
        assert!(g.allocatable.iter().all(|&c| (1..=253).contains(&c)));
    }

    #[test]
    fn arg_window_reserved() {
        let mut f = empty_function();
        f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 3));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
        let g = build_graph(&f);
        assert_eq!(g.arg_window_base, 251, "max_nargs=3 → base 251");
        assert!(!g.allocatable.contains(&251) && !g.allocatable.contains(&252) && !g.allocatable.contains(&253));
    }

    #[test]
    fn high_numbered_vreg_254_is_real_node() {
        // 大函数场景：真实 vreg 254 必须建节点（正确性前提）
        let mut f = empty_function();
        f.insts
            .push(Inst::new(OpCode::ADD, Operand::Reg(254), Operand::Reg(252), Operand::Reg(253)));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::Reg(254), Operand::None, Operand::None));
        let real = collect_real_vregs(&f);
        assert!(real.contains(&254), "真实 vreg 254 必须被收集");
        let g = build_graph(&f);
        assert!(g.nodes.contains_key(&254), "vreg 254 建节点");
        assert_eq!(g.nodes[&254].pre_color, None, "254 非预着色");
    }
}
