//! 精确轮单元断言：寄存器复用死写、STORE_VAR 特例、label 目标、收敛行为。

use oxide_dce::dce_precise;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;

/// helper：稀疏号集 → u64 位集 live_after。
fn live_sets(after: &[&[u32]], reg_count: usize) -> Vec<Vec<u64>> {
    let words = (reg_count + 1).div_ceil(64);
    after
        .iter()
        .map(|set| {
            let mut v = vec![0u64; words];
            for &r in *set {
                v[(r as usize) >> 6] |= 1u64 << ((r as usize) & 63);
            }
            v
        })
        .collect()
}

fn run_precise(f: &mut IRFunction, after: &[&[u32]], reg_count: usize) {
    let mut live = LiveInfo::new();
    live.inst_live_after = live_sets(after, reg_count);
    dce_precise(f, &live);
}

/// 精确轮标志性测试：寄存器复用后被后写杀死的死写（保守 use 计数抓不到）。
#[test]
fn reuse_killed_dead_write_deleted() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::ADD,
        Operand::Reg(5),
        Operand::Reg(1),
        Operand::Reg(2),
    ));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::ADD,
        Operand::Reg(5),
        Operand::Reg(3),
        Operand::Reg(4),
    ));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(5),
        Operand::None,
        Operand::None,
    ));
    // inst_live_after = [{3,4}, {5}, {}]：inst0 def 5 在 live_after[0] 无 5（后写杀死）
    run_precise(&mut f, &[&[3, 4], &[5], &[]], 5);
    assert_eq!(f.insts.len(), 2, "死写（前一个 ADD）应被删");
}

/// 局部死 STORE_VAR 删除（四条件全满足）。
#[test]
fn local_dead_store_var_deleted() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(1), 0));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::STORE_VAR,
        Operand::Reg(2),
        Operand::Reg(1),
        Operand::Imm(0),
    ));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(5),
        Operand::None,
        Operand::None,
    ));
    // is_top_level 默认 false；inst_live_after = [{1,5}, {5}, {}]
    run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
    assert_eq!(f.insts.len(), 2, "局部死 STORE_VAR 应被删（LOAD_CONST 单遍不级联保留）");
}

/// 顶层 STORE_VAR 永不删（顶层赋值全局可观察）。
#[test]
fn top_level_store_var_kept() {
    let mut f = IRFunction::new();
    f.is_top_level = true;
    f.insts.push(Inst::load_const(Operand::Reg(1), 0));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::STORE_VAR,
        Operand::Reg(2),
        Operand::Reg(1),
        Operand::Imm(0),
    ));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(5),
        Operand::None,
        Operand::None,
    ));
    run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
    assert_eq!(f.insts.len(), 3, "顶层 STORE_VAR 不可删");
}

/// escaped 槽 STORE_VAR 永不删（nested 直读）。
#[test]
fn escaped_slot_store_var_kept() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(1), 0));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::STORE_VAR,
        Operand::Reg(2),
        Operand::Reg(1),
        Operand::Imm(0),
    ));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(5),
        Operand::None,
        Operand::None,
    ));
    let mut sub = IRFunction::new();
    sub.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::LOAD_VAR,
        Operand::Reg(5),
        Operand::Reg(2),
        Operand::None,
    ));
    f.nested.push(sub);
    run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
    assert_eq!(f.insts.len(), 3, "被嵌套函数直引的 escaped 槽 STORE_VAR 不可删");
}

/// const 赋值路径 b=1 保留（运行时抛 TypeError 可观察）。
#[test]
fn const_guard_store_var_kept() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(1), 0));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::STORE_VAR,
        Operand::Reg(2),
        Operand::Reg(1),
        Operand::Imm(1),
    ));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(5),
        Operand::None,
        Operand::None,
    ));
    run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
    assert_eq!(f.insts.len(), 3, "b=1 const 赋值路径不可删");
}

/// label 目标指令永不删。
#[test]
fn label_target_inst_never_deleted() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(1), 0)); // label 0 目标，纯且结果未用
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(5),
        Operand::None,
        Operand::None,
    ));
    f.label_pos = vec![Some(0)];
    run_precise(&mut f, &[&[5], &[]], 5);
    assert_eq!(f.insts.len(), 2, "label 目标指令不可删");
}

/// SPILL/UNSPILL 永不删（is_pure=false）；活 MOV 保留。
#[test]
fn spill_unspill_never_deleted_mov_kept_when_live() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::inst_spill(Operand::Reg(1), 0));
    f.insts.push(Inst::inst_unspill(Operand::Reg(2), 0));
    f.insts.push(Inst::inst_mov(Operand::Reg(3), Operand::Reg(2)));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(3),
        Operand::None,
        Operand::None,
    ));
    run_precise(&mut f, &[&[], &[2], &[3], &[]], 5);
    assert_eq!(f.insts.len(), 4, "SPILL/UNSPILL 永不删，活 MOV 保留");
}

/// 过期 LiveInfo 维度守卫：直接 return 零删除。
#[test]
fn stale_liveinfo_no_op() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(1), 0));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::STORE_VAR,
        Operand::Reg(2),
        Operand::Reg(1),
        Operand::Imm(0),
    ));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(5),
        Operand::None,
        Operand::None,
    ));
    let live = LiveInfo::new(); // 空 LiveInfo：维度不符
    dce_precise(&mut f, &live);
    assert_eq!(f.insts.len(), 3, "过期 LiveInfo 不得删除任何指令");
}

/// 收敛性：单遍不级联设计下，重复跑会逐轮收敛（删 STORE_VAR 后其源 LOAD_CONST 变死），
/// 收敛到不动点后不再改变。
#[test]
fn dce_precise_converges() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(1), 0));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::STORE_VAR,
        Operand::Reg(2),
        Operand::Reg(1),
        Operand::Imm(0),
    ));
    f.insts.push(Inst::new(
        oxide_bytecode::opcode::OpCode::RETURN,
        Operand::Reg(5),
        Operand::None,
        Operand::None,
    ));
    // 第一遍：手填 live（STORE_VAR 死）
    run_precise(&mut f, &[&[1, 5], &[5], &[]], 5);
    assert_eq!(f.insts.len(), 2, "第一遍删 STORE_VAR");
    // 第二遍：重算 live → LOAD_CONST 变死
    let cfg = oxide_cfg::build_cfg(&f);
    let live = oxide_liveness::liveness(&f, &cfg);
    dce_precise(&mut f, &live);
    assert_eq!(f.insts.len(), 1, "第二遍删 LOAD_CONST（级联收敛）");
    let after2 = f.insts.clone();
    // 第三遍：不动点，不再改变
    let cfg = oxide_cfg::build_cfg(&f);
    let live = oxide_liveness::liveness(&f, &cfg);
    dce_precise(&mut f, &live);
    assert_eq!(f.insts, after2, "第三遍不再删除（收敛）");
}

/// 空函数退化。
#[test]
fn empty_function_unchanged() {
    let mut f = IRFunction::new();
    let live = LiveInfo::new();
    dce_precise(&mut f, &live);
    assert!(f.insts.is_empty());
}
