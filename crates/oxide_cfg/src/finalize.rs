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
