//! build_cfg 结构形状单测：IRFunction → Cfg。
//!
//! 覆盖块划分、条件双出边、try 异常边、exit 哨兵、退化形态与幂等；只走 pub API。

use oxide_bytecode::opcode::OpCode;
use oxide_cfg::{build_cfg, Cfg, EdgeKind};
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

fn succs_total(cfg: &Cfg) -> usize {
    cfg.blocks.iter().map(|b| b.succs.len()).sum()
}

fn preds_total(cfg: &Cfg) -> usize {
    cfg.blocks.iter().map(|b| b.preds.len()).sum()
}

/// 无跳转线性函数：1 实块 + exit 哨兵，块 0 Fallthrough→exit，preds/succs 对称。
#[test]
fn linear_function_is_single_block_plus_exit() {
    let mut f = IRFunction::new();
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));

    let cfg = build_cfg(&f);
    assert_eq!(cfg.blocks.len(), 2); // 1 实块 + exit 哨兵
    assert_eq!(cfg.entry, 0);
    assert_eq!(cfg.exit, 1);
    assert_eq!(cfg.blocks[0].inst_range, 0..3);
    assert_eq!(cfg.blocks[0].succs, vec![(1, EdgeKind::Fallthrough)]);
    assert!(cfg.blocks[1].succs.is_empty()); // exit 哨兵无出边
    assert_eq!(cfg.blocks[1].preds, vec![0]);
    assert_eq!(succs_total(&cfg), preds_total(&cfg));
}

/// if/else 典型形状：条件分裂块（Jump + Fallthrough 双出边），4 实块 + exit 哨兵。
#[test]
fn if_else_conditional_jump_splits_block_with_dual_edges() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::jmp_if_false(1, 0)); // 0: cond → L0(else)
    f.insts.push(Inst::call(Operand::Reg(2), Operand::Reg(0), Operand::Reg(3), 1)); // 1: then a()
    f.insts.push(Inst::jmp(1)); // 2: → L1(end)
    f.insts.push(Inst::call(Operand::Reg(4), Operand::Reg(0), Operand::Reg(3), 1)); // 3: else b()
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 4
    f.label_pos = vec![Some(3), Some(4)]; // L0→else 块头(3), L1→end(4)
    f.label_count = 2;

    let cfg = build_cfg(&f);
    assert_eq!(cfg.blocks.len(), 5); // 4 实块 + exit 哨兵
    assert_eq!(cfg.entry, 0);
    assert_eq!(cfg.exit, 4);

    // 块 0（cond）：条件分裂，Jump→else + Fallthrough→then 两条出边都在
    assert_eq!(cfg.blocks[0].inst_range, 0..1);
    assert_eq!(cfg.blocks[0].succs, vec![(2, EdgeKind::Jump), (1, EdgeKind::Fallthrough)]);
    assert!(cfg.blocks[0].preds.is_empty());
    // 块 1（then）：Jump→end
    assert_eq!(cfg.blocks[1].inst_range, 1..3);
    assert_eq!(cfg.blocks[1].succs, vec![(3, EdgeKind::Jump)]);
    assert_eq!(cfg.blocks[1].preds, vec![0]);
    // 块 2（else）：Fallthrough→end
    assert_eq!(cfg.blocks[2].inst_range, 3..4);
    assert_eq!(cfg.blocks[2].succs, vec![(3, EdgeKind::Fallthrough)]);
    assert_eq!(cfg.blocks[2].preds, vec![0]);
    // 块 3（end）：RETURN → Fallthrough→exit
    assert_eq!(cfg.blocks[3].inst_range, 4..5);
    assert_eq!(cfg.blocks[3].succs, vec![(4, EdgeKind::Fallthrough)]);
    assert_eq!(cfg.blocks[3].preds, vec![1, 2]);
    // 块 4（exit 哨兵）：空块 0..0，preds 含块 3，无出边
    assert_eq!(cfg.blocks[4].inst_range, 0..0);
    assert!(cfg.blocks[4].succs.is_empty());
    assert_eq!(cfg.blocks[4].preds, vec![3]);
    assert_eq!(succs_total(&cfg), preds_total(&cfg));
}

/// try/catch：TRY_BEGIN 是块内标记不切块，所在 BB 出 Exception 边到 catch 入口。
#[test]
fn try_begin_does_not_split_block_and_emits_exception_edge() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN（块内标记）
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 3: catch 入口
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 4
    f.label_pos = vec![Some(3)];
    f.label_count = 1;

    let cfg = build_cfg(&f);
    assert_eq!(cfg.blocks.len(), 3); // 2 实块 + exit 哨兵（TRY_BEGIN 不切块）
    assert_eq!(cfg.blocks[0].inst_range, 0..3); // try 体未被切块
                                                // Exception 边从 TRY_BEGIN 所在 BB 出发指向 catch 入口块。
    assert!(cfg.blocks[0].succs.contains(&(1, EdgeKind::Exception)));
    assert_eq!(cfg.blocks[1].succs, vec![(2, EdgeKind::Fallthrough)]);
    assert_eq!(cfg.blocks[2].preds, vec![0, 1]);
    assert_eq!(succs_total(&cfg), preds_total(&cfg));
}

/// try/finally：TRY_FINALLY_BEGIN 同上，Exception 边到 finally 入口块。
#[test]
fn try_finally_begin_emits_exception_edge() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::try_finally_begin(0)); // 0
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 3: finally 入口
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 4
    f.label_pos = vec![Some(3)];
    f.label_count = 1;

    let cfg = build_cfg(&f);
    assert_eq!(cfg.blocks.len(), 3);
    assert!(cfg.blocks[0].succs.contains(&(1, EdgeKind::Exception)));
    assert_eq!(cfg.blocks[2].preds, vec![0, 1]);
    assert_eq!(succs_total(&cfg), preds_total(&cfg));
}

/// 尾部 RETURN：该块 Fallthrough 到 exit。
#[test]
fn trailing_return_flows_to_exit() {
    let mut f = IRFunction::new();
    f.insts
        .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None));
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));

    let cfg = build_cfg(&f);
    assert_eq!(cfg.blocks.len(), 2);
    assert_eq!(cfg.blocks[0].succs, vec![(1, EdgeKind::Fallthrough)]);
    assert_eq!(cfg.blocks[1].preds, vec![0]);
}

/// 空 IRFunction：退化 CFG，entry == exit == 0，blocks 长度 1。
#[test]
fn empty_function_yields_degenerate_cfg() {
    let f = IRFunction::new();
    let cfg = build_cfg(&f);
    assert_eq!(cfg.blocks.len(), 1);
    assert_eq!(cfg.entry, 0);
    assert_eq!(cfg.exit, 0);
    assert_eq!(cfg.blocks[0].inst_range, 0..0);
    assert!(cfg.blocks[0].succs.is_empty());
    assert!(cfg.blocks[0].preds.is_empty());
}

/// 幂等性：同一 IR 两次构建结果相等。
#[test]
fn build_cfg_is_idempotent() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::jmp_if_false(1, 0)); // 0: → L0（回环目标）
    f.insts.push(Inst::jmp(0)); // 1: → L0
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2
    f.label_pos = vec![Some(2), Some(0)]; // L0→2（尾部）, L1→0（回环）
    f.label_count = 2;

    let cfg1 = build_cfg(&f);
    let cfg2 = build_cfg(&f);
    assert_eq!(cfg1, cfg2);
}
