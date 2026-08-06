//! Pass 4：exit 哨兵收尾 + succs 反推 preds。

use crate::{BasicBlock, Cfg};

/// 追加 exit 哨兵块（`0..0`，置末尾），从各块 succs 反推 preds。
pub(super) fn finalize(mut blocks: Vec<BasicBlock>, exit_id: usize) -> Cfg {
    blocks.push(BasicBlock { inst_range: 0..0, preds: Vec::new(), succs: Vec::new() });
    let mut succs_total = 0usize;
    let mut preds_total = 0usize;
    for i in 0..blocks.len() {
        succs_total += blocks[i].succs.len();
        let targets: Vec<crate::BBId> = blocks[i].succs.iter().map(|&(t, _)| t).collect();
        for target in targets {
            blocks[target].preds.push(i);
            preds_total += 1;
        }
    }
    debug_assert_eq!(succs_total, preds_total, "succs/preds 不对称（Pitfall 4）");

    Cfg { blocks, entry: 0, exit: exit_id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BasicBlock, EdgeKind};

    /// exit 哨兵恒在 blocks 末尾（空块 0..0），preds 反推正确。
    #[test]
    fn exit_sentinel_appended_with_preds_reversed() {
        let blocks_in = vec![
            BasicBlock { inst_range: 0..1, preds: Vec::new(), succs: vec![(1, EdgeKind::Jump)] },
            BasicBlock { inst_range: 1..2, preds: Vec::new(), succs: vec![(2, EdgeKind::Fallthrough)] },
        ];
        let cfg = finalize(blocks_in, 2);
        assert_eq!(cfg.blocks.len(), 3, "2 实块 + exit 哨兵");
        assert_eq!(cfg.entry, 0);
        assert_eq!(cfg.exit, 2);
        assert_eq!(cfg.blocks[2].inst_range, 0..0, "exit 是空块");
        assert_eq!(cfg.blocks[1].preds, vec![0], "块 1 preds 反推自块 0 的 Jump 边");
        assert_eq!(cfg.blocks[2].preds, vec![1], "exit preds 反推自块 1 的 Fallthrough 边");
    }

    /// succs/preds 对称：total 边数一致（Pitfall 4 防御性约束）。
    #[test]
    fn preds_and_succs_symmetric() {
        let blocks_in = vec![
            BasicBlock { inst_range: 0..1, preds: Vec::new(), succs: vec![(1, EdgeKind::Jump)] },
            BasicBlock { inst_range: 1..2, preds: Vec::new(), succs: vec![(2, EdgeKind::Fallthrough)] },
        ];
        let cfg = finalize(blocks_in, 2);
        let succs_total: usize = cfg.blocks.iter().map(|b| b.succs.len()).sum();
        let preds_total: usize = cfg.blocks.iter().map(|b| b.preds.len()).sum();
        assert_eq!(succs_total, preds_total, "succs/preds 对称");
    }
}
