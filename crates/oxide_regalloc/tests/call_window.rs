//! 调用点存活上界编码：pub 入口 encode_call_window 的 ext 高 8 位编码与跳过行为集成测试。

use oxide_bytecode::opcode::OpCode;
use oxide_cfg::build_cfg;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;
use oxide_liveness::LiveInfo;
use oxide_regalloc::encode_call_window;

#[test]
fn call_window_packs_upper_bound_from_live_after() {
    // 0: CALL（r1=callee, r2=this, r3=首参, 1 参）  def {0} use {1,2,3}
    // 1: ADD r5 = r4 + r4    r4 跨调用存活
    // 2: RETURN r5
    let mut f = IRFunction::new();
    f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 1));
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(5), Operand::Reg(4), Operand::Reg(4)));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
    let cfg = build_cfg(&f);
    let live = oxide_liveness::liveness(&f, &cfg);
    encode_call_window(&mut f, &live);
    // 存活集 {4} → 上界 5，ext = 1 | (5 << 8)
    assert_eq!(f.insts[0].ext[0], 1 | (5 << 8));
}

#[test]
fn call_window_zero_when_nothing_survives() {
    // 0: CALL（结果未用）  1: LOAD_CONST r6  2: RETURN r6
    let mut f = IRFunction::new();
    f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 1));
    f.insts.push(Inst::load_const(Operand::Reg(6), 0));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(6), Operand::None, Operand::None));
    let cfg = build_cfg(&f);
    let live = oxide_liveness::liveness(&f, &cfg);
    encode_call_window(&mut f, &live);
    // 无跨调用存活 → 保持全量（ext 高 8 位 = 0）
    assert_eq!(f.insts[0].ext[0], 1);
}

#[test]
fn call_window_excludes_this_and_new_target() {
    // 0: LOAD_CONST r6  1: CALL  2: LOAD_VAR r8, This（254 跨调用存活）
    // 3: ADD r7=r6+r8  4: RETURN r7
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(6), 0));
    f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 1));
    f.insts
        .push(Inst::new(OpCode::LOAD_VAR, Operand::Reg(8), Operand::This, Operand::None));
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(7), Operand::Reg(6), Operand::Reg(8)));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(7), Operand::None, Operand::None));
    let cfg = build_cfg(&f);
    let live = oxide_liveness::liveness(&f, &cfg);
    encode_call_window(&mut f, &live);
    // 存活 {6, 254} → 上界 7（254 由帧单独保存，不放大窗口）
    assert_eq!(f.insts[1].ext[0], 1 | (7 << 8));
}

#[test]
fn call_window_skips_generator_body() {
    let mut f = IRFunction::new();
    f.is_generator = true;
    f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 1));
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(5), Operand::Reg(4), Operand::Reg(4)));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
    let cfg = build_cfg(&f);
    let live = oxide_liveness::liveness(&f, &cfg);
    encode_call_window(&mut f, &live);
    // 生成器体不编码 → ext 保持纯 nargs
    assert_eq!(f.insts[0].ext[0], 1);
}

#[test]
fn call_window_stale_live_reruns() {
    // 空 LiveInfo（维度不符）→ 内部重跑，仍正确编码。
    let mut f = IRFunction::new();
    f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 1));
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(5), Operand::Reg(4), Operand::Reg(4)));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
    encode_call_window(&mut f, &LiveInfo::new());
    assert_eq!(f.insts[0].ext[0], 1 | (5 << 8));
}

#[test]
fn call_window_skips_function_with_try() {
    // 含 TRY_BEGIN 的函数整体跳过编码（全量窗口）：异常边只从 TRY 所在 BB 出发，
    // 分支/循环内调用点的 liveness 不含仅 handler 存活的寄存器，截断窗口会丢槽。
    let mut f = IRFunction::new();
    f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN → L0
    f.insts.push(Inst::call(Operand::Reg(1), Operand::Reg(2), Operand::Reg(3), 1));
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(5), Operand::Reg(4), Operand::Reg(4)));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None));
    f.label_pos = vec![Some(2)];
    f.label_count = 1;
    let cfg = build_cfg(&f);
    let live = oxide_liveness::liveness(&f, &cfg);
    encode_call_window(&mut f, &live);
    // 存活集 {4} 本可编码上界 5，但含 try 保持全量（ext 高 8 位 = 0）
    assert_eq!(f.insts[1].ext[0], 1);
}
