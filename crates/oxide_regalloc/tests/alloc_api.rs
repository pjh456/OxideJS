//! alloc 改写 pass 集成测试：经 crate 根 pub 入口 alloc() 验证空函数退化/过期 LiveInfo
//! 重跑/参数段重映射/嵌套槽传播/Err 传播/嵌套递归改写，及 AllocMap 构造零值。

use oxide_bytecode::opcode::OpCode;
use oxide_cfg::build_cfg;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;
use oxide_liveness::liveness;
use oxide_liveness::LiveInfo;
use oxide_regalloc::{alloc, AllocMap};

#[test]
fn alloc_empty_ok() {
    let mut f = IRFunction::new();
    assert!(alloc(&mut f, &LiveInfo::new()).is_ok());
}

#[test]
fn alloc_stale_live_reruns() {
    let mut f = IRFunction::new();
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(3), Operand::Reg(1), Operand::Reg(2)));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
    // 空 LiveInfo（维度不符）→ 内部重跑
    let stale = LiveInfo::new();
    assert!(alloc(&mut f, &stale).is_ok());
    // 重写后全部槽 ≤253
    for inst in &f.insts {
        for o in [&inst.rd, &inst.a, &inst.b] {
            if let Operand::Reg(r) = o {
                assert!(*r <= 253, "重写后槽 {r} 超 253");
            }
        }
    }
}

#[test]
fn alloc_remaps_high_parameter_segment() {
    let mut f = IRFunction::new();
    f.param_layout = oxide_ir::ParamLayout { base: 300, count: 1 };
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(300), Operand::None, Operand::None));
    let cfg = build_cfg(&f);
    let live = liveness(&f, &cfg);
    alloc(&mut f, &live).unwrap();
    assert_eq!(f.param_layout, oxide_ir::ParamLayout { base: 1, count: 1 });
    assert!(matches!(f.insts[0].rd, Operand::Reg(1)));
    assert_eq!(f.n_registers, 2);
}

#[test]
fn alloc_propagates_moved_parameter_to_nested_slot_reads() {
    let mut f = IRFunction::new();
    f.param_layout = oxide_ir::ParamLayout { base: 300, count: 1 };
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(300), Operand::None, Operand::None));
    let mut child = IRFunction::new();
    child.param_layout = oxide_ir::ParamLayout { base: 301, count: 0 };
    child
        .insts
        .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(302), Operand::Reg(300), Operand::None));
    child
        .insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(302), Operand::None, Operand::None));
    f.nested.push(child);
    let cfg = build_cfg(&f);
    let live = liveness(&f, &cfg);
    alloc(&mut f, &live).unwrap();
    assert!(matches!(f.nested[0].insts[0].a, Operand::Reg(1)));
}

#[test]
fn alloc_propagates_high_escaped_slot_to_nested_reads() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(254), 0));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(254), Operand::None, Operand::None));
    let mut child = IRFunction::new();
    child.param_layout = oxide_ir::ParamLayout { base: 300, count: 0 };
    child
        .insts
        .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(301), Operand::Reg(254), Operand::None));
    child
        .insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(301), Operand::None, Operand::None));
    f.nested.push(child);
    let cfg = build_cfg(&f);
    let live = liveness(&f, &cfg);
    alloc(&mut f, &live).unwrap();
    let physical = match f.insts[0].rd {
        Operand::Reg(reg) => reg,
        _ => panic!("LOAD_CONST 目标应为寄存器"),
    };
    assert!((1..=253).contains(&physical));
    assert!(matches!(f.nested[0].insts[0].a, Operand::Reg(reg) if reg == physical));
}

#[test]
fn alloc_err_propagates() {
    // 255 参数 CALL：参数窗口吞并全部可分配色 → k=0 → 无可行染色 → Err
    let mut f = IRFunction::new();
    f.insts
        .push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3000), 255));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
    let cfg = build_cfg(&f);
    let live = liveness(&f, &cfg);
    let err = alloc(&mut f, &live).unwrap_err();
    assert!(err.contains("too many registers"), "Err: {err}");
}

#[test]
fn alloc_recurses_nested() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(1), 0));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(1), Operand::None, Operand::None));
    let mut sub = IRFunction::new();
    sub.insts.push(Inst::load_const(Operand::Reg(3), 0));
    sub.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(3), Operand::None, Operand::None));
    f.nested.push(sub);
    let cfg = build_cfg(&f);
    let live = liveness(&f, &cfg);
    alloc(&mut f, &live).unwrap();
    // 父与子函数全部槽 ≤253
    for inst in &f.insts {
        for o in [&inst.rd, &inst.a, &inst.b] {
            if let Operand::Reg(r) = o {
                assert!(*r <= 253);
            }
        }
    }
    assert!(!f.nested[0].insts.is_empty(), "子函数也被重写");
    assert!(f.nested[0].n_registers <= 253, "子函数 n_registers 已回写");
}

#[test]
fn alloc_map_new_is_empty() {
    let m = AllocMap::new();
    assert!(m.map.is_empty());
    assert!(m.spills.is_empty());
    assert_eq!(m.phys_peak, 0);
    assert_eq!(m.arg_window_base, 254);
    assert!(AllocMap::default().map.is_empty());
}
