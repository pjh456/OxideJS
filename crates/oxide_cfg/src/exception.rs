//! Pass 3：异常边。
//!
//! 块内扫 TRY_BEGIN / TRY_FINALLY_BEGIN（块内标记，非 terminator，Pitfall 1），
//! 从所在 BB 连 Exception 边到处理入口（D-07/D-08：仅起始 BB，不扩散）。

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
                        // push 前去重（Pitfall 4：块内多个 TRY 标记可指向同一目标）。
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
