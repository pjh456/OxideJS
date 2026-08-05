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
        blocks.push(BasicBlock { inst_range: start..end, preds: Vec::new(), succs: Vec::new() });
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
            OpCode::JMP => {
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
                // RETURN/HALT 汇入 exit 哨兵，用 Fallthrough 表达正常流出口（A1 语义归属）。
                blocks[i].succs.push((exit_id, EdgeKind::Fallthrough));
            }
            OpCode::THROW => { /* 无出边：异常传播，D-07 不建恢复路径 */ }
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
