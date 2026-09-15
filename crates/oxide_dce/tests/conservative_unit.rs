//! 保守轮单元断言：空函数退化、幂等性、双分支可达保留。

use oxide_bytecode::opcode::OpCode;
use oxide_dce::dce;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

/// 空函数退化：insts 空时 dce 直接返回，状态不变。
#[test]
fn empty_function_unchanged() {
    let mut f = IRFunction::new();
    dce(&mut f);
    assert!(f.insts.is_empty());
    assert!(f.label_pos.is_empty());
    assert_eq!(f.label_count, 0);
}

/// 幂等性：dce 两次结果一致（不动点后无二次改写）。
#[test]
fn dce_is_idempotent() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::load_const(Operand::Reg(1), 0)); // 0: 死
    f.insts
        .push(Inst::new(OpCode::ADD, Operand::Reg(2), Operand::Reg(1), Operand::Reg(1))); // 1: 死
    f.insts.push(Inst::load_const(Operand::Reg(0), 1)); // 2: 存活（CALL 保活 r0）
    f.insts.push(Inst::call(Operand::Reg(0), Operand::Reg(0), Operand::Reg(0), 0)); // 3
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 4
    f.label_pos = vec![Some(0)];

    dce(&mut f);
    let once_insts = f.insts.clone();
    let once_labels = f.label_pos.clone();
    dce(&mut f);
    assert_eq!(f.insts, once_insts, "二次 dce 后 insts 不变");
    assert_eq!(f.label_pos, once_labels, "二次 dce 后 label_pos 不变");
}

/// if/else 双分支完整链路：Jump + Fallthrough 双出边全可达，指令全保留，label 重映射正确。
#[test]
fn if_else_both_branches_reachable_kept() {
    let mut f = IRFunction::new();
    f.insts.push(Inst::jmp_if_false(0, 0)); // 0: cond → L0(else)
    f.insts.push(Inst::call(Operand::Reg(2), Operand::Reg(0), Operand::Reg(3), 1)); // 1: then
    f.insts.push(Inst::jmp(1)); // 2: → L1(end)
    f.insts.push(Inst::call(Operand::Reg(4), Operand::Reg(0), Operand::Reg(3), 1)); // 3: else
    f.insts
        .push(Inst::new(OpCode::RETURN, Operand::Reg(5), Operand::None, Operand::None)); // 4
    f.label_pos = vec![Some(3), Some(4)];
    f.label_count = 2;

    dce(&mut f);
    assert_eq!(f.insts.len(), 5, "双分支全保留");
    assert_eq!(f.label_pos, vec![Some(3), Some(4)], "存活 label 重映射到新下标");
    assert_eq!(f.label_count, 2, "label_count 保持原值，不参与重建");
}
