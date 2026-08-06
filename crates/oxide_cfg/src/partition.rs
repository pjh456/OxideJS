//! Pass 1：块头识别。
//!
//! 块头集合 = `{0}` ∪ label 目标位置 ∪ 跳转目标 ∪ 条件跳转 fallthrough 后继。

use oxide_bytecode::opcode::OpCode;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;

/// 识别块头位置：返回 `heads`，长度 `insts.len()+1`（+1 容纳指向末尾的空尾块，Pitfall 3）。
pub(super) fn partition_blocks(f: &IRFunction) -> Vec<bool> {
    let len = f.insts.len();
    let mut heads = vec![false; len + 1];
    heads[0] = true;
    for p in f.label_pos.iter().flatten() {
        heads[*p] = true;
    }
    for (i, inst) in f.insts.iter().enumerate() {
        // label 恒在 b 槽（inst.rs 构造 API 保证，不查 rd/a 槽）。
        if let Operand::Label(l) = inst.b {
            match f.label_pos.get(l as usize).and_then(|p| *p) {
                Some(p) => heads[p] = true,
                None => debug_assert!(false, "unresolved label {l}"),
            }
        }
        // 跳转的 fallthrough 后继是块头：条件跳转否则 cond+then 融块、条件边丢失；
        // 无条件 JMP 同样必须切块头——JMP 不 fallthrough，但其后继位置需成块边界，
        // 否则 JMP 与后续指令融块、块尾判定错、Jump 边丢失（DCE 可达性误删目标块）。
        if matches!(
            inst.op,
            OpCode::JMP | OpCode::JMP_IF_TRUE | OpCode::JMP_IF_FALSE | OpCode::JMP_IF_NULLISH
        ) {
            heads[i + 1] = true;
        }
    }
    heads
}
