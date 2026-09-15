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
