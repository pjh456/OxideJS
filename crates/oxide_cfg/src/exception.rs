//! Pass 3：异常边。
//!
//! 块内扫 TRY_BEGIN / TRY_FINALLY_BEGIN（块内标记，非 terminator），
//! 从所在 BB 连 Exception 边到处理入口（仅起始 BB，不扩散）。

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

use crate::{BasicBlock, EdgeKind};

/// 补异常边：从 TRY 标记所在 BB 出发，指向处理入口块。
pub(super) fn add_exception_edges(f: &IRFunction, blocks: &mut [BasicBlock]) {
    for i in 0..blocks.len() {
        let range = blocks[i].inst_range.clone();
        for inst_idx in range.start..range.end {
            let inst = &f.insts[inst_idx];
            if !matches!(inst.op, OpCode::TRY_BEGIN | OpCode::TRY_FINALLY_BEGIN) {
                continue;
            }
            if let Operand::Label(l) = inst.b {
                match f.label_pos.get(l as usize).and_then(|p| *p) {
                    Some(p) => {
                        let target = crate::split::block_id_of(p, blocks);
                        // push 前去重：块内多个 TRY 标记可指向同一目标，只保留一条边。
                        if !blocks[i].succs.contains(&(target, EdgeKind::Exception)) {
                            blocks[i].succs.push((target, EdgeKind::Exception));
                        }
                    }
                    None => debug_assert!(false, "unresolved label {l}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BasicBlock;
    use oxide_ir::inst::Inst;

    /// TRY_BEGIN 所在 BB 出发 Exception 边 → catch 入口块（仅起始 BB）。
    #[test]
    fn try_begin_emits_exception_edge_to_handler() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN → catch（label 0 → inst 2）
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 1
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 2: catch 入口
        f.label_pos = vec![Some(2)];
        f.label_count = 1;

        let mut blocks = vec![
            BasicBlock {
                inst_range: 0..2,
                preds: Vec::new(),
                succs: Vec::new(),
            },
            BasicBlock {
                inst_range: 2..3,
                preds: Vec::new(),
                succs: Vec::new(),
            },
        ];
        add_exception_edges(&f, &mut blocks);
        assert_eq!(blocks[0].succs, vec![(1, EdgeKind::Exception)], "TRY_BEGIN 所在 BB 出 Exception 边");
        assert!(blocks[1].succs.is_empty(), "catch 入口块无出边");
    }

    /// 块内多个 TRY 标记指向同一目标：Exception 边去重。
    #[test]
    fn duplicate_exception_targets_deduplicated() {
        let mut f = IRFunction::new();
        f.insts.push(Inst::try_begin(0)); // 0: TRY_BEGIN → L0
        f.insts.push(Inst::try_finally_begin(0)); // 1: TRY_FINALLY_BEGIN → 同一 L0
        f.insts
            .push(Inst::new(OpCode::NOP, Operand::None, Operand::None, Operand::None)); // 2: L0 目标
        f.label_pos = vec![Some(2)];
        f.label_count = 1;

        let mut blocks = vec![
            BasicBlock {
                inst_range: 0..2,
                preds: Vec::new(),
                succs: Vec::new(),
            },
            BasicBlock {
                inst_range: 2..3,
                preds: Vec::new(),
                succs: Vec::new(),
            },
        ];
        add_exception_edges(&f, &mut blocks);
        assert_eq!(
            blocks[0].succs,
            vec![(1, EdgeKind::Exception)],
            "两个 TRY 标记指向同一目标只保留一条 Exception 边"
        );
    }
}
