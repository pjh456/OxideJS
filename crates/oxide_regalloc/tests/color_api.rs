//! 染色决策集成测试：经 crate 根 pub 入口 color() 验证选色/spill/确定性与 AllocMap 输出。

use oxide_bytecode::opcode::OpCode;
use oxide_cfg::build_cfg;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::{IRFunction, ParamLayout};
use oxide_liveness::liveness;
use oxide_regalloc::{color, Alloc, AllocMap};

fn empty_function() -> IRFunction {
    IRFunction::new()
}

fn color_of(insts: Vec<Inst>, param_layout: ParamLayout, nested: Vec<IRFunction>) -> Result<AllocMap, String> {
    let mut f = empty_function();
    f.insts = insts;
    f.param_layout = param_layout;
    f.nested = nested;
    let cfg = build_cfg(&f);
    let live = liveness(&f, &cfg);
    color(&f, &live)
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
        ParamLayout { base: 0, count: 0 },
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
        ParamLayout { base: 0, count: 0 },
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
        ParamLayout { base: 1, count: 2 },
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
    let cfg = build_cfg(&f);
    let live = liveness(&f, &cfg);
    let m = color(&f, &live).unwrap();
    assert_eq!(m.map[&3], Alloc::Phys(3), "escaped 槽保持原色");
    assert!(m.spills.iter().all(|s| s.vreg != 3), "escaped vreg 不得被 spill");
}

#[test]
fn highest_legal_color_uses_254_register_window() {
    let mut f = empty_function();
    f.builtin_reg_map = vec![("late".to_string(), 253)];
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(253), Operand::None, Operand::None));
    let cfg = build_cfg(&f);
    let live = liveness(&f, &cfg);
    let map = color(&f, &live).unwrap();
    assert_eq!(map.map[&253], Alloc::Phys(253));
    assert_eq!(map.phys_peak, 254);
}

#[test]
fn coloring_is_deterministic() {
    let src_insts = vec![
        Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(0), Operand::Reg(1)),
        Inst::new(OpCode::RETURN, Operand::Reg(2), Operand::None, Operand::None),
    ];
    let a = color_of(src_insts.clone(), ParamLayout { base: 0, count: 0 }, Vec::new()).unwrap();
    let b = color_of(src_insts, ParamLayout { base: 0, count: 0 }, Vec::new()).unwrap();
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
    let m = color_of(insts, ParamLayout { base: 0, count: 0 }, Vec::new()).unwrap();
    assert!(!m.spills.is_empty(), "长活 L 应被 spill");
    assert!(m.spills.iter().any(|s| s.vreg == 1000), "spill 的是 L");
    assert!(m.phys_peak <= 254, "phys_peak ≤ 254");
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
    let result = color_of(insts, ParamLayout { base: 0, count: 0 }, Vec::new());
    assert!(result.is_err(), "单点活度 ≥ k 应报 Err");
    assert!(result.unwrap_err().contains("too many registers"), "Err 消息应含 too many registers");
}

#[test]
fn pop_order_is_lowest_vreg_among_light_nodes() {
    // 混合图：vreg 5 的 degree 为 2（邻 1/7）、vreg 7 的 degree 为 1（邻 5）、vreg 1 的 degree 为 1
    // （邻 5），最小 vreg 的 degree 不是全图最小。弹出顺序是"light 节点中最小 vreg"（1, 5, 7 依次），
    // 不是"最小 degree 优先"（后者会先弹 8/9）。两种顺序染色结果不同：现实现 5→2、7→1；
    // 若按最小 degree 优先弹，则 5→1、7→2。断言值由现实现跑出后固化，钉死弹出顺序语义。
    let m = color_of(
        vec![
            Inst::load_const(Operand::Reg(1), 0),
            Inst::load_const(Operand::Reg(5), 0),
            Inst::new(OpCode::ADD, Operand::Reg(8), Operand::Reg(1), Operand::Reg(5)),
            Inst::load_const(Operand::Reg(7), 0),
            Inst::new(OpCode::ADD, Operand::Reg(9), Operand::Reg(5), Operand::Reg(7)),
            Inst::new(OpCode::RETURN, Operand::Reg(9), Operand::None, Operand::None),
        ],
        ParamLayout { base: 0, count: 0 },
        Vec::new(),
    )
    .unwrap();
    assert_eq!(m.map[&5], Alloc::Phys(2), "5 先弹、后染色，7 先占 1 号色");
    assert_eq!(m.map[&7], Alloc::Phys(1), "7 后弹、先染色，取最低可用色 1");
    assert_eq!(m.map[&1], Alloc::Phys(1), "1 最后染色，邻 5 已占 2 号色");
}

#[test]
fn stuck_spill_order_is_degree_then_vreg() {
    // 250 个未使用参数预着色 1..250（k=3）+ 六个非预着色节点全部 degree ≥ 3：
    // 300/301/302/303 同时 live（K4），304 与 300/301/302 同时 live，305 是 303 的
    // kill 结果（def 规则向 live_after 的 300/301/302 补边）。degree 为
    // 304:3、305:3、303:4、300:5、301:5、302:5。全部卡住，spill 候选序按
    // (degree, vreg) 升序：304、305、303、300、301、302（不是 vreg 号序 300..305）。
    // slot 号记录第一轮 failed 顺序，断言其钉死卡住路径的取最小 (degree, vreg) 语义。
    let insts = vec![
        Inst::load_const(Operand::Reg(300), 0),
        Inst::load_const(Operand::Reg(301), 0),
        Inst::load_const(Operand::Reg(302), 0),
        Inst::load_const(Operand::Reg(303), 0),
        Inst::new(OpCode::ADD, Operand::Reg(305), Operand::Reg(303), Operand::Reg(1)),
        Inst::load_const(Operand::Reg(304), 0),
        Inst::new(OpCode::ADD, Operand::Reg(306), Operand::Reg(300), Operand::Reg(304)),
        Inst::new(OpCode::ADD, Operand::Reg(307), Operand::Reg(301), Operand::Reg(302)),
        Inst::new(OpCode::RETURN, Operand::Reg(307), Operand::None, Operand::None),
    ];
    let m = color_of(insts, ParamLayout { base: 1, count: 250 }, Vec::new()).unwrap();
    assert_eq!(m.spills.len(), 6, "六个非预着色节点应全部 spill");
    let slot = |v: u32| m.spills.iter().find(|s| s.vreg == v).map(|s| s.slot);
    assert_eq!(slot(304), Some(0), "最小 (degree, vreg) 是 304，最先 spill");
    assert_eq!(slot(305), Some(1));
    assert_eq!(slot(303), Some(2));
    assert_eq!(slot(300), Some(3));
    assert_eq!(slot(301), Some(4));
    assert_eq!(slot(302), Some(5));
}
