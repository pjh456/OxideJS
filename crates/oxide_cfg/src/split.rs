//! Pass 2：线性切块 + 出边（跳转/fallthrough）。
//!
//! 消费 Pass 1 的 `heads`，产出实块（`preds` 暂空，`succs` 已填跳转/fallthrough 边）
//! 与 `exit_id`（= 实块数，exit 哨兵将放在该下标）。

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

use crate::{BasicBlock, EdgeKind};

/// 按块尾指令判定出边（含 label 目标解析）。
pub(super) fn split_and_edges(f: &IRFunction, heads: &[bool]) -> (Vec<BasicBlock>, usize) {
    let len = f.insts.len();
    let head_positions: Vec<usize> = heads.iter().enumerate().filter(|(_, &h)| h).map(|(i, _)| i).collect();
    let mut blocks: Vec<BasicBlock> = Vec::with_capacity(head_positions.len());
    for (idx, &start) in head_positions.iter().enumerate() {
        let end = head_positions.get(idx + 1).copied().unwrap_or(len);
        blocks.push(BasicBlock {
            inst_range: start..end,
            preds: Vec::new(),
            succs: Vec::new(),
        });
    }
    let exit_id = blocks.len(); // exit 哨兵块号 = 实块数

    for i in 0..blocks.len() {
        if blocks[i].inst_range.is_empty() {
            continue; // 空尾块（label 指向 len）：无指令可判出边
        }
        let last = &f.insts[blocks[i].inst_range.end - 1];
        if !crate::is_terminator(last.op) {
            // 非 terminator：顺序落入下一块；已是最后一块则无出边。
            if i + 1 < blocks.len() {
                blocks[i].succs.push((i + 1, EdgeKind::Fallthrough));
            }
            continue;
        }
        match last.op {
            OpCode::JMP | OpCode::BREAK | OpCode::CONTINUE => {
                if let Operand::Label(l) = last.b {
                    match f.label_pos.get(l as usize).and_then(|p| *p) {
                        Some(p) => {
                            let target = block_id_of(p, &blocks);
                            blocks[i].succs.push((target, EdgeKind::Jump));
                        }
                        None => debug_assert!(false, "unresolved label {l}"),
                    }
                }
            }
            OpCode::JMP_IF_TRUE | OpCode::JMP_IF_FALSE | OpCode::JMP_IF_NULLISH => {
                if let Operand::Label(l) = last.b {
                    match f.label_pos.get(l as usize).and_then(|p| *p) {
                        Some(p) => {
                            let target = block_id_of(p, &blocks);
                            blocks[i].succs.push((target, EdgeKind::Jump));
                        }
                        None => debug_assert!(false, "unresolved label {l}"),
                    }
                }
                // fallthrough 后继：下一块（Pass 1 置 heads[i+1] 保证存在）。
                blocks[i].succs.push((i + 1, EdgeKind::Fallthrough));
            }
            OpCode::RETURN | OpCode::HALT => {
                // RETURN/HALT 汇入 exit 哨兵，用 Fallthrough 表达正常流出口。
                blocks[i].succs.push((exit_id, EdgeKind::Fallthrough));
            }
            OpCode::THROW => { /* 无出边：异常传播，不建恢复路径 */ }
            _ => unreachable!("is_terminator 已穷尽其余分支"),
        }
    }

    (blocks, exit_id)
}

/// label 目标指令位置 → 块 id。blocks 按 `inst_range.start` 升序，p 必为某块块头
/// （Pass 1 已把所有 label 目标置为块头）。
pub(super) fn block_id_of(p: usize, blocks: &[BasicBlock]) -> usize {
    blocks.partition_point(|b| b.inst_range.start <= p) - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_ir::inst::Inst;

    /// JMP → Jump 边指向 label 目标块。
    #[test]
    fn unconditional_jmp_produces_jump_edge() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp(0)); // 0: → L0
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 1: L0 目标
        f.label_pos = vec![Some(1)];
        f.label_count = 1;

        // heads: 0（entry）+ 1（label 目标）+ JMP 后继 1
        let heads = crate::partition::partition_blocks(&f);
        let (blocks, _exit_id) = split_and_edges(&f, &heads);
        assert_eq!(blocks[0].succs, vec![(1, EdgeKind::Jump)], "JMP 产出 Jump 边");
        assert_eq!(blocks[1].succs, vec![(2, EdgeKind::Fallthrough)], "RETURN 汇 exit");
    }

    /// 条件跳转双出边：Jump → 目标块 + Fallthrough → 下一块。
    #[test]
    fn conditional_jump_dual_edges() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp_if_false(1, 0)); // 0: cond → L0(else)
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1: then
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2: else 入口
        f.label_pos = vec![Some(2)];
        f.label_count = 1;

        let heads = crate::partition::partition_blocks(&f);
        let (blocks, _exit_id) = split_and_edges(&f, &heads);
        assert_eq!(
            blocks[0].succs,
            vec![(2, EdgeKind::Jump), (1, EdgeKind::Fallthrough)],
            "条件跳转双出边：Jump→else + Fallthrough→then"
        );
    }

    /// 非 terminator 块尾：Fallthrough → 下一块；RETURN 汇 exit 哨兵。
    #[test]
    fn non_terminator_fallthrough_and_return_to_exit() {
        let mut f = IRFunction::new();
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 0
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None)); // 2

        let heads = crate::partition::partition_blocks(&f);
        let (blocks, exit_id) = split_and_edges(&f, &heads);
        assert_eq!(blocks.len(), 1, "线性函数单块");
        assert_eq!(exit_id, 1, "exit 哨兵 id = 实块数");
        assert_eq!(blocks[0].succs, vec![(1, EdgeKind::Fallthrough)], "RETURN Fallthrough 汇 exit");
    }

    /// exit_id 恒为实块数（split 的 exit 约定，finalize 追加哨兵）。
    #[test]
    fn exit_id_equals_block_count() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::jmp_if_false(1, 0));
        f.insts.push(Inst::call(Operand::Reg(2), Operand::Reg(0), Operand::Reg(3), 1));
        f.insts.push(Inst::jmp(1));
        f.insts.push(Inst::call(Operand::Reg(4), Operand::Reg(0), Operand::Reg(3), 1));
        f.insts
            .push(Inst::new(OpCode::RETURN, Operand::None, Operand::None, Operand::None));
        f.label_pos = vec![Some(3), Some(4)];
        f.label_count = 2;

        let heads = crate::partition::partition_blocks(&f);
        let (blocks, exit_id) = split_and_edges(&f, &heads);
        assert_eq!(exit_id, blocks.len(), "exit_id = 实块数（4 实块）");
    }
}
